//! Messengers: the chat services a profile answers, behind one interface.
//!
//! Each [`Messenger`] receives text messages after a cursor and sends text
//! back, through the service's own HTTP API, with credentials from the
//! environment. [`bridge`] answers the chats it is told to with a profile,
//! one message at a time, and keeps each messenger's cursor so that a
//! restarted bridge goes on where it stopped.
//!
//! | Messenger | Receives by | Credentials | Chats are |
//! |---|---|---|---|
//! | Telegram | long polling `getUpdates` | `TELEGRAM_BOT_TOKEN` | chat ids |
//! | Slack | polling `conversations.history` | `SLACK_BOT_TOKEN` | channel ids |
//! | Discord | polling a channel's messages | `DISCORD_BOT_TOKEN` | channel ids |
//! | Matrix | long polling `/sync` | `MATRIX_ACCESS_TOKEN`, `MATRIX_HOMESERVER` | room ids |
//! | Signal | polling signal-cli-rest-api's `receive` | `SIGNAL_API_BASE`, `SIGNAL_NUMBER` | phone numbers |
//!
//! Each `*_API_BASE` variable points a messenger at another server (a
//! stand-in, in the tests). Bots are not answered: Telegram does not show a
//! bot other bots' messages, Slack and Discord mark them, and on Matrix the
//! bridge skips its own. On Signal, only direct messages are answered. The
//! first time a bridge polls Slack, Discord or Matrix it starts from the
//! latest message instead of answering the whole history.
//!
//! Who may approve or decline a waiting call is the bridge's `approvers`,
//! by the ids the messenger gives people: without them, anyone who can
//! write in an allowed chat decides, as the messenger identifies them.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{json, Value};

use crate::error::CliError;
use crate::profiles::Profile;
use crate::runtime::SharedLedger;

/// A message from a chat.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Incoming {
    /// The chat it came from (a chat, channel or room id, or a number).
    pub chat: String,
    /// Who wrote it, as the messenger names them.
    pub from: String,
    /// What it says (a voice note's caption, if it has one).
    pub text: String,
    /// The voice note it is, if it is one.
    pub voice: Option<Note>,
}

/// A voice note a messenger received: its handle for it, and what it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Note {
    /// The messenger's handle (a file id, an address).
    pub id: String,
    /// Its media type, where the messenger says.
    pub media_type: Option<String>,
}

/// A chat service a profile answers.
pub trait Messenger: std::fmt::Debug {
    /// Its name: `telegram`, `slack`, `discord`, `matrix` or `signal`.
    fn name(&self) -> &'static str;
    /// What its chats are called in what the bridge prints: `chat`,
    /// `channel`, `room` or `number`.
    fn chat_word(&self) -> &'static str {
        "chat"
    }
    /// Where the bridge keeps its cursor for `profile`.
    fn cursor_key(&self, profile: &str) -> String {
        format!("{}-cursor:{profile}", self.name())
    }
    /// The messages after `cursor` (empty the first time), waiting up to
    /// `wait` seconds for one where the service can, and the next cursor.
    fn receive(&self, cursor: &str, wait: u64) -> Result<(Vec<Incoming>, String), String>;
    /// Sends `text` to `chat`.
    fn send(&self, chat: &str, text: &str) -> Result<(), String>;
    /// Whether [`Messenger::receive`] waits for messages itself; if not, the
    /// bridge pauses between polls.
    fn long_polls(&self) -> bool;
    /// Whether it receives messages from chats the bridge does not answer
    /// yet, which pairing needs; one polled chat by chat does not.
    fn sees_strangers(&self) -> bool {
        true
    }
    /// The audio of a voice note it received.
    fn fetch_voice(&self, _note: &Note) -> Result<crate::voice::Audio, String> {
        Err(format!("{} gives no voice notes", self.name()))
    }
    /// Sends `audio` to `chat` as a voice note, with `caption` where it
    /// takes one: `Ok(false)` if it sends no voice notes.
    fn send_voice(
        &self,
        _chat: &str,
        _audio: &crate::voice::Audio,
        _caption: &str,
    ) -> Result<bool, String> {
        Ok(false)
    }
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

