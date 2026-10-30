//! Channels: how people reach an agent profile from outside the command
//! line, through an HTTP endpoint (for webhooks and other programs), a
//! messenger ([`crate::messengers`]: Telegram, Slack, Discord, Matrix,
//! Signal), or a gateway on another machine ([`crate::gateway`]).
//!
//! Each conversation is a session, continued message by message. A session
//! that waits for a person's approval is answered from the same channel:
//! `/approve`, or `/decline <why>`. `/new` starts over, and `/status` says
//! where the session stands. The HTTP endpoint needs a bearer token: the
//! machine's, which reaches every profile, or one of a profile's own
//! ([`add_token`]), kept only as its hash, which reaches that profile and
//! decides what it waits on only if it was made to. A messenger answers
//! only the chats it is told to, and lets only the approvers it is given
//! decide.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::PathBuf;

use serde_json::{json, Value};

use crate::error::CliError;
use crate::profiles::Profile;
use crate::runtime::{Reply, SharedLedger};
use crate::session::wallet_dir;

/// The longest message accepted or sent, in bytes.
pub const MAX_MESSAGE: usize = 16 << 10;

/// The longest text Telegram takes in one message, in characters.
const TELEGRAM_TEXT: usize = 4_000;

fn conversations_path() -> PathBuf {
    wallet_dir()
        .parent()
        .map(|home| home.join("agents").join("conversations.json"))
        .unwrap_or_else(|| PathBuf::from(".sion/agents/conversations.json"))
}

/// The session each conversation is in, by key, and other channel state.
fn conversations() -> Result<BTreeMap<String, String>, CliError> {
    let path = conversations_path();
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text)
            .map_err(|e| CliError::Usage(format!("{}: {e}", path.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(e) => Err(CliError::Usage(format!("{}: {e}", path.display()))),
    }
}

fn remember(key: &str, value: Option<&str>) -> Result<(), CliError> {
    let mut all = conversations()?;
    match value {
        Some(value) => all.insert(key.to_string(), value.to_string()),
        None => all.remove(key),
    };
    crate::profiles::write_json(&conversations_path(), &all)
}

/// A channel's own state under `key` (a messenger's cursor, say).
pub fn state(key: &str) -> Result<Option<String>, CliError> {
    Ok(conversations()?.get(key).cloned())
}

/// Sets, or with `None` clears, a channel's own state under `key`.
pub fn set_state(key: &str, value: Option<&str>) -> Result<(), CliError> {
    remember(key, value)
}

/// A profile's channel token, kept as its hash: whoever holds the token may
/// talk to the profile through a channel, and with `approve`, decide what
/// its sessions wait on.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ChannelToken {
    /// A short id, to name it when removing it.
    pub id: String,
    /// The profile.
    pub profile: String,
    /// The profile's agent. A gateway routes the profile only to a host
    /// that proves this agent's key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<sion_core::ids::ObjectId>,
    /// The token's hash.
    pub hash: sion_core::hash::Hash32,
    /// Whether it may approve or decline.
    pub approve: bool,
    /// When it was made (seconds since the Unix epoch).
    pub created: u64,
}

fn tokens_path() -> PathBuf {
    wallet_dir()
        .parent()
        .map(|home| home.join("agents").join("channel-tokens.json"))
        .unwrap_or_else(|| PathBuf::from(".sion/agents/channel-tokens.json"))
}

fn hash_token(secret: &str) -> sion_core::hash::Hash32 {
    sion_core::hash::hash_domain("sion/agent/channel-token/v1", secret.as_bytes())
}

