//! A gateway that routes channels across machines.
//!
//! Profiles run on the machines that hold their agents' keys: *hosts*.
//! People reach them through a *gateway* with a public address, which
//! answers the HTTP channel's API (`POST /v1/profiles/<name>/messages`,
//! with the profile's channel tokens) and passes each message to the host
//! that serves the profile. A host connects out to the gateway, so it needs
//! no address of its own, and proves for each profile it serves that it
//! holds the key of the profile's agent, the agent the gateway's tokens for
//! the profile name: it signs the gateway's challenge, and the gateway
//! checks the signature against the agent's account policy on the ledger.
//! The gateway routes a profile's messages only to a host that did, and
//! only while the agent is in good standing (`GET
//! /v1/agents/{id}`, at most [`STANDING_TTL`] old): a suspended, revoked or
//! expired agent's messages are answered by the gateway itself and never
//! reach the host.
//!
//! For hosts:
//!
//! | Method | Path | Body or query | Answer |
//! |---|---|---|---|
//! | POST | `/v1/hosts/hello` | `{agent}` | `{challenge}`, valid for [`CHALLENGE_TTL`] |
//! | POST | `/v1/hosts/login` | `{agent, profile, challenge, policy, authorization, host?}` | `{host}`, a host token |
//! | GET | `/v1/hosts/inbox` | `?wait=<seconds>` | the messages waiting for the host's profiles |
//! | POST | `/v1/hosts/outbox` | `{id, said, lines, session}` | the answer to one message |
//!
//! A host token goes in `Authorization: Bearer`. The signed message names
//! the gateway (`--name`), so a login cannot be replayed at another one.
//! Messages and answers cross the gateway in the clear: put it behind TLS.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io::Read;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use sion_core::encode::{decode, encode};
use sion_core::hash::{hash_domain, Hash32};
use sion_core::ids::{AccountId, ObjectId};
use sion_crypto::{AuthPolicy, Authorization};
use sion_exec::object::ObjectData;
use sion_node::Query;

use crate::channels::{Access, Answer};
use crate::error::CliError;
use crate::ledger::LedgerExt;
use crate::runtime::SharedLedger;

/// How long a challenge may be answered.
pub const CHALLENGE_TTL: Duration = Duration::from_secs(60);

/// How long an agent's standing is trusted before the gateway asks again.
pub const STANDING_TTL: Duration = Duration::from_secs(5);

/// How long a message waits for its host's answer.
pub const ANSWER_TIMEOUT: Duration = Duration::from_secs(120);

/// The longest a host's inbox request may wait.
pub const MAX_WAIT: u64 = 50;

/// The most logins under way at once: challenges given and not answered.
/// When there are more, the oldest is forgotten.
pub const MAX_CHALLENGES: usize = 1_024;

/// The most logins under way at once for one agent.
pub const MAX_AGENT_CHALLENGES: usize = 8;

/// The most messages waiting for their hosts' answers at once.
pub const MAX_PENDING: usize = 256;

/// The domain of the message a host signs to log in.
pub const LOGIN_DOMAIN: &str = "sion/agent/gateway-login/v1";

/// What a host signs to serve `profile` as `agent` at the gateway `name`.
pub fn login_message(name: &str, agent: &ObjectId, profile: &str, challenge: &Hash32) -> Vec<u8> {
    encode(&(name.to_string(), *agent, profile.to_string(), *challenge))
}

/// A message waiting for, or answered by, a host.
#[derive(Clone, Debug)]
struct Pending {
    host: Hash32,
    profile: String,
    key: String,
    text: String,
    taken: bool,
    answer: Option<Result<Answer, String>>,
    /// Whether whoever wrote it is told what the agent does as it works.
    stream: bool,
    /// What the host told of its work and was not passed on yet.
    events: Vec<(String, Value)>,
}

/// The most events a host may tell of one message's work: past them, the
/// rest come with its answer.
pub const MAX_EVENTS: usize = 20_000;

/// How often a host tells the gateway what the agent did since it last
/// told it, while it works on a message whose writer waits for a stream.
pub const PROGRESS_EVERY: Duration = Duration::from_millis(250);