fn client(timeout: u64) -> Result<sion_http::Client, String> {
    sion_http::Client::new(sion_http::Options {
        timeout: Duration::from_secs(timeout),
        // The person names the service's address.
        allow_private: true,
        ..sion_http::Options::default()
    })
    .map_err(|e| e.to_string())
}

fn json_of(body: &[u8], service: &str) -> Result<Value, String> {
    serde_json::from_slice(body)
        .map_err(|e| format!("{service} answered with something other than JSON: {e}"))
}

fn cut(text: &str, chars: usize) -> String {
    let text: String = text.chars().take(chars).collect();
    if text.is_empty() {
        "(nothing)".into()
    } else {
        text
    }
}

/// `text` with a secret replaced, for error messages.
fn hide(text: String, secret: &str) -> String {
    if secret.is_empty() {
        text
    } else {
        text.replace(secret, "<token>")
    }
}

/// Percent-encodes a path segment (a Matrix room id, say).
fn segment(text: &str) -> String {
    let mut out = String::new();
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// A cursor of one position per chat: `chat=position,…`.
fn read_positions(cursor: &str) -> BTreeMap<String, String> {
    cursor
        .split(',')
        .filter_map(|p| p.split_once('='))
        .map(|(c, p)| (c.to_string(), p.to_string()))
        .collect()
}

fn write_positions(positions: &BTreeMap<String, String>) -> String {
    positions
        .iter()
        .map(|(c, p)| format!("{c}={p}"))
        .collect::<Vec<_>>()
        .join(",")
}

/// Slack, through the Web API: polls `conversations.history` of each
/// channel it answers, and posts with `chat.postMessage`.
pub struct Slack {
    client: sion_http::Client,
    base: String,
    token: String,
    channels: Vec<String>,
}

impl std::fmt::Debug for Slack {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Slack({})", self.base)
    }
}

impl Slack {
    /// The bot whose token is in `SLACK_BOT_TOKEN`, at `SLACK_API_BASE`
    /// (default `https://slack.com/api`), polling `channels`.
    pub fn from_env(channels: &[String]) -> Result<Slack, String> {
        Ok(Slack {
            client: client(30)?,
            base: env("SLACK_API_BASE")
                .unwrap_or_else(|| "https://slack.com/api".into())
                .trim_end_matches('/')
                .to_string(),
            token: env("SLACK_BOT_TOKEN").ok_or("set SLACK_BOT_TOKEN")?,
            channels: channels.to_vec(),
        })
    }

    fn call(&self, method: &str, body: &Value) -> Result<Value, String> {
        let auth = format!("Bearer {}", self.token);
        let fetched = self
            .client
            .post_json(
                &format!("{}/{method}", self.base),
                &[("Authorization", &auth)],
                body.to_string().as_bytes(),
                4 << 20,
            )
            .map_err(|e| hide(e.to_string(), &self.token))?;
        let value = json_of(&fetched.body, "Slack")?;
        if value["ok"] != true {
            return Err(format!(
                "Slack refused {method}: {}",
                value["error"].as_str().unwrap_or("no reason given")
            ));
        }
        Ok(value)
    }
}