/// The channel tokens of every profile on this machine.
pub fn tokens() -> Result<Vec<ChannelToken>, CliError> {
    let path = tokens_path();
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text)
            .map_err(|e| CliError::Usage(format!("{}: {e}", path.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(CliError::Usage(format!("{}: {e}", path.display()))),
    }
}

/// Makes a channel token for `profile` (a profile name: on a gateway, the
/// profile lives on another machine), whose agent is `agent`. Returns its
/// record and the token, which is shown once and kept only as its hash.
pub fn add_token(
    profile: &str,
    agent: sion_core::ids::ObjectId,
    approve: bool,
) -> Result<(ChannelToken, String), CliError> {
    if !crate::profiles::valid_name(profile) {
        return Err(CliError::Usage(format!(
            "{profile:?} is not a profile name"
        )));
    }
    let mut all = tokens()?;
    // One agent per profile name, so that every token of a profile means
    // the same agent.
    if let Some(other) = all
        .iter()
        .find(|t| t.profile == profile && t.agent.is_some_and(|a| a != agent))
    {
        return Err(CliError::Usage(format!(
            "profile {profile}'s token {} names agent {}: remove its tokens first, or use another name",
            other.id,
            other.agent.map(|a| a.0.short()).unwrap_or_default()
        )));
    }
    let mut bytes = [0u8; 32];
    rand_core::RngCore::fill_bytes(&mut rand_core::OsRng, &mut bytes);
    let secret = format!("sion_ch_{}", hex::encode(bytes));
    let hash = hash_token(&secret);
    let record = ChannelToken {
        id: hash.to_hex()[..8].to_string(),
        profile: profile.to_string(),
        agent: Some(agent),
        hash,
        approve,
        created: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    };
    all.push(record.clone());
    crate::profiles::write_json(&tokens_path(), &all)?;
    Ok((record, secret))
}

/// Whether a token on this machine says that `profile` is `agent`'s: what
/// a gateway asks before it routes the profile.
pub fn names_agent(profile: &str, agent: &sion_core::ids::ObjectId) -> Result<bool, CliError> {
    Ok(tokens()?
        .iter()
        .any(|t| t.profile == profile && t.agent.as_ref() == Some(agent)))
}

/// Removes every token of `profile`. Returns how many there were.
pub fn remove_tokens(profile: &str) -> Result<usize, CliError> {
    let mut all = tokens()?;
    let before = all.len();
    all.retain(|t| t.profile != profile);
    let removed = before - all.len();
    if removed > 0 {
        crate::profiles::write_json(&tokens_path(), &all)?;
    }
    Ok(removed)
}

/// Removes `profile`'s token `id`. Returns whether there was one.
pub fn remove_token(profile: &str, id: &str) -> Result<bool, CliError> {
    let mut all = tokens()?;
    let before = all.len();
    all.retain(|t| !(t.profile == profile && t.id == id));
    let removed = all.len() < before;
    if removed {
        crate::profiles::write_json(&tokens_path(), &all)?;
    }
    Ok(removed)
}

/// What a presented token may do with a profile.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Access {
    /// Nothing.
    None,
    /// Start and continue sessions.
    Message,
    /// That, and approve or decline.
    Approve,
}

/// What `presented` may do with `profile`: the machine's `admin` token may
/// do everything with every profile; a token of the profile, what it was
/// made for.
pub fn access(profile: &str, presented: Option<&str>, admin: Option<&str>) -> Access {
    let Some(presented) = presented else {
        return Access::None;
    };
    if admin.is_some_and(|admin| same_token(presented, admin)) {
        return Access::Approve;
    }
    let hash = hash_token(presented);
    match tokens()
        .unwrap_or_default()
        .iter()
        .find(|t| t.profile == profile && t.hash == hash)
    {
        Some(t) if t.approve => Access::Approve,
        Some(_) => Access::Message,
        None => Access::None,
    }
}

/// Why a request with `presented` (or none) may not reach a profile.
pub fn refusal(presented: Option<&str>) -> &'static str {
    match presented {
        Some(_) => "this token does not reach this profile",
        None => "a bearer token is needed",
    }
}

/// Whether a message decides what a session waits on.
pub fn is_decision(text: &str) -> bool {
    matches!(
        text.split_whitespace().next(),
        Some("/approve" | "/decline")
    )
}

/// What a conversation's next message leads to: the session and the reply.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Answer {
    /// The session, if there is one.
    pub session: Option<String>,
    /// What to say back.
    pub reply: String,
    /// What happened in the session, one line each.
    pub lines: Vec<String>,
}