/// A connected host.
#[derive(Clone, Debug, Default)]
struct Host {
    profiles: BTreeSet<String>,
}

/// Where the gateway stands.
#[derive(Debug, Default)]
struct State {
    challenges: BTreeMap<Hash32, (ObjectId, Instant)>,
    hosts: BTreeMap<Hash32, Host>,
    /// Profile → (host, agent).
    routes: BTreeMap<String, (Hash32, ObjectId)>,
    pending: BTreeMap<u64, Pending>,
    queue: BTreeMap<Hash32, VecDeque<u64>>,
    next: u64,
    standing: BTreeMap<ObjectId, (String, Instant)>,
}

/// A gateway, shared by its request threads.
#[derive(Clone)]
pub struct Gateway {
    name: String,
    ledger: SharedLedger,
    admin: Option<String>,
    state: Arc<(Mutex<State>, Condvar)>,
}

impl std::fmt::Debug for Gateway {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Gateway({})", self.name)
    }
}

fn random_hash() -> Hash32 {
    let mut bytes = [0u8; 32];
    rand_core::RngCore::fill_bytes(&mut rand_core::OsRng, &mut bytes);
    Hash32(bytes)
}

fn token_hash(token: &str) -> Hash32 {
    hash_domain("sion/agent/gateway-host-token/v1", token.as_bytes())
}

fn field<'a>(body: &'a Value, name: &str) -> Result<&'a str, String> {
    body[name]
        .as_str()
        .ok_or_else(|| format!("{name} is missing"))
}

fn agent_of(body: &Value) -> Result<ObjectId, String> {
    Hash32::from_hex(field(body, "agent")?)
        .map(ObjectId::from_hash)
        .map_err(|_| "agent is an id: 64 hex digits".to_string())
}

/// Told of an event of the agent's work: its name and what it says.
pub type Tell<'a> = &'a dyn Fn(&str, &Value);