impl Messenger for Slack {
    fn name(&self) -> &'static str {
        "slack"
    }

    fn chat_word(&self) -> &'static str {
        "channel"
    }

    fn receive(&self, cursor: &str, _wait: u64) -> Result<(Vec<Incoming>, String), String> {
        let mut positions = read_positions(cursor);
        let mut out = Vec::new();
        for channel in &self.channels {
            let oldest = positions.get(channel).cloned();
            let mut body = json!({"channel": channel, "limit": 100});
            match &oldest {
                Some(ts) => body["oldest"] = json!(ts),
                // The first poll starts from the latest message.
                None => body["limit"] = json!(1),
            }
            let value = self.call("conversations.history", &body)?;
            let mut messages: Vec<&Value> = value["messages"]
                .as_array()
                .map(|m| m.iter().collect())
                .unwrap_or_default();
            // Newest first in Slack's answer; oldest first here.
            messages.sort_by(|a, b| {
                let ts = |m: &Value| {
                    m["ts"]
                        .as_str()
                        .unwrap_or("0")
                        .parse::<f64>()
                        .unwrap_or(0.0)
                };
                ts(a).total_cmp(&ts(b))
            });
            let mut latest = oldest.clone();
            for message in messages {
                let Some(ts) = message["ts"].as_str() else {
                    continue;
                };
                if oldest.as_deref() == Some(ts) {
                    continue;
                }
                latest = Some(ts.to_string());
                let from_bot = !message["bot_id"].is_null() || !message["subtype"].is_null();
                if oldest.is_none() || from_bot {
                    continue;
                }
                if let Some(text) = message["text"].as_str() {
                    out.push(Incoming {
                        chat: channel.clone(),
                        from: message["user"].as_str().unwrap_or_default().to_string(),
                        text: text.to_string(),
                        voice: None,
                    });
                }
            }
            positions.insert(channel.clone(), latest.unwrap_or_else(|| "0".to_string()));
        }
        Ok((out, write_positions(&positions)))
    }

    fn send(&self, chat: &str, text: &str) -> Result<(), String> {
        self.call(
            "chat.postMessage",
            &json!({"channel": chat, "text": cut(text, 3_000)}),
        )
        .map(|_| ())
    }

    fn long_polls(&self) -> bool {
        false
    }

    fn sees_strangers(&self) -> bool {
        false
    }
}

/// Discord, through its HTTP API: polls each channel it answers for
/// messages after the last one seen, and posts to the channel. The bot
/// needs the message-content intent to read what people write.
pub struct Discord {
    client: sion_http::Client,
    base: String,
    token: String,
    channels: Vec<String>,
}

impl std::fmt::Debug for Discord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Discord({})", self.base)
    }
}

impl Discord {
    /// The bot whose token is in `DISCORD_BOT_TOKEN`, at `DISCORD_API_BASE`
    /// (default `https://discord.com/api/v10`), polling `channels`.
    pub fn from_env(channels: &[String]) -> Result<Discord, String> {
        Ok(Discord {
            client: client(30)?,
            base: env("DISCORD_API_BASE")
                .unwrap_or_else(|| "https://discord.com/api/v10".into())
                .trim_end_matches('/')
                .to_string(),
            token: env("DISCORD_BOT_TOKEN").ok_or("set DISCORD_BOT_TOKEN")?,
            channels: channels.to_vec(),
        })
    }

    fn call(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Value, String> {
        let auth = format!("Bot {}", self.token);
        let bytes = body.map(|b| b.to_string().into_bytes());
        let mut headers = vec![("Authorization", auth.as_str())];
        if bytes.is_some() {
            headers.push(("Content-Type", "application/json"));
        }
        let fetched = self
            .client
            .request(
                method,
                &format!("{}{path}", self.base),
                &headers,
                bytes.as_deref(),
                4 << 20,
            )
            .map_err(|e| hide(e.to_string(), &self.token))?;
        json_of(&fetched.body, "Discord")
    }
}

impl Messenger for Discord {
    fn name(&self) -> &'static str {
        "discord"
    }