/// Takes the next message of conversation `key` with `profile`: a command,
/// or text that starts a session or continues it.
pub fn converse(
    ledger: SharedLedger,
    profile: &Profile,
    key: &str,
    text: &str,
) -> Result<Answer, CliError> {
    let text = text.trim();
    if text.len() > MAX_MESSAGE {
        return Err(CliError::Usage(format!(
            "a message has at most {MAX_MESSAGE} bytes"
        )));
    }
    let current = conversations()?.get(key).cloned();
    let pending = current
        .as_deref()
        .and_then(|id| crate::runtime::Saved::load(id).ok())
        .and_then(|saved| saved.state.pending.clone().map(|p| (saved, p)));
    let reply = |session: Option<String>, reply: String| Answer {
        session,
        reply,
        lines: Vec::new(),
    };
    let (command, rest) = text.split_once(' ').unwrap_or((text, ""));
    let decision = match command {
        "/new" => {
            remember(key, None)?;
            return Ok(reply(
                None,
                "Starting over: the next message begins a new session.".into(),
            ));
        }
        "/status" => {
            return Ok(match &current {
                Some(id) => {
                    let saved = crate::runtime::Saved::load(id)?;
                    reply(Some(id.clone()), crate::runtime::last_words(&saved))
                }
                None => reply(None, "No session yet: send a task.".into()),
            })
        }
        "/approve" => Some(Reply::Approve),
        "/decline" => Some(Reply::Decline(if rest.trim().is_empty() {
            "declined".to_string()
        } else {
            rest.trim().to_string()
        })),
        _ => None,
    };
    let (saved, lines) = match (decision, &pending, &current) {
        (Some(decision), Some((saved, _)), _) => {
            crate::runtime::resume(ledger, &saved.state.id, Some(decision))?
        }
        (Some(_), None, _) => return Ok(reply(current, "Nothing waits for a decision.".into())),
        (None, Some((saved, waiting)), _) => {
            return Ok(reply(
                Some(saved.state.id.clone()),
                format!(
                    "The session waits for your decision on {} {}: /approve, or /decline <why>.",
                    waiting.tool, waiting.input
                ),
            ))
        }
        (None, None, Some(id)) => {
            crate::runtime::resume(ledger, id, Some(Reply::Say(text.into())))?
        }
        (None, None, None) => profile.start(ledger, text)?,
    };
    remember(key, Some(&saved.state.id))?;
    Ok(Answer {
        session: Some(saved.state.id.clone()),
        reply: crate::runtime::last_words(&saved),
        lines,
    })
}