impl Gateway {
    /// A gateway called `name`, checking agents against `ledger`; `admin`
    /// is a token that may speak to every profile.
    pub fn new(name: &str, ledger: SharedLedger, admin: Option<String>) -> Gateway {
        Gateway {
            name: name.to_string(),
            ledger,
            admin,
            state: Arc::new((Mutex::new(State::default()), Condvar::new())),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// `/v1/hosts/hello`: a fresh challenge for `agent`, which a token here
    /// must name as some profile's. Past [`MAX_AGENT_CHALLENGES`] for the
    /// agent, or [`MAX_CHALLENGES`] in all, the oldest is forgotten.
    pub fn hello(&self, body: &Value) -> Result<Value, String> {
        let agent = agent_of(body)?;
        let named = crate::channels::tokens()
            .map_err(|e| e.to_string())?
            .iter()
            .any(|t| t.agent == Some(agent));
        if !named {
            return Err(format!(
                "no token at this gateway names agent {}",
                agent.0.short()
            ));
        }
        let challenge = random_hash();
        let mut state = self.lock();
        let now = Instant::now();
        state
            .challenges
            .retain(|_, (_, at)| now.duration_since(*at) < CHALLENGE_TTL);
        let oldest = |state: &State, only: Option<ObjectId>| {
            state
                .challenges
                .iter()
                .filter(|(_, (a, _))| only.map_or(true, |only| *a == only))
                .min_by_key(|(_, (_, at))| *at)
                .map(|(c, _)| *c)
        };
        let mine = state
            .challenges
            .values()
            .filter(|(a, _)| *a == agent)
            .count();
        if mine >= MAX_AGENT_CHALLENGES {
            if let Some(old) = oldest(&state, Some(agent)) {
                state.challenges.remove(&old);
            }
        }
        if state.challenges.len() >= MAX_CHALLENGES {
            if let Some(old) = oldest(&state, None) {
                state.challenges.remove(&old);
            }
        }
        state.challenges.insert(challenge, (agent, now));
        Ok(json!({"challenge": challenge.to_hex(), "gateway": self.name}))
    }

    /// The account policy that must have signed for `agent`'s account, and
    /// the account itself.
    fn policy_of(&self, agent: &ObjectId, offered: &AuthPolicy) -> Result<AuthPolicy, String> {
        let charter = match self.ledger.object(*agent).map_err(|e| e.to_string())? {
            Some(object) => match object.data {
                ObjectData::Charter(charter) => charter,
                _ => return Err(format!("{agent} is not an agent")),
            },
            None => return Err(format!("no agent {agent} on the ledger")),
        };
        let account: AccountId = charter.agent;
        let view = self.ledger.account(account).map_err(|e| e.to_string())?;
        match view.object.map(|o| o.data) {
            Some(ObjectData::Account(state)) => Ok(state.policy),
            _ if offered.account_id() == account => Ok(offered.clone()),
            _ => Err("the policy offered is not the agent's account's".into()),
        }
    }

    /// The agent's standing, as of at most [`STANDING_TTL`] ago.
    fn standing(&self, agent: &ObjectId) -> Result<String, String> {
        if let Some((status, at)) = self.lock().standing.get(agent) {
            if at.elapsed() < STANDING_TTL {
                return Ok(status.clone());
            }
        }
        let value = self
            .ledger
            .query(Query::Agent(*agent))
            .map_err(|e| e.to_string())?;
        let status = value["status"].as_str().unwrap_or("unknown").to_string();
        self.lock()
            .standing
            .insert(*agent, (status.clone(), Instant::now()));
        Ok(status)
    }

    fn in_good_standing(status: &str) -> bool {
        status == "active" || status == "active, narrowed"
    }

    /// `/v1/hosts/login`: checks the signature on the challenge against the
    /// agent's account policy and the agent's standing, then routes the
    /// profile to the host (a new host, or the one named by `host`).
    pub fn login(&self, body: &Value) -> Result<Value, String> {
        let agent = agent_of(body)?;
        let profile = field(body, "profile")?.to_string();
        if !crate::profiles::valid_name(&profile) {
            return Err(format!("{profile:?} is not a profile name"));
        }
        let challenge = Hash32::from_hex(field(body, "challenge")?)
            .map_err(|_| "challenge is 64 hex digits".to_string())?;
        let policy: AuthPolicy = hex::decode(field(body, "policy")?)
            .ok()
            .and_then(|b| decode(&b).ok())
            .ok_or("policy is hex of its Borsh encoding")?;
        let authorization: Authorization = hex::decode(field(body, "authorization")?)
            .ok()
            .and_then(|b| decode(&b).ok())
            .ok_or("authorization is hex of its Borsh encoding")?;
        {
            let mut state = self.lock();
            match state.challenges.remove(&challenge) {
                Some((for_agent, at)) if for_agent == agent && at.elapsed() < CHALLENGE_TTL => {}
                _ => return Err("unknown or expired challenge".into()),
            }
        }
        // The profile's name is the gateway's to give: only an agent a token
        // here names as the profile's may serve it.
        if !crate::channels::names_agent(&profile, &agent).map_err(|e| e.to_string())? {
            return Err(format!(
                "no token at this gateway is for profile {profile} as agent {}",
                agent.0.short()
            ));
        }
        let policy = self.policy_of(&agent, &policy)?;
        policy
            .verify(
                LOGIN_DOMAIN,
                &login_message(&self.name, &agent, &profile, &challenge),
                &authorization,
            )
            .map_err(|e| format!("the signature does not verify: {e}"))?;
        let status = self.standing(&agent)?;
        if !Self::in_good_standing(&status) {
            return Err(format!("agent {} is {status}", agent.0.short()));
        }
        let mut state = self.lock();
        let (token, hash) = match body["host"].as_str() {
            Some(token) if state.hosts.contains_key(&token_hash(token)) => {
                (token.to_string(), token_hash(token))
            }
            Some(_) => return Err("unknown host".into()),
            None => {
                let token = format!("host_{}", random_hash().to_hex());
                let hash = token_hash(&token);
                state.hosts.insert(hash, Host::default());
                (token, hash)
            }
        };
        // A profile served elsewhere moves here: the newest proof wins.
        if let Some((old, _)) = state.routes.insert(profile.clone(), (hash, agent)) {
            if let Some(host) = state.hosts.get_mut(&old) {
                host.profiles.remove(&profile);
            }
        }
        if let Some(host) = state.hosts.get_mut(&hash) {
            host.profiles.insert(profile.clone());
        }
        Ok(json!({"host": token, "profile": profile, "agent": agent}))
    }

    fn host_of(&self, bearer: Option<&str>) -> Option<Hash32> {
        let hash = token_hash(bearer?);
        self.lock().hosts.contains_key(&hash).then_some(hash)
    }

    /// `/v1/hosts/inbox`: the messages waiting for the host, waiting up to
    /// `wait` seconds for one.
    pub fn inbox(&self, host: Hash32, wait: u64) -> Value {
        let deadline = Instant::now() + Duration::from_secs(wait.min(MAX_WAIT));
        let (lock, ready) = &*self.state;
        let mut state = lock.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            let ids: Vec<u64> = state
                .queue
                .get_mut(&host)
                .map(|q| q.drain(..).collect())
                .unwrap_or_default();
            if !ids.is_empty() {
                let mut out = Vec::new();
                for id in ids {
                    if let Some(p) = state.pending.get_mut(&id) {
                        p.taken = true;
                        out.push(json!({"id": id, "profile": p.profile, "key": p.key,
                            "text": p.text, "stream": p.stream}));
                    }
                }
                return json!({"messages": out});
            }
            let now = Instant::now();
            if now >= deadline {
                return json!({"messages": []});
            }
            state = ready
                .wait_timeout(state, deadline - now)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }

    /// `/v1/hosts/outbox`: a host's answer to one of its messages.
    pub fn answer(&self, host: Hash32, body: &Value) -> Result<(), String> {
        let id = body["id"].as_u64().ok_or("id is a number")?;
        let (lock, ready) = &*self.state;
        let mut state = lock.lock().unwrap_or_else(|e| e.into_inner());
        let pending = state.pending.get_mut(&id).ok_or("no such message")?;
        if pending.host != host {
            return Err("not your message".into());
        }
        pending.answer = Some(match body["error"].as_str() {
            Some(error) => Err(error.to_string()),
            None => Ok(Answer {
                session: body["session"].as_str().map(str::to_string),
                reply: body["said"].as_str().unwrap_or_default().to_string(),
                lines: body["lines"]
                    .as_array()
                    .map(|l| {
                        l.iter()
                            .filter_map(|x| x.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default(),
                ..Answer::default()
            }),
        });
        ready.notify_all();
        Ok(())
    }

    /// `/v1/hosts/progress`: what the agent did, as its host tells it,
    /// working on one of the host's messages: `{"id": …, "events":
    /// [{"event": …, "data": …}, …]}`.
    pub fn progress(&self, host: Hash32, body: &Value) -> Result<(), String> {
        let id = body["id"].as_u64().ok_or("id is a number")?;
        let (lock, ready) = &*self.state;
        let mut state = lock.lock().unwrap_or_else(|e| e.into_inner());
        let pending = state.pending.get_mut(&id).ok_or("no such message")?;
        if pending.host != host {
            return Err("not your message".into());
        }
        for told in body["events"].as_array().into_iter().flatten() {
            if pending.events.len() >= MAX_EVENTS {
                break;
            }
            if let Some(name) = told["event"].as_str() {
                pending
                    .events
                    .push((name.to_string(), told["data"].clone()));
            }
        }
        ready.notify_all();
        Ok(())
    }

    /// Passes a message for `profile` to its host and waits for the answer.
    pub fn ask(&self, profile: &str, key: &str, text: &str) -> Result<Answer, String> {
        self.ask_watched(profile, key, text, None)
    }

    /// [`Gateway::ask`], what the host tells of the agent's work passed to
    /// `tell` as it comes ([`crate::channels::converse_events`]).
    pub fn ask_watched(
        &self,
        profile: &str,
        key: &str,
        text: &str,
        tell: Option<Tell<'_>>,
    ) -> Result<Answer, String> {
        let (host, agent) = self
            .lock()
            .routes
            .get(profile)
            .copied()
            .ok_or_else(|| format!("no host serves profile {profile}"))?;
        let status = self.standing(&agent)?;
        if !Self::in_good_standing(&status) {
            return Ok(Answer {
                session: None,
                reply: format!(
                    "Agent {} is {status} on the ledger: the gateway passes it no messages.",
                    agent.0.short()
                ),
                ..Answer::default()
            });
        }
        let (lock, ready) = &*self.state;
        let mut state = lock.lock().unwrap_or_else(|e| e.into_inner());
        if state.pending.len() >= MAX_PENDING {
            return Err("the gateway has too many messages waiting: try again soon".into());
        }
        let id = state.next;
        state.next += 1;
        state.pending.insert(
            id,
            Pending {
                host,
                profile: profile.to_string(),
                key: key.to_string(),
                text: text.to_string(),
                taken: false,
                answer: None,
                stream: tell.is_some(),
                events: Vec::new(),
            },
        );
        state.queue.entry(host).or_default().push_back(id);
        ready.notify_all();
        let mut deadline = Instant::now() + ANSWER_TIMEOUT;
        loop {
            // What the host told is passed on outside the lock.
            let told: Vec<(String, Value)> = state
                .pending
                .get_mut(&id)
                .map(|p| std::mem::take(&mut p.events))
                .unwrap_or_default();
            if !told.is_empty() {
                drop(state);
                if let Some(tell) = tell {
                    for (name, data) in &told {
                        tell(name, data);
                    }
                }
                // A host at work is waited for as long as it works.
                deadline = Instant::now() + ANSWER_TIMEOUT;
                state = lock.lock().unwrap_or_else(|e| e.into_inner());
                continue;
            }
            if let Some(answer) = state.pending.get(&id).and_then(|p| p.answer.clone()) {
                state.pending.remove(&id);
                return answer;
            }
            let now = Instant::now();
            if now >= deadline {
                let taken = state.pending.remove(&id).is_some_and(|p| p.taken);
                if let Some(q) = state.queue.get_mut(&host) {
                    q.retain(|m| *m != id);
                }
                return Err(if taken {
                    "the host took the message but did not answer in time".into()
                } else {
                    "the host did not ask for its messages in time".into()
                });
            }
            state = ready
                .wait_timeout(state, deadline - now)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }

    /// The profiles routed, with their hosts' short ids, for `/v1/status`.
    pub fn routes(&self) -> Value {
        let state = self.lock();
        json!(state
            .routes
            .iter()
            .map(|(p, (h, a))| (p.clone(), json!({"agent": a, "host": h.short()})))
            .collect::<BTreeMap<_, _>>())
    }
}

fn respond(request: tiny_http::Request, status: u16, body: &Value) {
    let header = tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
        .expect("a valid header");
    let _ = request.respond(
        tiny_http::Response::from_string(body.to_string())
            .with_status_code(status)
            .with_header(header),
    );
}

fn bearer(request: &tiny_http::Request) -> Option<String> {
    request
        .headers()
        .iter()
        .find(|h| h.field.equiv("Authorization"))
        .and_then(|h| h.value.as_str().strip_prefix("Bearer ").map(str::to_string))
}

fn body_of(request: &mut tiny_http::Request) -> Option<Value> {
    let mut body = String::new();
    request
        .as_reader()
        // A login carries two post-quantum signatures, hex-encoded.
        .take(crate::channels::MAX_MESSAGE as u64 + (128 << 10))
        .read_to_string(&mut body)
        .ok()?;
    serde_json::from_str(&body).ok()
}

/// Handles one request.
fn handle(gateway: &Gateway, mut request: tiny_http::Request) {
    let url = request.url().to_string();
    let (path, query) = url.split_once('?').unwrap_or((&url, ""));
    let path = path.to_string();
    let method = request.method().as_str().to_string();
    let token = bearer(&request);
    match (method.as_str(), path.as_str()) {
        ("GET", "/v1/status") => respond(
            request,
            200,
            &json!({"gateway": gateway.name, "routes": gateway.routes()}),
        ),
        ("POST", "/v1/hosts/hello") => match body_of(&mut request).map(|b| gateway.hello(&b)) {
            Some(Ok(v)) => respond(request, 200, &v),
            Some(Err(e)) => respond(request, 400, &json!({"error": e})),
            None => respond(request, 400, &json!({"error": "send JSON"})),
        },
        ("POST", "/v1/hosts/login") => match body_of(&mut request).map(|b| gateway.login(&b)) {
            Some(Ok(v)) => respond(request, 200, &v),
            Some(Err(e)) => respond(request, 403, &json!({"error": e})),
            None => respond(request, 400, &json!({"error": "send JSON"})),
        },
        ("GET", "/v1/hosts/inbox") => match gateway.host_of(token.as_deref()) {
            Some(host) => {
                let wait = query
                    .split('&')
                    .find_map(|p| p.strip_prefix("wait="))
                    .and_then(|w| w.parse().ok())
                    .unwrap_or(30);
                let messages = gateway.inbox(host, wait);
                respond(request, 200, &messages)
            }
            None => respond(request, 401, &json!({"error": "log in first"})),
        },
        ("POST", "/v1/hosts/progress") => match gateway.host_of(token.as_deref()) {
            Some(host) => match body_of(&mut request).map(|b| gateway.progress(host, &b)) {
                Some(Ok(())) => respond(request, 200, &json!({"ok": true})),
                Some(Err(e)) => respond(request, 400, &json!({"error": e})),
                None => respond(request, 400, &json!({"error": "send JSON"})),
            },
            None => respond(request, 401, &json!({"error": "log in first"})),
        },
        ("POST", "/v1/hosts/outbox") => match gateway.host_of(token.as_deref()) {
            Some(host) => match body_of(&mut request).map(|b| gateway.answer(host, &b)) {
                Some(Ok(())) => respond(request, 200, &json!({"ok": true})),
                Some(Err(e)) => respond(request, 400, &json!({"error": e})),
                None => respond(request, 400, &json!({"error": "send JSON"})),
            },
            None => respond(request, 401, &json!({"error": "log in first"})),
        },
        ("POST", p) if p.starts_with("/v1/profiles/") && p.ends_with("/messages") => {
            let name = p["/v1/profiles/".len()..p.len() - "/messages".len()].to_string();
            let body = body_of(&mut request);
            let text = body
                .as_ref()
                .and_then(|b| b["text"].as_str())
                .map(str::to_string);
            let Some(text) = text else {
                return respond(
                    request,
                    400,
                    &json!({"error": "send {\"text\": …} as JSON"}),
                );
            };
            let access = crate::channels::access(&name, token.as_deref(), gateway.admin.as_deref());
            let refused = crate::channels::refused_for_talk(&text);
            if access == Access::None {
                return respond(
                    request,
                    401,
                    &json!({"error": crate::channels::refusal(token.as_deref())}),
                );
            }
            if let Some(refused) = refused.filter(|_| access != Access::Approve) {
                return respond(request, 403, &json!({ "error": refused }));
            }
            let conversation = body
                .as_ref()
                .and_then(|b| b["conversation"].as_str())
                .map(str::to_string)
                .unwrap_or_else(crate::runtime::new_id);
            let key = format!("gateway:{name}:{conversation}");
            // A stream: what the host tells of the agent's work, as Server-
            // Sent Events, then the answer.
            if body.as_ref().is_some_and(|b| b["stream"] == true) {
                let (tell, pieces) = std::sync::mpsc::channel::<Vec<u8>>();
                let gateway = gateway.clone();
                std::thread::spawn(move || {
                    let told = |event: &str, data: &Value| {
                        let _ = tell.send(crate::channels::event(Some(event), data));
                    };
                    let done = match gateway.ask_watched(&name, &key, &text, Some(&told)) {
                        Ok(answer) => (
                            "done",
                            json!({"conversation": conversation, "session": answer.session,
                                "said": answer.reply, "lines": answer.lines}),
                        ),
                        Err(e) => ("error", json!({ "error": e })),
                    };
                    told(done.0, &done.1);
                });
                return crate::channels::respond_streaming(request, pieces);
            }
            match gateway.ask(&name, &key, &text) {
                Ok(answer) => respond(
                    request,
                    200,
                    &json!({"conversation": conversation, "session": answer.session,
                            "said": answer.reply, "lines": answer.lines}),
                ),
                Err(e) => respond(request, 503, &json!({"error": e})),
            }
        }
        _ => respond(request, 404, &json!({"error": "not found"})),
    }
}

/// Serves the gateway on `listen` until stopped, one thread per request.
pub fn serve(gateway: Gateway, listen: &str) -> Result<(), CliError> {
    let server = tiny_http::Server::http(listen)
        .map_err(|e| CliError::Usage(format!("cannot listen on {listen}: {e}")))?;
    println!(
        "gateway {} on http://{listen}: hosts connect to /v1/hosts, people write to /v1/profiles/<name>/messages",
        gateway.name
    );
    for request in server.incoming_requests() {
        let gateway = gateway.clone();
        std::thread::spawn(move || handle(&gateway, request));
    }
    Ok(())
}

/// Why a gateway refused a request: the `error` its JSON answer gives, or
/// the error itself.
fn reason(error: &sion_http::HttpError) -> String {
    match error {
        sion_http::HttpError::Status { body, .. } => serde_json::from_str::<Value>(body)
            .ok()
            .and_then(|v| v["error"].as_str().map(str::to_string))
            .unwrap_or_else(|| error.to_string()),
        other => other.to_string(),
    }
}

/// Serves `profiles` through the gateway at `url` (called `name`): logs in
/// for each with its agent's wallet, then answers its messages until
/// stopped (or for `messages` messages), logging in again if the gateway
/// forgets the host.
pub fn host(
    ledger: SharedLedger,
    url: &str,
    profiles: &[crate::profiles::Profile],
    messages: Option<usize>,
) -> Result<(), CliError> {
    let client = sion_http::Client::new(sion_http::Options {
        timeout: Duration::from_secs(MAX_WAIT + 30),
        allow_private: true,
        ..sion_http::Options::default()
    })
    .map_err(|e| CliError::Connection(e.to_string()))?;
    let base = url.trim_end_matches('/').to_string();
    let post = |path: &str,
                headers: &[(&str, &str)],
                body: &Value|
     -> Result<Value, sion_http::HttpError> {
        let fetched = client.post_json(
            &format!("{base}{path}"),
            headers,
            body.to_string().as_bytes(),
            1 << 20,
        )?;
        serde_json::from_slice(&fetched.body).map_err(|e| {
            sion_http::HttpError::Transport(format!("the gateway did not answer JSON: {e}"))
        })
    };
    let login = || -> Result<String, CliError> {
        let mut host: Option<String> = None;
        for profile in profiles {
            let hello = post("/v1/hosts/hello", &[], &json!({"agent": profile.agent}))
                .map_err(|e| CliError::Connection(reason(&e)))?;
            let challenge = Hash32::from_hex(hello["challenge"].as_str().unwrap_or_default())
                .map_err(|_| CliError::Connection("the gateway sent no challenge".into()))?;
            let name = hello["gateway"].as_str().unwrap_or_default().to_string();
            let wallet = crate::session::wallet_for(&profile.wallet);
            let authorization = wallet
                .keys
                .authorize(
                    LOGIN_DOMAIN,
                    &login_message(&name, &profile.agent, &profile.name, &challenge),
                )
                .map_err(|e| CliError::Usage(e.to_string()))?;
            let mut body = json!({
                "agent": profile.agent,
                "profile": profile.name,
                "challenge": challenge.to_hex(),
                "policy": hex::encode(encode(&wallet.policy)),
                "authorization": hex::encode(encode(&authorization)),
            });
            if let Some(token) = &host {
                body["host"] = json!(token);
            }
            let answer = match post("/v1/hosts/login", &[], &body) {
                Ok(answer) => answer,
                Err(e @ sion_http::HttpError::Status { .. }) => {
                    return Err(CliError::Usage(format!(
                        "the gateway refused {}: {}",
                        profile.name,
                        reason(&e)
                    )))
                }
                Err(e) => return Err(CliError::Connection(e.to_string())),
            };
            let Some(token) = answer["host"].as_str() else {
                return Err(CliError::Connection(format!(
                    "the gateway logged {} in without a host token",
                    profile.name
                )));
            };
            println!("serving {} at gateway {name}", profile.name);
            host = Some(token.to_string());
        }
        host.ok_or_else(|| CliError::Usage("serve at least one profile".into()))
    };
    let mut token = login()?;
    let mut answered = 0usize;
    loop {
        let auth = format!("Bearer {token}");
        let fetched = client.request(
            "GET",
            &format!("{base}/v1/hosts/inbox?wait={MAX_WAIT}"),
            &[("Authorization", &auth)],
            None,
            4 << 20,
        );
        let inbox: Value = match fetched {
            Ok(f) => serde_json::from_slice(&f.body).unwrap_or_default(),
            Err(sion_http::HttpError::Status { status: 401, .. }) => {
                token = login()?;
                continue;
            }
            Err(e) => return Err(CliError::Connection(e.to_string())),
        };
        for message in inbox["messages"].as_array().into_iter().flatten() {
            let id = message["id"].as_u64().unwrap_or_default();
            let name = message["profile"].as_str().unwrap_or_default();
            let key = message["key"].as_str().unwrap_or_default();
            let text = message["text"].as_str().unwrap_or_default();
            let streams = message["stream"] == true;
            let body = match profiles.iter().find(|p| p.name == name) {
                Some(profile) => {
                    println!("{name} ({key}): {text}");
                    let answered = if streams {
                        // What the agent does is told to the gateway as it
                        // goes, a batch every PROGRESS_EVERY.
                        let told: Mutex<Vec<Value>> = Mutex::new(Vec::new());
                        let working = std::sync::atomic::AtomicBool::new(true);
                        let tell = |event: &str, data: Value| {
                            if let Ok(mut told) = told.lock() {
                                told.push(json!({"event": event, "data": data}));
                            }
                        };
                        let flush = || {
                            let events: Vec<Value> = told
                                .lock()
                                .map(|mut t| std::mem::take(&mut *t))
                                .unwrap_or_default();
                            if !events.is_empty() {
                                let _ = post(
                                    "/v1/hosts/progress",
                                    &[("Authorization", &auth)],
                                    &json!({"id": id, "events": events}),
                                );
                            }
                        };
                        let answered = std::thread::scope(|scope| {
                            scope.spawn(|| {
                                while working.load(std::sync::atomic::Ordering::SeqCst) {
                                    std::thread::sleep(PROGRESS_EVERY);
                                    flush();
                                }
                            });
                            let answered = crate::channels::converse_events(
                                ledger.clone(),
                                profile,
                                key,
                                text,
                                &[],
                                &tell,
                            );
                            working.store(false, std::sync::atomic::Ordering::SeqCst);
                            answered
                        });
                        // The last of it, before the answer.
                        flush();
                        answered
                    } else {
                        crate::channels::converse(ledger.clone(), profile, key, text)
                    };
                    match answered {
                        Ok(answer) => {
                            for line in &answer.lines {
                                println!("  {line}");
                            }
                            json!({"id": id, "said": answer.reply,
                                   "lines": answer.lines, "session": answer.session})
                        }
                        Err(e) => {
                            println!("  {e}");
                            json!({"id": id, "error": e.to_string()})
                        }
                    }
                }
                None => json!({"id": id, "error": format!("this host does not serve {name}")}),
            };
            match post("/v1/hosts/outbox", &[("Authorization", &auth)], &body) {
                Ok(_) => {}
                // The gateway restarted and forgot this host, and with it
                // whoever waited for the answer.
                Err(sion_http::HttpError::Status { status: 401, .. }) => {
                    println!("the gateway forgot this host: logging in again");
                    token = login()?;
                    break;
                }
                // Whoever asked stopped waiting (the gateway waits
                // ANSWER_TIMEOUT): the answer has no one to go to.
                Err(e @ sion_http::HttpError::Status { .. }) => {
                    println!(
                        "the gateway did not take the answer to {name}: {}",
                        reason(&e)
                    );
                }
                Err(e) => return Err(CliError::Connection(e.to_string())),
            }
            answered += 1;
            if messages.is_some_and(|most| answered >= most) {
                return Ok(());
            }
        }
    }
}