    fn chat_word(&self) -> &'static str {
        "channel"
    }

    fn receive(&self, cursor: &str, _wait: u64) -> Result<(Vec<Incoming>, String), String> {
        let mut positions = read_positions(cursor);
        let mut out = Vec::new();
        for channel in &self.channels {
            let after = positions.get(channel).cloned();
            let path = match &after {
                Some(id) => format!(
                    "/channels/{}/messages?after={id}&limit=50",
                    segment(channel)
                ),
                None => format!("/channels/{}/messages?limit=1", segment(channel)),
            };
            let value = self.call("GET", &path, None)?;
            let mut messages: Vec<&Value> = value
                .as_array()
                .map(|m| m.iter().collect())
                .unwrap_or_default();
            let id = |m: &Value| {
                m["id"]
                    .as_str()
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or(0)
            };
            messages.sort_by_key(|m| id(m));
            let mut latest = after.clone();
            for message in messages {
                latest = Some(id(message).to_string());
                if after.is_none() || message["author"]["bot"] == true {
                    continue;
                }
                let text = message["content"].as_str().unwrap_or_default();
                // A voice message is an audio attachment.
                let voice = message["attachments"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|a| {
                        a["content_type"]
                            .as_str()
                            .is_some_and(|t| t.starts_with("audio/"))
                    })
                    .and_then(|a| {
                        Some(Note {
                            id: a["url"].as_str()?.to_string(),
                            media_type: a["content_type"].as_str().map(str::to_string),
                        })
                    });
                if !text.is_empty() || voice.is_some() {
                    out.push(Incoming {
                        chat: channel.clone(),
                        from: message["author"]["id"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string(),
                        text: text.to_string(),
                        voice,
                    });
                }
            }
            positions.insert(channel.clone(), latest.unwrap_or_else(|| "0".to_string()));
        }
        Ok((out, write_positions(&positions)))
    }

    fn send(&self, chat: &str, text: &str) -> Result<(), String> {
        self.call(
            "POST",
            &format!("/channels/{}/messages", segment(chat)),
            Some(&json!({"content": cut(text, 2_000)})),
        )
        .map(|_| ())
    }

    fn long_polls(&self) -> bool {
        false
    }

    fn sees_strangers(&self) -> bool {
        false
    }

    fn fetch_voice(&self, note: &Note) -> Result<crate::voice::Audio, String> {
        // An attachment's address is Discord's own, signed: no token goes
        // with it.
        let fetched = self
            .client
            .get(&note.id, crate::voice::MAX_HEARD as u64)
            .map_err(|e| format!("the voice message could not be fetched: {e}"))?;
        let media_type = note
            .media_type
            .clone()
            .unwrap_or_else(|| crate::voice::Audio::sniff(&fetched.body).to_string());
        Ok(crate::voice::Audio::new(&media_type, fetched.body))
    }
}

/// Matrix, through the client-server API: long polls `/sync`, and sends
/// `m.text` messages to rooms. With pairing, it joins the rooms it is
/// invited to (someone who writes to it first invites it to a room).
pub struct Matrix {
    client: sion_http::Client,
    base: String,
    token: String,
    me: String,
    join_invites: bool,
}

impl std::fmt::Debug for Matrix {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Matrix({}, {})", self.base, self.me)
    }
}

impl Matrix {
    /// The account whose access token is in `MATRIX_ACCESS_TOKEN`, on the
    /// homeserver at `MATRIX_HOMESERVER`.
    pub fn from_env() -> Result<Matrix, String> {
        let mut matrix = Matrix {
            client: client(90)?,
            base: env("MATRIX_HOMESERVER")
                .ok_or("set MATRIX_HOMESERVER (https://…)")?
                .trim_end_matches('/')
                .to_string(),
            token: env("MATRIX_ACCESS_TOKEN").ok_or("set MATRIX_ACCESS_TOKEN")?,
            me: String::new(),
            join_invites: false,
        };
        let whoami = matrix.call("GET", "/_matrix/client/v3/account/whoami", None)?;
        matrix.me = whoami["user_id"]
            .as_str()
            .ok_or("the homeserver did not say who the token is for")?
            .to_string();
        Ok(matrix)
    }

    /// The same account, joining the rooms it is invited to.
    pub fn joining_invites(self) -> Matrix {
        Matrix {
            join_invites: true,
            ..self
        }
    }

    fn call(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Value, String> {
        let auth = format!("Bearer {}", self.token);
        let bytes = body.map(|b| b.to_string().into_bytes());
        let mut headers = vec![("Authorization", auth.as_str())];
        if bytes.is_some() {
            headers.push(("Content-Type", "application/json"));
        }
        let fetched = self
            .client
            .request(
                method,
                &format!("{}{path}", self.base),
                &headers,
                bytes.as_deref(),
                16 << 20,
            )
            .map_err(|e| hide(e.to_string(), &self.token))?;
        json_of(&fetched.body, "the homeserver")
    }
}

impl Messenger for Matrix {
    fn name(&self) -> &'static str {
        "matrix"
    }