/// Whether two tokens are equal, in time that does not depend on where
/// they differ.
fn same_token(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
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

/// Serves the HTTP channel on `listen` until stopped (or for `requests`
/// requests, if given), with the machine's `token`: see [`serve_with`].
pub fn serve(
    ledger: SharedLedger,
    listen: &str,
    token: &str,
    requests: Option<usize>,
) -> Result<(), CliError> {
    serve_with(ledger, listen, Some(token), requests)
}

/// Serves the HTTP channel on `listen` until stopped (or for `requests`
/// requests, if given): `POST /v1/profiles/<name>/messages` with
/// `{"text": …, "conversation": …}`, and `GET /v1/sessions/<id>`. Every
/// request needs `Authorization: Bearer <token>`: the machine's `admin`
/// token, which reaches every profile and every session, or one of the
/// profile's own, which reaches only it, and decides what it waits on only
/// if it was made to. Requests are handled one at a time.
pub fn serve_with(
    ledger: SharedLedger,
    listen: &str,
    admin: Option<&str>,
    requests: Option<usize>,
) -> Result<(), CliError> {
    if admin.is_some_and(|token| token.len() < 16) {
        return Err(CliError::Usage(
            "the channel's token has at least 16 characters".into(),
        ));
    }
    let server = tiny_http::Server::http(listen)
        .map_err(|e| CliError::Usage(format!("cannot listen on {listen}: {e}")))?;
    println!("agent channel on http://{listen} (POST /v1/profiles/<name>/messages)");
    for (index, mut request) in server.incoming_requests().enumerate() {
        let presented = request
            .headers()
            .iter()
            .find(|h| h.field.equiv("Authorization"))
            .and_then(|h| h.value.as_str().strip_prefix("Bearer ").map(str::to_string));
        let path = request
            .url()
            .split('?')
            .next()
            .unwrap_or_default()
            .to_string();
        let method = request.method().as_str().to_string();
        let profile = path
            .strip_prefix("/v1/profiles/")
            .and_then(|p| p.strip_suffix("/messages"))
            .unwrap_or_default()
            .to_string();
        let allowed = if profile.is_empty() {
            // Sessions are the machine's: only its token reads them.
            if admin.is_some_and(|a| presented.as_deref().is_some_and(|p| same_token(p, a))) {
                Access::Approve
            } else {
                Access::None
            }
        } else {
            access(&profile, presented.as_deref(), admin)
        };
        if allowed == Access::None {
            let error = if profile.is_empty() && presented.is_some() {
                "only the machine's token reads sessions"
            } else {
                refusal(presented.as_deref())
            };
            respond(request, 401, &json!({ "error": error }));
        } else if let (Some(name), "POST") = (
            path.strip_prefix("/v1/profiles/")
                .and_then(|p| p.strip_suffix("/messages")),
            method.as_str(),
        ) {
            let mut body = String::new();
            let read = request
                .as_reader()
                .take(MAX_MESSAGE as u64 + 1)
                .read_to_string(&mut body);
            let parsed: Option<Value> = read.ok().and_then(|_| serde_json::from_str(&body).ok());
            match parsed {
                Some(value)
                    if allowed != Access::Approve
                        && is_decision(value["text"].as_str().unwrap_or_default()) =>
                {
                    respond(
                        request,
                        403,
                        &json!({"error": "this token may talk to the profile, not approve or decline"}),
                    )
                }
                Some(value) if value["text"].is_string() => {
                    let conversation = value["conversation"]
                        .as_str()
                        .map(str::to_string)
                        .unwrap_or_else(crate::runtime::new_id);
                    let key = format!("http:{name}:{conversation}");
                    let answer = crate::profiles::get(name).and_then(|profile| {
                        converse(
                            ledger.clone(),
                            &profile,
                            &key,
                            value["text"].as_str().unwrap_or_default(),
                        )
                    });
                    match answer {
                        Ok(answer) => respond(
                            request,
                            200,
                            &json!({
                                "conversation": conversation,
                                "session": answer.session,
                                "said": answer.reply,
                                "lines": answer.lines,
                            }),
                        ),
                        Err(e) => respond(request, 400, &json!({"error": e.to_string()})),
                    }
                }
                _ => respond(
                    request,
                    400,
                    &json!({"error": "send {\"text\": …} as JSON"}),
                ),
            }
        } else if let (Some(id), "GET") = (path.strip_prefix("/v1/sessions/"), method.as_str()) {
            match crate::runtime::Saved::load(id) {
                Ok(saved) => respond(
                    request,
                    200,
                    &json!({
                        "session": saved.state.id,
                        "said": crate::runtime::last_words(&saved),
                        "ended": saved.ended,
                    }),
                ),
                Err(e) => respond(request, 404, &json!({"error": e.to_string()})),
            }
        } else {
            respond(request, 404, &json!({"error": "not found"}));
        }
        if requests.is_some_and(|most| index + 1 >= most) {
            break;
        }
    }
    Ok(())
}

/// A message from a Telegram chat.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Update {
    /// The update's number.
    pub id: i64,
    /// The chat.
    pub chat: i64,
    /// Who wrote it (a Telegram user id), if Telegram says.
    pub from: Option<i64>,
    /// The message's text (a voice note's caption).
    pub text: String,
    /// The voice note or audio it is, if one: its file id and media type.
    pub voice: Option<(String, String)>,
    /// Whether the chat is a private one with the bot.
    pub direct: bool,
    /// Whom it names: `@usernames` (lower-cased) and user ids, the writer
    /// of the message it answers included.
    pub mentions: Vec<String>,
}

/// A Telegram bot, through the Bot API.
pub struct Telegram {
    client: sion_http::Client,
    base: String,
    token: String,
    me: std::sync::OnceLock<Vec<String>>,
}

impl std::fmt::Debug for Telegram {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never the token.
        write!(f, "Telegram({})", self.base)
    }
}

impl Telegram {
    /// The bot whose token is in `TELEGRAM_BOT_TOKEN`, at
    /// `TELEGRAM_API_BASE` (default `https://api.telegram.org`).
    pub fn from_env() -> Result<Telegram, String> {
        let token = std::env::var("TELEGRAM_BOT_TOKEN")
            .ok()
            .filter(|t| !t.is_empty())
            .ok_or("set TELEGRAM_BOT_TOKEN")?;
        let base = std::env::var("TELEGRAM_API_BASE")
            .ok()
            .filter(|b| !b.is_empty())
            .unwrap_or_else(|| "https://api.telegram.org".into());
        let client = sion_http::Client::new(sion_http::Options {
            timeout: std::time::Duration::from_secs(75),
            // The person names the API's address.
            allow_private: true,
            ..sion_http::Options::default()
        })
        .map_err(|e| e.to_string())?;
        Ok(Telegram {
            client,
            base: base.trim_end_matches('/').to_string(),
            token,
            me: std::sync::OnceLock::new(),
        })
    }

