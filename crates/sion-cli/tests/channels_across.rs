//! Channels across machines, on a devnet: a profile's own tokens reach only
//! it, and decide what it waits on only if made to; a gateway passes
//! messages to the host that proved the agent's key, and none once the
//! agent is suspended; messengers (Slack, Discord, Matrix and Signal,
//! against stand-ins for their APIs) answer the chats they are told to,
//! and only approvers decide.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use sion_agent::Autonomy;
use sion_cli::ledger::LedgerExt;
use sion_cli::profiles::Profile;
use sion_cli::runtime::SharedLedger;
use sion_cli::session::Session;
use sion_core::ids::{AssetId, ObjectId, Pulse};
use sion_core::Amount;
use sion_exec::charter::Caveats;
use sion_exec::execute::Event;
use sion_exec::TxKind;
use sion_node::{Devnet, DevnetConfig};

fn register(session: &mut Session<'_>, ledger: &SharedLedger, wallet: &str) -> ObjectId {
    let pulse = ledger.as_ref().pulse().unwrap();
    let account = session.id(wallet);
    let view = session
        .must(
            "alice",
            TxKind::RegisterAgent {
                agent: account,
                card: sion_core::hash::Hash32::ZERO,
                caveats: Caveats {
                    payees: None,
                    max_per_payment: Amount::units(10),
                    burst: Amount::units(50),
                    rate: Amount(50_000),
                    expires_at: Pulse(pulse.0 + 100_000),
                    evidence_above: None,
                    evidence_policy: sion_assurance::Policy::standard(),
                    receipts: true,
                    may_delegate: false,
                },
                funding: Amount::units(100),
                agent_bond: Amount::ZERO,
            },
            Vec::new(),
        )
        .unwrap();
    view.effects
        .events
        .iter()
        .find_map(|e| match e {
            Event::AgentRegistered { agent, .. } => Some(*agent),
            _ => None,
        })
        .unwrap()
}

fn profile(
    home: &std::path::Path,
    name: &str,
    agent: ObjectId,
    autonomy: Autonomy,
    steps: Value,
) -> Profile {
    let path = home.join(format!("{name}.json"));
    std::fs::write(&path, steps.to_string()).unwrap();
    Profile {
        name: name.into(),
        wallet: name.into(),
        agent,
        models: vec![format!("script:{}", path.display())],
        autonomy,
        memory: None,
        skills: None,
        workspace: None,
        web: false,
        delegate: false,
        subagent_models: Vec::new(),
        adjudicator: None,
        reflect: false,
        shell: None,
        mcp: Vec::new(),
        storage: false,
        instructions: None,
        skill_approvals: true,
        screen: Default::default(),
    }
}

fn client() -> sion_http::Client {
    sion_http::Client::new(sion_http::Options {
        allow_private: true,
        proxy_from_env: false,
        timeout: std::time::Duration::from_secs(150),
        ..sion_http::Options::default()
    })
    .unwrap()
}

fn free_listen() -> String {
    let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    format!("127.0.0.1:{}", probe.local_addr().unwrap().port())
}

/// Posts `text` to `profile` at `base` with `token`: the status and the
/// answer. Retries while the server is not up yet.
fn say(
    base: &str,
    profile: &str,
    token: Option<&str>,
    text: &str,
    conversation: &str,
) -> (u16, Value) {
    let auth = token.map(|t| format!("Bearer {t}"));
    let headers: Vec<(&str, &str)> = auth.iter().map(|a| ("Authorization", a.as_str())).collect();
    let body = json!({"text": text, "conversation": conversation}).to_string();
    for _ in 0..100 {
        match client().post_json(
            &format!("{base}/v1/profiles/{profile}/messages"),
            &headers,
            body.as_bytes(),
            1 << 20,
        ) {
            Ok(answer) => return (answer.status, serde_json::from_slice(&answer.body).unwrap()),
            Err(sion_http::HttpError::Status { status, body }) => {
                return (
                    status,
                    serde_json::from_str(&body).unwrap_or(json!({"body": body})),
                )
            }
            Err(_) => std::thread::sleep(std::time::Duration::from_millis(100)),
        }
    }
    panic!("{base} never answered");
}

/// A stand-in for a messenger's API: answers each request with what
/// `answer` returns for its method, path and body, and keeps them all.
struct StandIn {
    base: String,
    seen: Arc<Mutex<Vec<(String, String, Value)>>>,
}

impl StandIn {
    fn start(answer: impl Fn(&str, &str, &Value) -> Value + Send + 'static) -> StandIn {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let base = format!("http://{}", server.server_addr());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let kept = seen.clone();
        std::thread::spawn(move || {
            for mut request in server.incoming_requests() {
                let mut body = String::new();
                std::io::Read::read_to_string(request.as_reader(), &mut body).unwrap();
                let body: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
                let method = request.method().as_str().to_string();
                let url = request.url().to_string();
                let reply = answer(&method, &url, &body);
                kept.lock().unwrap().push((method, url, body));
                let _ = request.respond(tiny_http::Response::from_string(reply.to_string()));
            }
        });
        StandIn { base, seen }
    }