    fn chat_word(&self) -> &'static str {
        "room"
    }

    fn receive(&self, cursor: &str, wait: u64) -> Result<(Vec<Incoming>, String), String> {
        let path = if cursor.is_empty() {
            // The first sync only finds where to start.
            "/_matrix/client/v3/sync?timeout=0".to_string()
        } else {
            format!(
                "/_matrix/client/v3/sync?timeout={}&since={}",
                wait * 1000,
                segment(cursor)
            )
        };
        let value = self.call("GET", &path, None)?;
        let next = value["next_batch"]
            .as_str()
            .ok_or("the homeserver gave no next_batch")?
            .to_string();
        if self.join_invites {
            for room in value["rooms"]["invite"]
                .as_object()
                .into_iter()
                .flat_map(|rooms| rooms.keys())
            {
                // What is said there next is a stranger's, given a code.
                match self.call(
                    "POST",
                    &format!("/_matrix/client/v3/join/{}", segment(room)),
                    Some(&json!({})),
                ) {
                    Ok(_) => println!("room {room}: invited; joined"),
                    Err(why) => println!("room {room}: invited; not joined: {why}"),
                }
            }
        }
        let mut out = Vec::new();
        if !cursor.is_empty() {
            if let Some(rooms) = value["rooms"]["join"].as_object() {
                for (room, joined) in rooms {
                    for event in joined["timeline"]["events"]
                        .as_array()
                        .into_iter()
                        .flatten()
                    {
                        let sender = event["sender"].as_str().unwrap_or_default();
                        if event["type"] != "m.room.message"
                            || event["content"]["msgtype"] != "m.text"
                            || sender == self.me
                        {
                            continue;
                        }
                        if let Some(text) = event["content"]["body"].as_str() {
                            out.push(Incoming {
                                chat: room.clone(),
                                from: sender.to_string(),
                                text: text.to_string(),
                                voice: None,
                            });
                        }
                    }
                }
            }
        }
        Ok((out, next))
    }

    fn send(&self, chat: &str, text: &str) -> Result<(), String> {
        let txn = format!(
            "sion-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        self.call(
            "PUT",
            &format!(
                "/_matrix/client/v3/rooms/{}/send/m.room.message/{txn}",
                segment(chat)
            ),
            Some(&json!({"msgtype": "m.text", "body": cut(text, 16_000)})),
        )
        .map(|_| ())
    }

    fn long_polls(&self) -> bool {
        true
    }
}

/// Signal, through signal-cli-rest-api (a local service holding the
/// number's keys): polls `receive` for direct messages, and sends with
/// `/v2/send`.
pub struct Signal {
    client: sion_http::Client,
    base: String,
    number: String,
}

impl std::fmt::Debug for Signal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Signal({}, {})", self.base, self.number)
    }
}

impl Signal {
    /// The number in `SIGNAL_NUMBER`, served by signal-cli-rest-api at
    /// `SIGNAL_API_BASE`.
    pub fn from_env() -> Result<Signal, String> {
        Ok(Signal {
            client: client(60)?,
            base: env("SIGNAL_API_BASE")
                .ok_or("set SIGNAL_API_BASE (signal-cli-rest-api, e.g. http://127.0.0.1:8080)")?
                .trim_end_matches('/')
                .to_string(),
            number: env("SIGNAL_NUMBER").ok_or("set SIGNAL_NUMBER")?,
        })
    }
}

impl Messenger for Signal {
    fn name(&self) -> &'static str {
        "signal"
    }

    fn chat_word(&self) -> &'static str {
        "number"
    }

    fn receive(&self, _cursor: &str, _wait: u64) -> Result<(Vec<Incoming>, String), String> {
        let fetched = self
            .client
            .get(
                &format!("{}/v1/receive/{}", self.base, segment(&self.number)),
                4 << 20,
            )
            .map_err(|e| e.to_string())?;
        let value = json_of(&fetched.body, "signal-cli-rest-api")?;
        let mut out = Vec::new();
        for item in value.as_array().into_iter().flatten() {
            let envelope = &item["envelope"];
            let message = &envelope["dataMessage"];
            // Direct messages only: a group's members all speak for you.
            if !message["groupInfo"].is_null() {
                continue;
            }
            let from = envelope["sourceNumber"]
                .as_str()
                .or(envelope["source"].as_str())
                .unwrap_or_default();
            if let (Some(text), false) = (message["message"].as_str(), from.is_empty()) {
                out.push(Incoming {
                    chat: from.to_string(),
                    from: from.to_string(),
                    text: text.to_string(),
                    voice: None,
                });
            }
        }
        // Received messages leave the service's queue: no cursor to keep.
        Ok((out, String::new()))
    }

    fn send(&self, chat: &str, text: &str) -> Result<(), String> {
        self.client
            .post_json(
                &format!("{}/v2/send", self.base),
                &[],
                json!({"message": cut(text, 4_000), "number": self.number, "recipients": [chat]})
                    .to_string()
                    .as_bytes(),
                1 << 20,
            )
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    fn long_polls(&self) -> bool {
        false
    }
}