    /// The names messages give the bot: its `@username`, lower-cased, and
    /// its id (`getMe`, asked once).
    pub fn me(&self) -> Result<Vec<String>, String> {
        if let Some(me) = self.me.get() {
            return Ok(me.clone());
        }
        let bot = self.call("getMe", &json!({}))?;
        let mut names = Vec::new();
        if let Some(username) = bot["username"].as_str() {
            names.push(format!("@{}", username.to_ascii_lowercase()));
        }
        if let Some(id) = bot["id"].as_i64() {
            names.push(id.to_string());
        }
        if names.is_empty() {
            return Err("Telegram did not say who the bot is".into());
        }
        Ok(self.me.get_or_init(|| names).clone())
    }

    fn call(&self, method: &str, body: &Value) -> Result<Value, String> {
        let url = format!("{}/bot{}/{method}", self.base, self.token);
        let fetched = self
            .client
            .post_json(&url, &[], body.to_string().as_bytes(), 1 << 20)
            // The URL holds the token: never show it.
            .map_err(|e| e.to_string().replace(&self.token, "<token>"))?;
        let value: Value = serde_json::from_slice(&fetched.body)
            .map_err(|e| format!("Telegram answered with something other than JSON: {e}"))?;
        if value["ok"] != true {
            return Err(format!(
                "Telegram refused {method}: {}",
                value["description"].as_str().unwrap_or("no reason given")
            ));
        }
        Ok(value["result"].clone())
    }

    /// Sends `text` to `chat`.
    pub fn send(&self, chat: i64, text: &str) -> Result<(), String> {
        let text: String = text.chars().take(TELEGRAM_TEXT).collect();
        self.call(
            "sendMessage",
            &json!({"chat_id": chat, "text": if text.is_empty() { "(nothing)".to_string() } else { text }}),
        )
        .map(|_| ())
    }