    /// The bodies of the requests other than GETs whose URL has `path_part`.
    fn sent(&self, path_part: &str) -> Vec<Value> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .filter(|(method, url, _)| method != "GET" && url.contains(path_part))
            .map(|(_, _, body)| body.clone())
            .collect()
    }
}

#[test]
fn profiles_are_reached_by_their_own_tokens_across_machines_and_messengers() {
    let home = std::env::temp_dir().join(format!("sion-channels-across-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    // The only test in this binary, so the environment is its own.
    std::env::set_var("SION_HOME", &home);

    let devnet = Arc::new(
        Devnet::start(DevnetConfig {
            rounds_per_pulse: 2,
            prove_chunk: None,
            ..DevnetConfig::default()
        })
        .unwrap(),
    );
    let ledger: SharedLedger = devnet.clone();
    let mut session = Session::open(ledger.as_ref()).unwrap();
    session
        .fund("alice", AssetId::usd(), Amount::units(1_000))
        .unwrap();
    let lead = register(&mut session, &ledger, "lead");
    let payer = register(&mut session, &ledger, "payer");
    for wallet in ["lead", "payer", "bob"] {
        session
            .fund(wallet, AssetId::usd(), Amount::units(1))
            .unwrap();
    }
    let lead_profile = profile(
        &home,
        "lead",
        lead,
        Autonomy::L3,
        json!([{"call": "ledger_balance"}, {"say": "Budget checked."}]),
    );
    let payer_profile = profile(
        &home,
        "payer",
        payer,
        Autonomy::L5,
        json!([
            {"call": "ledger_pay", "input": {"to": "bob", "amount": "1", "purpose": "the test"}},
            {"say": "Paid bob 1 USD."}
        ]),
    );
    sion_cli::profiles::save_all(
        &[
            ("lead".to_string(), lead_profile.clone()),
            ("payer".to_string(), payer_profile.clone()),
        ]
        .into_iter()
        .collect(),
    )
    .unwrap();

    // Per-profile tokens on the HTTP channel, without a machine token.
    let (_, talk) = sion_cli::channels::add_token("payer", payer, false).unwrap();
    let (approver, decide) = sion_cli::channels::add_token("payer", payer, true).unwrap();
    let (_, lead_token) = sion_cli::channels::add_token("lead", lead, false).unwrap();
    assert!(
        sion_cli::channels::add_token("lead", payer, false).is_err(),
        "a profile's tokens name one agent"
    );
    assert_eq!(sion_cli::channels::tokens().unwrap().len(), 3);
    assert!(
        !std::fs::read_to_string(home.join("agents/channel-tokens.json"))
            .unwrap()
            .contains(&talk)
    );
    let listen = free_listen();
    let base = format!("http://{listen}");
    let serving = {
        let ledger = ledger.clone();
        let listen = listen.clone();
        std::thread::spawn(move || sion_cli::channels::serve_with(ledger, &listen, None, Some(4)))
    };
    let (status, answer) = say(&base, "payer", Some(&lead_token), "Pay bob 1 USD.", "c1");
    assert_eq!(status, 401, "another profile's token");
    assert_eq!(answer["error"], "this token does not reach this profile");
    let (status, answer) = say(&base, "payer", Some(&talk), "Pay bob 1 USD.", "c1");
    assert_eq!(status, 200, "{answer}");
    assert!(
        answer["lines"].to_string().contains("ledger_pay"),
        "{answer}"
    );
    let (status, answer) = say(&base, "payer", Some(&talk), "/approve", "c1");
    assert_eq!(status, 403, "{answer}");
    assert!(
        answer["error"].as_str().unwrap().contains("not approve"),
        "{answer}"
    );
    let (status, answer) = say(&base, "payer", Some(&decide), "/approve", "c1");
    assert_eq!(status, 200, "{answer}");
    assert_eq!(answer["said"], "Paid bob 1 USD.", "{answer}");
    serving.join().unwrap().unwrap();
    assert!(sion_cli::channels::remove_token("payer", &approver.id).unwrap());
    assert!(!sion_cli::channels::remove_token("payer", &approver.id).unwrap());

    // A gateway: the host connects out, proves the agent's key, and answers.
    let gateway_listen = free_listen();
    let gateway_url = format!("http://{gateway_listen}");
    {
        let gateway = sion_cli::gateway::Gateway::new("gateway-test", ledger.clone(), None);
        let listen = gateway_listen.clone();
        std::thread::spawn(move || sion_cli::gateway::serve(gateway, &listen));
    }
    let hosting = {
        let ledger = ledger.clone();
        let url = gateway_url.clone();
        let profiles = vec![lead_profile.clone()];
        std::thread::spawn(move || {
            for _ in 0..100 {
                match sion_cli::gateway::host(ledger.clone(), &url, &profiles, Some(1)) {
                    Err(sion_cli::error::CliError::Connection(_)) => {
                        std::thread::sleep(std::time::Duration::from_millis(100))
                    }
                    other => return other,
                }
            }
            panic!("the gateway never came up")
        })
    };
    let mut routed = false;
    for _ in 0..200 {
        if let Ok(status) = client().get(&format!("{gateway_url}/v1/status"), 1 << 20) {
            let value: Value = serde_json::from_slice(&status.body).unwrap();
            if value["routes"]["lead"].is_object() {
                routed = true;
                break;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(routed, "the host logged in");
    let (status, _) = say(&gateway_url, "lead", None, "How much budget is left?", "g1");
    assert_eq!(status, 401);
    let (status, answer) = say(&gateway_url, "lead", Some(&lead_token), "/approve", "g1");
    assert_eq!(status, 403, "a token for messages only: {answer}");
    let (status, answer) = say(
        &gateway_url,
        "lead",
        Some(&lead_token),
        "How much budget is left?",
        "g1",
    );
    assert_eq!(status, 200, "{answer}");
    assert_eq!(answer["said"], "Budget checked.", "{answer}");
    hosting.join().unwrap().unwrap();
    // A host without the agent's key is refused.
    let impostor = Profile {
        wallet: "mallory".into(),
        ..lead_profile.clone()
    };
    let refused =
        sion_cli::gateway::host(ledger.clone(), &gateway_url, &[impostor], Some(1)).unwrap_err();
    assert!(refused.to_string().contains("refused"), "{refused}");
    // Nor may another agent, with its own key, take the profile's name.
    let hijacker = Profile {
        name: "lead".into(),
        ..payer_profile.clone()
    };
    let refused =
        sion_cli::gateway::host(ledger.clone(), &gateway_url, &[hijacker], Some(1)).unwrap_err();
    assert!(refused.to_string().contains("refused"), "{refused}");
    let status: Value = serde_json::from_slice(
        &client()
            .get(&format!("{gateway_url}/v1/status"), 1 << 20)
            .unwrap()
            .body,
    )
    .unwrap();
    assert_eq!(status["routes"]["lead"]["agent"], json!(lead), "{status}");
    // Once the agent is suspended, its messages stop at the gateway.
    session
        .must("alice", TxKind::SuspendAgent { charter: lead }, Vec::new())
        .unwrap();
    std::thread::sleep(sion_cli::gateway::STANDING_TTL + std::time::Duration::from_secs(1));
    let (status, answer) = say(
        &gateway_url,
        "lead",
        Some(&lead_token),
        "How much budget is left?",
        "g2",
    );
    assert_eq!(status, 200, "{answer}");
    assert!(
        answer["said"].as_str().unwrap().contains("suspended"),
        "{answer}"
    );
    session
        .must("alice", TxKind::ResumeAgent { charter: lead }, Vec::new())
        .unwrap();

    // Slack, against a stand-in: the first poll finds where to start, the
    // second answers what was written since; bots are skipped, and only an
    // approver decides.
    let polls = Arc::new(Mutex::new(VecDeque::from(vec![
        json!({"ok": true, "messages": [{"ts": "100.000001", "user": "U1", "text": "old"}]}),
        json!({"ok": true, "messages": [
            {"ts": "100.000003", "user": "U2", "text": "/approve"},
            {"ts": "100.000002", "user": "U1", "text": "How much budget is left?"},
            {"ts": "100.000004", "bot_id": "B1", "text": "a bot"}
        ]}),
    ])));
    let slack = StandIn::start(move |_, url, _| {
        if url.ends_with("/conversations.history") {
            polls
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(json!({"ok": true, "messages": []}))
        } else {
            json!({"ok": true})
        }
    });
    std::env::set_var("SLACK_BOT_TOKEN", "xoxb-test");
    std::env::set_var("SLACK_API_BASE", &slack.base);
    let messenger = sion_cli::messengers::from_env("slack", &["C1".to_string()]).unwrap();
    for _ in 0..2 {
        sion_cli::messengers::bridge(
            ledger.clone(),
            &lead_profile,
            messenger.as_ref(),
            &["C1".to_string()],
            &["U9".to_string()],
            true,
        )
        .unwrap();
    }
    let posted = slack.sent("/chat.postMessage");
    assert_eq!(posted.len(), 2, "{posted:?}");
    assert_eq!(posted[0]["text"], "Budget checked.", "{posted:?}");
    assert_eq!(
        posted[1]["text"], "Only U9 may approve or decline here.",
        "{posted:?}"
    );
    let history = slack.sent("/conversations.history");
    assert_eq!(history[1]["oldest"], "100.000001", "{history:?}");

    // Discord: the same, by message ids.
    let discord = StandIn::start(|method, url, _| {
        if method == "GET" && url.contains("limit=1") && !url.contains("after=") {
            json!([{"id": "10", "author": {"id": "A1"}, "content": "old"}])
        } else if method == "GET" {
            json!([{"id": "11", "author": {"id": "A1"}, "content": "How much budget is left?"},
                   {"id": "12", "author": {"id": "B", "bot": true}, "content": "a bot"}])
        } else {
            json!({"id": "13"})
        }
    });
    std::env::set_var("DISCORD_BOT_TOKEN", "discord-test");
    std::env::set_var("DISCORD_API_BASE", &discord.base);
    let messenger = sion_cli::messengers::from_env("discord", &["900".to_string()]).unwrap();
    let (first, cursor) = messenger.receive("", 0).unwrap();
    assert!(first.is_empty());
    let (next, cursor) = messenger.receive(&cursor, 0).unwrap();
    assert_eq!(next.len(), 1, "the bot's message is skipped");
    assert_eq!(
        (next[0].chat.as_str(), next[0].from.as_str()),
        ("900", "A1")
    );
    assert_eq!(cursor, "900=12");
    messenger.send("900", "Budget checked.").unwrap();
    assert_eq!(
        discord.sent("/channels/900/messages")[0]["content"],
        "Budget checked."
    );

    // Matrix: whoami, a first sync that only finds where to start, then
    // the room's messages, the account's own skipped.
    let matrix = StandIn::start(|_, url, _| {
        if url.contains("/account/whoami") {
            json!({"user_id": "@bot:example.org"})
        } else if url.contains("/sync") && !url.contains("since=") {
            json!({"next_batch": "s1"})
        } else if url.contains("/sync") {
            json!({"next_batch": "s2", "rooms": {"join": {"!r:example.org": {"timeline": {"events": [
                {"type": "m.room.message", "sender": "@ann:example.org", "content": {"msgtype": "m.text", "body": "How much budget is left?"}},
                {"type": "m.room.message", "sender": "@bot:example.org", "content": {"msgtype": "m.text", "body": "mine"}}
            ]}}}}})
        } else {
            json!({"event_id": "$e"})
        }
    });
    std::env::set_var("MATRIX_HOMESERVER", &matrix.base);
    std::env::set_var("MATRIX_ACCESS_TOKEN", "matrix-test");
    let messenger = sion_cli::messengers::from_env("matrix", &[]).unwrap();
    let (first, cursor) = messenger.receive("", 0).unwrap();
    assert!(first.is_empty() && cursor == "s1");
    let (next, cursor) = messenger.receive(&cursor, 0).unwrap();
    assert_eq!(cursor, "s2");
    assert_eq!(next.len(), 1);
    assert_eq!(
        (next[0].chat.as_str(), next[0].from.as_str()),
        ("!r:example.org", "@ann:example.org")
    );
    messenger.send("!r:example.org", "Budget checked.").unwrap();
    assert_eq!(
        matrix.sent("/rooms/%21r%3Aexample.org/send/m.room.message/")[0]["body"],
        "Budget checked."
    );

    // Signal, through signal-cli-rest-api: direct messages only.
    let signal = StandIn::start(|method, _, _| {
        if method == "GET" {
            json!([
                {"envelope": {"sourceNumber": "+15550001111", "dataMessage": {"message": "How much budget is left?"}}},
                {"envelope": {"sourceNumber": "+15550002222", "dataMessage": {"message": "in a group", "groupInfo": {"groupId": "g"}}}}
            ])
        } else {
            json!({"timestamp": "1"})
        }
    });
    std::env::set_var("SIGNAL_API_BASE", &signal.base);
    std::env::set_var("SIGNAL_NUMBER", "+15559990000");
    let messenger = sion_cli::messengers::from_env("signal", &[]).unwrap();
    let (messages, _) = messenger.receive("", 0).unwrap();
    assert_eq!(messages.len(), 1, "group messages are skipped");
    assert_eq!(messages[0].chat, "+15550001111");
    messenger.send("+15550001111", "Budget checked.").unwrap();
    let sent = signal.sent("/v2/send");
    assert_eq!(sent[0]["recipients"], json!(["+15550001111"]));
    assert_eq!(sent[0]["number"], "+15559990000");
    assert!(sion_cli::messengers::from_env("carrier-pigeon", &[]).is_err());
    std::fs::remove_dir_all(&home).unwrap();
}