/// Telegram, through the Bot API ([`crate::channels::Telegram`]): long
/// polls `getUpdates`, keeping the offset as its cursor.
impl Messenger for crate::channels::Telegram {
    fn name(&self) -> &'static str {
        "telegram"
    }

    fn cursor_key(&self, profile: &str) -> String {
        // Where `sion agent telegram` kept it before the bridge did.
        format!("telegram-offset:{profile}")
    }

    fn receive(&self, cursor: &str, wait: u64) -> Result<(Vec<Incoming>, String), String> {
        let offset: i64 = cursor.parse().unwrap_or(0);
        let updates = self.updates(offset, wait)?;
        let next = updates.iter().map(|u| u.id + 1).max().unwrap_or(offset);
        let messages = updates
            .into_iter()
            .map(|u| Incoming {
                chat: u.chat.to_string(),
                from: u.from.map(|f| f.to_string()).unwrap_or_default(),
                text: u.text,
                voice: u.voice.map(|(id, media_type)| Note {
                    id,
                    media_type: Some(media_type),
                }),
            })
            .collect();
        Ok((messages, next.to_string()))
    }

    fn send(&self, chat: &str, text: &str) -> Result<(), String> {
        let chat: i64 = chat
            .parse()
            .map_err(|_| format!("{chat} is not a Telegram chat id"))?;
        crate::channels::Telegram::send(self, chat, text)
    }

    fn long_polls(&self) -> bool {
        true
    }

    fn fetch_voice(&self, note: &Note) -> Result<crate::voice::Audio, String> {
        self.file(&note.id, note.media_type.as_deref())
    }

    fn send_voice(
        &self,
        chat: &str,
        audio: &crate::voice::Audio,
        caption: &str,
    ) -> Result<bool, String> {
        let chat: i64 = chat
            .parse()
            .map_err(|_| format!("{chat} is not a Telegram chat id"))?;
        self.send_voice(chat, audio, caption).map(|()| true)
    }
}

/// The messenger called `name`, with credentials from the environment;
/// `chats` are the channels to poll, for the messengers that poll them.
pub fn from_env(name: &str, chats: &[String]) -> Result<Box<dyn Messenger>, String> {
    for_bridge(
        name,
        &Options {
            allow: chats.to_vec(),
            ..Options::default()
        },
    )
}

/// The messenger called `name`, for a bridge with `options`: polling the
/// chats they allow, for the messengers that poll chats, and with pairing,
/// joining the rooms it is invited to, where it must join to be written
/// to.
pub fn for_bridge(name: &str, options: &Options) -> Result<Box<dyn Messenger>, String> {
    let chats = &options.allow;
    Ok(match name {
        "telegram" => Box::new(crate::channels::Telegram::from_env()?),
        "slack" => Box::new(Slack::from_env(chats)?),
        "discord" => Box::new(Discord::from_env(chats)?),
        "matrix" if options.pairing => Box::new(Matrix::from_env()?.joining_invites()),
        "matrix" => Box::new(Matrix::from_env()?),
        "signal" => Box::new(Signal::from_env()?),
        other => {
            return Err(format!(
                "{other} is not a messenger: telegram, slack, discord, matrix or signal"
            ))
        }
    })
}

/// Seconds between polls of a messenger that does not wait for messages.
pub const POLL_PAUSE: u64 = 3;

/// `--pairing`, `--hear` and `--speak`, for a channel's command.
#[derive(Clone, Debug, Default, clap::Args)]
pub struct ChannelArgs {
    /// Give someone it does not know a pairing code, and answer them once
    /// you approve it (`sion agent pairing approve`): on Telegram, Matrix
    /// (joining the rooms it is invited to) and Signal.
    #[arg(long)]
    pub pairing: bool,
    /// Hear voice notes: openai:<model>, groq:<model>, or
    /// command:<program> (given the audio's file, it prints the text).
    #[arg(long, value_name = "PROVIDER")]
    pub hear: Option<String>,
    /// Answer voice notes aloud: openai:<model>:<voice>,
    /// elevenlabs:<voice id>[:<model>], or command:<program> (given the
    /// text, it writes Ogg Opus or MP3).
    #[arg(long, value_name = "PROVIDER", requires = "hear")]
    pub speak: Option<String>,
}