    /// The text messages and voice notes after `offset`, waiting up to
    /// `timeout` seconds for one.
    pub fn updates(&self, offset: i64, timeout: u64) -> Result<Vec<Update>, String> {
        let result = self.call(
            "getUpdates",
            &json!({"offset": offset, "timeout": timeout, "allowed_updates": ["message"]}),
        )?;
        Ok(result
            .as_array()
            .map(|updates| {
                updates
                    .iter()
                    .filter_map(|u| {
                        let message = &u["message"];
                        let text = message["text"]
                            .as_str()
                            .or_else(|| message["caption"].as_str())
                            .unwrap_or_default()
                            .to_string();
                        // A voice note is Ogg Opus; an audio file says what
                        // it is.
                        let voice = match (&message["voice"], &message["audio"]) {
                            (voice, _) if voice.is_object() => voice["file_id"]
                                .as_str()
                                .map(|id| (id.to_string(), "audio/ogg".to_string())),
                            (_, audio) if audio.is_object() => {
                                audio["file_id"].as_str().map(|id| {
                                    (
                                        id.to_string(),
                                        audio["mime_type"]
                                            .as_str()
                                            .unwrap_or("audio/mpeg")
                                            .to_string(),
                                    )
                                })
                            }
                            _ => None,
                        };
                        if text.is_empty() && voice.is_none() {
                            return None;
                        }
                        // `@username`s in the text, people named without
                        // one, and the writer of the message answered.
                        let mut mentions: Vec<String> = text
                            .split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '@'))
                            .filter(|word| word.len() > 1 && word.starts_with('@'))
                            .map(str::to_ascii_lowercase)
                            .collect();
                        for entity in message["entities"]
                            .as_array()
                            .into_iter()
                            .chain(message["caption_entities"].as_array())
                            .flatten()
                        {
                            if let Some(id) = entity["user"]["id"].as_i64() {
                                mentions.push(id.to_string());
                            }
                        }
                        if let Some(id) = message["reply_to_message"]["from"]["id"].as_i64() {
                            mentions.push(id.to_string());
                        }
                        Some(Update {
                            id: u["update_id"].as_i64()?,
                            chat: message["chat"]["id"].as_i64()?,
                            from: message["from"]["id"].as_i64(),
                            text,
                            voice,
                            direct: message["chat"]["type"] == "private",
                            mentions,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    /// The file `file_id`, as audio of `media_type` (or what its bytes
    /// show).
    pub fn file(
        &self,
        file_id: &str,
        media_type: Option<&str>,
    ) -> Result<crate::voice::Audio, String> {
        let file = self.call("getFile", &json!({"file_id": file_id}))?;
        let path = file["file_path"]
            .as_str()
            .ok_or("Telegram gave no path for the file")?;
        let url = format!("{}/file/bot{}/{path}", self.base, self.token);
        let fetched = self
            .client
            .get(&url, crate::voice::MAX_HEARD as u64)
            .map_err(|e| e.to_string().replace(&self.token, "<token>"))?;
        let media_type = media_type
            .map(str::to_string)
            .unwrap_or_else(|| crate::voice::Audio::sniff(&fetched.body).to_string());
        Ok(crate::voice::Audio::new(&media_type, fetched.body))
    }

    /// Sends `audio` to `chat` as a voice note, with `caption` if it fits
    /// (1,024 characters); a longer one follows as text.
    pub fn send_voice(
        &self,
        chat: i64,
        audio: &crate::voice::Audio,
        caption: &str,
    ) -> Result<(), String> {
        let fits = caption.chars().count() <= 1024;
        let boundary = format!("sion-{}", crate::runtime::new_id());
        let mut body = Vec::new();
        let mut field = |name: &str, value: &str| {
            body.extend_from_slice(
                format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n")
                    .as_bytes(),
            );
        };
        field("chat_id", &chat.to_string());
        if fits && !caption.is_empty() {
            field("caption", caption);
        }
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"voice\"; filename=\"{}\"\r\nContent-Type: {}\r\n\r\n",
                audio.name, audio.media_type
            )
            .as_bytes(),
        );
        body.extend_from_slice(&audio.bytes);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        let url = format!("{}/bot{}/sendVoice", self.base, self.token);
        let content_type = format!("multipart/form-data; boundary={boundary}");
        let response = self
            .client
            .send(
                "POST",
                &url,
                &[("Content-Type", content_type.as_str())],
                Some(&body),
            )
            .map_err(|e| e.to_string().replace(&self.token, "<token>"))?;
        let answer: Value =
            serde_json::from_slice(&response.read_all(1 << 20).map_err(|e| e.to_string())?)
                .unwrap_or(Value::Null);
        if answer["ok"] != true {
            return Err(format!(
                "Telegram refused sendVoice: {}",
                answer["description"].as_str().unwrap_or("no reason given")
            ));
        }
        if !fits {
            self.send(chat, caption)?;
        }
        Ok(())
    }
}

/// Answers the chats in `allow` with `profile`, by long polling, until
/// stopped; with `once`, one batch of messages.
pub fn telegram(
    ledger: SharedLedger,
    profile: &Profile,
    allow: &[i64],
    once: bool,
) -> Result<(), CliError> {
    telegram_with(ledger, profile, allow, &[], once)
}

/// [`telegram`], where only `approvers` (Telegram user ids), if any are
/// given, may approve or decline.
pub fn telegram_with(
    ledger: SharedLedger,
    profile: &Profile,
    allow: &[i64],
    approvers: &[String],
    once: bool,
) -> Result<(), CliError> {
    if allow.is_empty() {
        return Err(CliError::Usage(
            "name the chats the bot answers: --allow <chat id>,…".into(),
        ));
    }
    let bot = Telegram::from_env().map_err(CliError::Usage)?;
    let allow: Vec<String> = allow.iter().map(i64::to_string).collect();
    crate::messengers::bridge(ledger, profile, &bot, &allow, approvers, once)
}

/// [`telegram`], as `options` say (pairing, voice notes).
pub fn telegram_options(
    ledger: SharedLedger,
    profile: &Profile,
    options: &crate::messengers::Options,
) -> Result<(), CliError> {
    let bot = Telegram::from_env().map_err(CliError::Usage)?;
    crate::messengers::bridge_with(ledger, profile, &bot, options)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_compare_whole() {
        assert!(same_token("a-long-enough-secret", "a-long-enough-secret"));
        assert!(!same_token("a-long-enough-secret", "a-long-enough-secreT"));
        assert!(!same_token("short", "shorter"));
    }
}
