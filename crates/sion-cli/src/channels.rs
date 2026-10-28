//! Channels: how people reach an agent profile from outside the command
//! line, through an HTTP endpoint (for webhooks and other programs) or
//! Telegram.
//!
//! Each conversation is a session, continued message by message. A session
//! that waits for a person's approval is answered from the same channel:
//! `/approve`, or `/decline <why>`. `/new` starts over, and `/status` says
//! where the session stands. The HTTP endpoint needs a bearer token; a
//! Telegram bot answers only the chats it is told to.

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
/// requests, if given): `POST /v1/profiles/<name>/messages` with
/// `{"text": …, "conversation": …}`, and `GET /v1/sessions/<id>`. Every
/// request needs `Authorization: Bearer <token>`. Requests are handled one
/// at a time.
pub fn serve(
    ledger: SharedLedger,
    listen: &str,
    token: &str,
    requests: Option<usize>,
) -> Result<(), CliError> {
    if token.len() < 16 {
        return Err(CliError::Usage(
            "the channel's token has at least 16 characters".into(),
        ));
    }
    let server = tiny_http::Server::http(listen)
        .map_err(|e| CliError::Usage(format!("cannot listen on {listen}: {e}")))?;
    println!("agent channel on http://{listen} (POST /v1/profiles/<name>/messages)");
    for (index, mut request) in server.incoming_requests().enumerate() {
        let authorized = request
            .headers()
            .iter()
            .find(|h| h.field.equiv("Authorization"))
            .and_then(|h| h.value.as_str().strip_prefix("Bearer ").map(str::to_string))
            .is_some_and(|given| same_token(&given, token));
        let path = request
            .url()
            .split('?')
            .next()
            .unwrap_or_default()
            .to_string();
        let method = request.method().as_str().to_string();
        if !authorized {
            respond(request, 401, &json!({"error": "a bearer token is needed"}));
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
    /// The message's text.
    pub text: String,
}

/// A Telegram bot, through the Bot API.
pub struct Telegram {
    client: sion_http::Client,
    base: String,
    token: String,
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
        })
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

    /// The text messages after `offset`, waiting up to `timeout` seconds
    /// for one.
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
                        Some(Update {
                            id: u["update_id"].as_i64()?,
                            chat: u["message"]["chat"]["id"].as_i64()?,
                            text: u["message"]["text"].as_str()?.to_string(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default())
    }
}

/// Answers the chats in `allow` with `profile`, by long polling, until
/// stopped; with `once`, one batch of messages. Returns what to print.
pub fn telegram(
    ledger: SharedLedger,
    profile: &Profile,
    allow: &[i64],
    once: bool,
) -> Result<(), CliError> {
    if allow.is_empty() {
        return Err(CliError::Usage(
            "name the chats the bot answers: --allow <chat id>,…".into(),
        ));
    }
    let bot = Telegram::from_env().map_err(CliError::Usage)?;
    let offset_key = format!("telegram-offset:{}", profile.name);
    loop {
        let offset: i64 = conversations()?
            .get(&offset_key)
            .and_then(|o| o.parse().ok())
            .unwrap_or(0);
        let updates = bot
            .updates(offset, if once { 0 } else { 50 })
            .map_err(CliError::Connection)?;
        for update in updates {
            remember(&offset_key, Some(&(update.id + 1).to_string()))?;
            if !allow.contains(&update.chat) {
                println!("chat {}: not allowed, ignored", update.chat);
                continue;
            }
            println!("chat {}: {}", update.chat, update.text);
            let key = format!("telegram:{}:{}", profile.name, update.chat);
            let reply = match converse(ledger.clone(), profile, &key, &update.text) {
                Ok(answer) => {
                    for line in &answer.lines {
                        println!("  {line}");
                    }
                    answer.reply
                }
                Err(e) => format!("That did not work: {e}"),
            };
            println!("to chat {}: {reply}", update.chat);
            bot.send(update.chat, &reply)
                .map_err(CliError::Connection)?;
        }
        if once {
            return Ok(());
        }
    }
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