impl ChannelArgs {
    /// The bridge's options, with `allow`, `approvers` and `once`.
    pub fn options(
        self,
        allow: Vec<String>,
        approvers: Vec<String>,
        once: bool,
    ) -> Result<Options, CliError> {
        Ok(Options {
            allow,
            approvers,
            once,
            pairing: self.pairing,
            hearing: self
                .hear
                .as_deref()
                .map(crate::voice::Hearing::parse)
                .transpose()
                .map_err(CliError::Usage)?,
            speaking: self
                .speak
                .as_deref()
                .map(crate::voice::Speaking::parse)
                .transpose()
                .map_err(CliError::Usage)?,
        })
    }
}

/// How a bridge answers.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// The chats it answers.
    pub allow: Vec<String>,
    /// Who may approve or decline a waiting call (anyone in an allowed
    /// chat, if none are named).
    pub approvers: Vec<String>,
    /// One batch of messages, then stop.
    pub once: bool,
    /// Give a chat it does not know a pairing code ([`crate::pairing`]),
    /// and answer it once the operator approves the code.
    pub pairing: bool,
    /// What hears voice notes, if any are heard.
    pub hearing: Option<crate::voice::Hearing>,
    /// What speaks the replies to voice notes, if they are spoken.
    pub speaking: Option<crate::voice::Speaking>,
}

/// Answers the chats in `allow` with `profile` through `messenger`, until
/// stopped; with `once`, one batch of messages. With `approvers`, only
/// they may approve or decline a waiting call; anyone in an allowed chat
/// may otherwise talk to the profile.
pub fn bridge(
    ledger: SharedLedger,
    profile: &Profile,
    messenger: &dyn Messenger,
    allow: &[String],
    approvers: &[String],
    once: bool,
) -> Result<(), CliError> {
    bridge_with(
        ledger,
        profile,
        messenger,
        &Options {
            allow: allow.to_vec(),
            approvers: approvers.to_vec(),
            once,
            ..Options::default()
        },
    )
}

/// [`bridge`], as `options` say: with pairing for chats it does not know,
/// and voice notes heard and answered aloud.
pub fn bridge_with(
    ledger: SharedLedger,
    profile: &Profile,
    messenger: &dyn Messenger,
    options: &Options,
) -> Result<(), CliError> {
    let (allow, approvers, once) = (&options.allow, &options.approvers, options.once);
    if allow.is_empty() && !options.pairing {
        return Err(CliError::Usage(
            "name the chats the profile answers (--allow <chat>,…), or let people pair (--pairing)"
                .into(),
        ));
    }
    if options.pairing && !messenger.sees_strangers() {
        return Err(CliError::Usage(format!(
            "{} is polled here channel by channel, so no message from a chat it does not answer reaches the bridge: pairing works on Telegram, Matrix and Signal",
            messenger.name()
        )));
    }
    let cursor_key = messenger.cursor_key(&profile.name);
    let word = messenger.chat_word();
    let channel = format!("{}:{}", messenger.name(), profile.name);
    loop {
        let cursor = crate::channels::state(&cursor_key)?.unwrap_or_default();
        let (messages, next) = messenger
            .receive(&cursor, if once { 0 } else { 50 })
            .map_err(CliError::Connection)?;
        crate::channels::set_state(&cursor_key, Some(&next))?;
        let quiet = messages.is_empty();
        for message in messages {
            if !allow.contains(&message.chat) {
                let known =
                    options.pairing && crate::pairing::load()?.allows(&channel, &message.chat);
                if !known {
                    if !options.pairing {
                        println!("{word} {}: not allowed, ignored", message.chat);
                        continue;
                    }
                    match crate::pairing::stranger(
                        &channel,
                        &message.chat,
                        &message.from,
                        &message.text,
                    )? {
                        crate::pairing::Stranger::Allowed => {}
                        crate::pairing::Stranger::Code(code) => {
                            // The code goes to the sender only: the bridge
                            // does not print it.
                            println!(
                                "{word} {}: not known yet; sent a pairing code (sion agent pairing list)",
                                message.chat
                            );
                            messenger
                                .send(
                                    &message.chat,
                                    &format!(
                                        "I do not know you yet. To talk with me, ask the person who runs me to let you in with: sion agent pairing approve {}",
                                        crate::pairing::shown(&code)
                                    ),
                                )
                                .map_err(CliError::Connection)?;
                            continue;
                        }
                        crate::pairing::Stranger::Quiet => {
                            println!("{word} {}: not known yet, ignored", message.chat);
                            continue;
                        }
                    }
                }
            }
            // A voice note is heard, and goes to the agent as what its
            // sender said, marked as heard.
            let mut text = message.text.clone();
            if let Some(note) = &message.voice {
                let heard = match &options.hearing {
                    None => Err("voice notes are not heard here: write instead".to_string()),
                    Some(hearing) => messenger
                        .fetch_voice(note)
                        .and_then(|audio| hearing.transcribe(&audio)),
                };
                match heard {
                    Ok(heard) => {
                        println!("{word} {}: (voice note) {heard}", message.chat);
                        text = if text.is_empty() {
                            format!("(voice note) {heard}")
                        } else {
                            format!("{text}\n(voice note) {heard}")
                        };
                    }
                    Err(why) => {
                        println!("{word} {}: a voice note not heard: {why}", message.chat);
                        messenger
                            .send(&message.chat, &format!("I could not hear that: {why}"))
                            .map_err(CliError::Connection)?;
                        continue;
                    }
                }
            } else if message.from.is_empty() || message.from == message.chat {
                println!("{word} {}: {}", message.chat, message.text);
            } else {
                println!(
                    "{word} {} ({}): {}",
                    message.chat, message.from, message.text
                );
            }
            let message = Incoming { text, ..message };
            let key = format!("{}:{}:{}", messenger.name(), profile.name, message.chat);
            let deciding = crate::channels::is_decision(&message.text);
            let reply = if deciding && !approvers.is_empty() && !approvers.contains(&message.from) {
                format!("Only {} may approve or decline here.", approvers.join(", "))
            } else {
                match crate::channels::converse(ledger.clone(), profile, &key, &message.text) {
                    Ok(answer) => {
                        for line in &answer.lines {
                            println!("  {line}");
                        }
                        answer.reply
                    }
                    Err(e) => format!("That did not work: {e}"),
                }
            };
            // A voice note is answered aloud where the messenger sends
            // voice notes, and in text otherwise.
            let spoken = match (&message.voice, &options.speaking) {
                (Some(_), Some(speaking)) => match speaking
                    .speak(&reply)
                    .and_then(|audio| messenger.send_voice(&message.chat, &audio, &reply))
                {
                    Ok(sent) => sent,
                    Err(why) => {
                        println!("  (not spoken: {why})");
                        false
                    }
                },
                _ => false,
            };
            if spoken {
                println!("to {word} {} (voice note): {reply}", message.chat);
            } else {
                println!("to {word} {}: {reply}", message.chat);
                messenger
                    .send(&message.chat, &reply)
                    .map_err(CliError::Connection)?;
            }
        }
        if once {
            return Ok(());
        }
        if quiet && !messenger.long_polls() {
            std::thread::sleep(Duration::from_secs(POLL_PAUSE));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_round_trip_and_segments_are_encoded() {
        let cursor = "C1=1700000000.000100,C2=0";
        assert_eq!(write_positions(&read_positions(cursor)), cursor);
        assert!(read_positions("").is_empty());
        assert_eq!(segment("!room:example.org"), "%21room%3Aexample.org");
        assert_eq!(segment("+15550001111"), "%2B15550001111");
    }

    #[test]
    fn long_replies_are_cut_and_empty_ones_say_so() {
        assert_eq!(cut("", 10), "(nothing)");
        assert_eq!(cut("abcdef", 3), "abc");
        assert_eq!(hide("bot123/x failed".into(), "123"), "bot<token>/x failed");
    }
}
