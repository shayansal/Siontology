//! Messengers: the chat services a profile answers, behind one interface.
//!
//! Each [`Messenger`] receives messages (after a cursor, where the service
//! keeps them) and sends text back, through the service's own API, with
//! credentials from the environment. [`bridge`] answers the chats it is
//! told to with a profile, one message at a time, and keeps each
//! messenger's cursor so that a restarted bridge goes on where it stopped.
//!
//! | Messenger | Receives by | Credentials | Chats are |
//! |---|---|---|---|
//! | Telegram | long polling `getUpdates` | `TELEGRAM_BOT_TOKEN` | chat ids |
//! | Slack | Socket Mode, or polling `conversations.history` | `SLACK_BOT_TOKEN`, and `SLACK_APP_TOKEN` for Socket Mode | channel ids |
//! | Discord | the gateway | `DISCORD_BOT_TOKEN` | channel ids |
//! | Matrix | long polling `/sync` | `MATRIX_ACCESS_TOKEN`, `MATRIX_HOMESERVER` | room ids |
//! | Signal | polling signal-cli-rest-api's `receive` | `SIGNAL_API_BASE`, `SIGNAL_NUMBER` | phone numbers |
//! | Mattermost | its WebSocket ([`crate::mattermost`]) | `MATTERMOST_URL`, `MATTERMOST_TOKEN` | channel ids |
//! | IRC, Twitch | a connection of its own ([`crate::irc`]) | `IRC_SERVER`, `IRC_NICK`; `TWITCH_NICK`, `TWITCH_TOKEN` | `#channels`, nicks |
//! | WhatsApp, LINE, SMS | signed webhooks ([`crate::webhooks`]) | see there | numbers, ids |
//! | Microsoft Teams, Google Chat | webhooks signed with the services' keys ([`crate::teams`], [`crate::google_chat`]) | `TEAMS_APP_ID`, `TEAMS_APP_PASSWORD`; `GOOGLE_CHAT_AUDIENCE` | conversation ids, spaces |
//! | iMessage | `imsg rpc` on the Mac that runs Messages ([`crate::imessage`]) | none (`IMSG_COMMAND`) | handles, `chat…` |
//! | DingTalk | Stream mode, a WebSocket of its own ([`crate::dingtalk`]) | `DINGTALK_CLIENT_ID`, `DINGTALK_CLIENT_SECRET` | conversation ids |
//! | Feishu, Lark, Nextcloud Talk | webhooks with the app's token or the bot's signature ([`crate::feishu`], [`crate::nextcloud`]) | `FEISHU_APP_ID`, `FEISHU_APP_SECRET`, `FEISHU_VERIFICATION_TOKEN`; `NEXTCLOUD_URL`, `NEXTCLOUD_TALK_SECRET` | chat ids, conversation tokens |
//! | Email | IMAP, IDLE where the server has it ([`crate::email`]) | `EMAIL_ADDRESS`, `EMAIL_IMAP_URL`, `EMAIL_SMTP_URL`, `EMAIL_PASSWORD` | addresses |
//!
//! Each `*_API_BASE` variable points a messenger at another server (a
//! stand-in, in the tests). Bots are not answered: Telegram does not show a
//! bot other bots' messages, Slack and Discord mark them, and on Matrix the
//! bridge skips its own. On Signal, only direct messages are answered. The
//! first time a bridge polls Slack or Matrix it starts from the latest
//! message instead of answering the whole history; Discord's gateway and
//! Slack's Socket Mode pass on only what is said while the bridge listens
//! ([`crate::live`]).
//!
//! A chat the bridge does not answer is ignored, or with pairing, if it
//! is a direct one, given a code ([`crate::pairing`]); with
//! `require_mention`, a group chat it answers is answered only where a
//! message mentions the bot or answers it.
//!
//! Who may approve or decline a waiting call is the bridge's `approvers`,
//! by the ids the messenger gives people: without them, anyone who can
//! write in an allowed chat decides, as the messenger identifies them.

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde_json::{json, Value};

use crate::error::CliError;
use crate::profiles::Profile;
use crate::runtime::SharedLedger;

/// A message from a chat.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Incoming {
    /// The messenger's id of the message, where it gives one and the bridge
    /// needs it: to mark it read, or show on it that the agent is at work.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
    /// The thread it is in, where the messenger keeps threads apart and it
    /// is in one (a Slack thread, a Telegram topic, a Mattermost root
    /// post): the reply goes there ([`Messenger::reply`]).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub thread: String,
    /// The chat it came from (a chat, channel or room id, or a number).
    pub chat: String,
    /// Who wrote it, as the messenger names them.
    pub from: String,
    /// What it says (a voice note's caption, if it has one).
    pub text: String,
    /// The voice note it is, if it is one.
    pub voice: Option<Note>,
    /// Whether the chat is between its writer and the bot alone (a
    /// direct message), not a group's.
    pub direct: bool,
    /// Whom it names, as the messenger names people (ids, `@usernames`),
    /// the writer of a message it answers included.
    pub mentions: Vec<String>,
    /// The files it carries besides a voice note: pictures, documents.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<Note>,
}

/// A file a messenger received (a voice note, a picture, a document): its
/// handle for it, and what it is.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Note {
    /// The messenger's handle (a file id, an address).
    pub id: String,
    /// Its media type, where the messenger says.
    pub media_type: Option<String>,
    /// Its name, where the messenger gives one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// A chat service a profile answers.
pub trait Messenger: std::fmt::Debug + Sync {
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
    /// How long one batch (`--once`) waits for messages, in seconds: none
    /// where the messenger keeps what was said while the bridge was away,
    /// some where it only passes on what is said while it listens.
    fn once_wait(&self) -> u64 {
        0
    }
    /// The names its messages give the bot (as [`Incoming::mentions`]
    /// gives them), for telling which group messages are said to it.
    fn me(&self) -> Result<Vec<String>, String> {
        Ok(Vec::new())
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
    /// The bytes of a file it received (a picture, a document): fetched as
    /// a voice note is, where the messenger fetches those.
    fn fetch_file(&self, note: &Note) -> Result<crate::files::File, String> {
        let mut file: crate::files::File = self.fetch_voice(note)?.into();
        // What the messenger said it is; else what its bytes or its name
        // show; else what the fetch found.
        let told = note
            .media_type
            .as_deref()
            .filter(|t| !t.is_empty() && *t != "application/octet-stream");
        let named = note
            .name
            .as_deref()
            .map(crate::files::media_type_of)
            .filter(|t| *t != "application/octet-stream");
        if let Some(media_type) = told.or(crate::files::sniff_picture(&file.bytes)).or(named) {
            file.media_type = media_type.to_string();
        }
        file.name = match &note.name {
            Some(name) => name.clone(),
            None => crate::files::safe_name(None, &note.id, &file.media_type),
        };
        Ok(file)
    }
    /// Sends `file` in answer to `message`, with `caption` where it takes
    /// one: `Ok(false)` if it sends no files.
    fn send_file(
        &self,
        _message: &Incoming,
        _file: &crate::files::File,
        _caption: &str,
    ) -> Result<bool, String> {
        Ok(false)
    }
    /// Sends `text` to the chat of `message`, in its thread if it is in
    /// one.
    fn reply(&self, message: &Incoming, text: &str) -> Result<(), String> {
        self.send(&message.chat, text)
    }
    /// The most characters one message holds: a longer reply goes in
    /// parts, split where it reads best ([`crate::showing::pieces`]).
    fn room(&self) -> usize {
        4_000
    }
    /// What shows the chat of `message` that the agent is at work on it,
    /// where the messenger can (`aloud`: the reply will be a voice note).
    fn typist(&self, _message: &Incoming, _aloud: bool) -> Option<Box<dyn crate::showing::Typist>> {
        None
    }
    /// What shows the reply to `message` as it is written, where the
    /// messenger can.
    fn drafting(&self, _message: &Incoming) -> Option<crate::showing::Drafting> {
        None
    }
    /// Marks `message` as `ack` says, where the messenger can: with a
    /// reaction, or a read receipt ([`Ack`]).
    fn acknowledge(&self, _message: &Incoming, _ack: Ack) -> Result<(), String> {
        Ok(())
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
pub(crate) fn segment(text: &str) -> String {
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

/// Slack, through the Web API, posting with `chat.postMessage`. With an
/// app-level token it receives through Socket Mode, everything the app
/// is subscribed to (direct messages included); without one, it polls
/// `conversations.history` of each channel it answers.
pub struct Slack {
    pub(crate) client: sion_http::Client,
    pub(crate) base: String,
    pub(crate) token: String,
    channels: Vec<String>,
    app_token: Option<String>,
    live: OnceLock<crate::live::Live>,
    me: OnceLock<Vec<String>>,
    /// The bot's workspace (`auth.test`), which Slack's streams name.
    team: OnceLock<String>,
}

impl std::fmt::Debug for Slack {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Slack({})", self.base)
    }
}

impl Slack {
    /// The bot whose token is in `SLACK_BOT_TOKEN`, at `SLACK_API_BASE`
    /// (default `https://slack.com/api`): through Socket Mode with the
    /// app-level token in `SLACK_APP_TOKEN`, or polling `channels`.
    pub fn from_env(channels: &[String]) -> Result<Slack, String> {
        Ok(Slack {
            client: client(30)?,
            base: env("SLACK_API_BASE")
                .unwrap_or_else(|| "https://slack.com/api".into())
                .trim_end_matches('/')
                .to_string(),
            token: env("SLACK_BOT_TOKEN").ok_or("set SLACK_BOT_TOKEN")?,
            channels: channels.to_vec(),
            app_token: env("SLACK_APP_TOKEN"),
            live: OnceLock::new(),
            me: OnceLock::new(),
            team: OnceLock::new(),
        })
    }

    /// The bot's workspace, asked once.
    pub(crate) fn team(&self) -> Option<String> {
        if self.team.get().is_none() {
            let _ = self.me();
        }
        self.team.get().cloned()
    }

    /// The Socket Mode connection, started the first time it is needed.
    fn live(&self, app_token: &str) -> &crate::live::Live {
        self.live.get_or_init(|| {
            crate::live::Live::start(
                "slack",
                crate::live::slack(
                    self.client.clone(),
                    self.base.clone(),
                    app_token.to_string(),
                    self.token.clone(),
                ),
            )
        })
    }

    fn call(&self, method: &str, body: &Value) -> Result<Value, String> {
        self.call_as(method, "application/json", body.to_string().as_bytes())
    }

    /// `method` with form fields, for the methods that take no JSON.
    fn call_form(&self, method: &str, fields: &[(&str, &str)]) -> Result<Value, String> {
        let body = fields
            .iter()
            .map(|(name, value)| format!("{}={}", segment(name), segment(value)))
            .collect::<Vec<_>>()
            .join("&");
        self.call_as(method, "application/x-www-form-urlencoded", body.as_bytes())
    }

    fn call_as(&self, method: &str, content_type: &str, body: &[u8]) -> Result<Value, String> {
        let auth = format!("Bearer {}", self.token);
        let fetched = self
            .client
            .request(
                "POST",
                &format!("{}/{method}", self.base),
                &[("Authorization", &auth), ("Content-Type", content_type)],
                Some(body),
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

    /// Shares a file in `chat` (in `thread`, if not empty), with `caption`:
    /// Slack's upload, an address for the file, the bytes to it (it takes no
    /// token), then the file shared.
    fn upload(
        &self,
        chat: &str,
        thread: &str,
        name: &str,
        media_type: &str,
        bytes: &[u8],
        caption: &str,
    ) -> Result<(), String> {
        let length = bytes.len().to_string();
        let to = self.call_form(
            "files.getUploadURLExternal",
            &[("filename", name), ("length", &length)],
        )?;
        let url = to["upload_url"]
            .as_str()
            .ok_or("Slack gave no address to upload to")?;
        let id = to["file_id"].as_str().ok_or("Slack gave no file id")?;
        self.client
            .request(
                "POST",
                url,
                &[("Content-Type", media_type)],
                Some(bytes),
                1 << 20,
            )
            .map_err(|e| format!("the upload to Slack: {e}"))?;
        let files = json!([{"id": id, "title": name}]).to_string();
        let caption: String = caption.chars().take(3_000).collect();
        let mut fields = vec![("files", files.as_str()), ("channel_id", chat)];
        if !caption.is_empty() {
            fields.push(("initial_comment", caption.as_str()));
        }
        if !thread.is_empty() {
            fields.push(("thread_ts", thread));
        }
        self.call_form("files.completeUploadExternal", &fields)?;
        Ok(())
    }

    /// Whether `url` is Slack's own (its files', or the API's server), the
    /// only addresses the bot's token goes to.
    fn is_own(&self, url: &str) -> bool {
        url.starts_with("https://files.slack.com/")
            || origin(&self.base).is_some_and(|own| url.starts_with(&format!("{own}/")))
    }
}

/// The scheme and authority of `url` (`https://slack.com`).
fn origin(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    (!authority.is_empty()).then(|| format!("{scheme}://{authority}"))
}

impl Messenger for Slack {
    fn acknowledge(&self, message: &Incoming, ack: Ack) -> Result<(), String> {
        let react = |method: &str, name: &str| -> Result<(), String> {
            match self.call(
                method,
                &json!({"channel": message.chat, "timestamp": message.id, "name": name}),
            ) {
                // Already so: nothing to do.
                Err(why) if why.contains("already_reacted") || why.contains("no_reaction") => {
                    Ok(())
                }
                other => other.map(|_| ()),
            }
        };
        match ack {
            Ack::Seen => react("reactions.add", "eyes"),
            Ack::Done => react("reactions.remove", "eyes"),
            Ack::Failed => {
                react("reactions.remove", "eyes")?;
                react("reactions.add", "warning")
            }
        }
    }

    fn name(&self) -> &'static str {
        "slack"
    }

    fn chat_word(&self) -> &'static str {
        "channel"
    }

    fn receive(&self, cursor: &str, wait: u64) -> Result<(Vec<Incoming>, String), String> {
        if let Some(app_token) = &self.app_token {
            return Ok((self.live(app_token).receive(wait)?, cursor.to_string()));
        }
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
                let voice = crate::live::slack_voice(message);
                if oldest.is_none() || !crate::live::slack_said(message, &voice) {
                    continue;
                }
                let text = message["text"].as_str().unwrap_or_default();
                let files = crate::live::slack_files(message);
                if !text.is_empty() || voice.is_some() || !files.is_empty() {
                    let mut mentions = crate::live::slack_mentions(text);
                    if let Some(parent) = message["parent_user_id"].as_str() {
                        mentions.push(parent.to_string());
                    }
                    out.push(Incoming {
                        id: ts.to_string(),
                        thread: crate::live::slack_thread(message),
                        chat: channel.clone(),
                        from: message["user"].as_str().unwrap_or_default().to_string(),
                        text: text.to_string(),
                        voice,
                        // Direct messages' channels begin with D.
                        direct: channel.starts_with('D'),
                        mentions,
                        files,
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

    fn reply(&self, message: &Incoming, text: &str) -> Result<(), String> {
        crate::drafts::slack_reply(self, message, text)
    }

    fn room(&self) -> usize {
        3_000
    }

    fn typist(&self, message: &Incoming, _aloud: bool) -> Option<Box<dyn crate::showing::Typist>> {
        crate::drafts::slack_typist(self, message)
    }

    fn drafting(&self, message: &Incoming) -> Option<crate::showing::Drafting> {
        crate::drafts::slack_drafting(self, message)
    }

    fn fetch_voice(&self, note: &Note) -> Result<crate::voice::Audio, String> {
        if !self.is_own(&note.id) {
            return Err("a file that is not on Slack is not fetched with the bot's token".into());
        }
        let auth = format!("Bearer {}", self.token);
        let fetched = self
            .client
            .request(
                "GET",
                &note.id,
                &[("Authorization", &auth)],
                None,
                crate::voice::MAX_HEARD as u64,
            )
            .map_err(|e| hide(e.to_string(), &self.token))?;
        let media_type = note
            .media_type
            .clone()
            .unwrap_or_else(|| crate::voice::Audio::sniff(&fetched.body).to_string());
        Ok(crate::voice::Audio::new(&media_type, fetched.body))
    }

    fn send_voice(
        &self,
        chat: &str,
        audio: &crate::voice::Audio,
        caption: &str,
    ) -> Result<bool, String> {
        self.upload(
            chat,
            "",
            &audio.name,
            &audio.media_type,
            &audio.bytes,
            caption,
        )
        .map(|()| true)
    }

    fn send_file(
        &self,
        message: &Incoming,
        file: &crate::files::File,
        caption: &str,
    ) -> Result<bool, String> {
        self.upload(
            &message.chat,
            &message.thread,
            &file.name,
            &file.media_type,
            &file.bytes,
            caption,
        )
        .map(|()| true)
    }

    fn long_polls(&self) -> bool {
        self.app_token.is_some()
    }

    fn sees_strangers(&self) -> bool {
        self.app_token.is_some()
    }

    fn once_wait(&self) -> u64 {
        if self.app_token.is_some() {
            LIVE_ONCE_WAIT
        } else {
            0
        }
    }

    fn me(&self) -> Result<Vec<String>, String> {
        if let Some(me) = self.me.get() {
            return Ok(me.clone());
        }
        let who = self.call("auth.test", &json!({}))?;
        if let Some(team) = who["team_id"].as_str() {
            let _ = self.team.set(team.to_string());
        }
        let me = vec![who["user_id"]
            .as_str()
            .ok_or("Slack did not say who the bot is")?
            .to_string()];
        Ok(self.me.get_or_init(|| me).clone())
    }
}

/// How long one batch waits on a messenger that passes on only what is
/// said while it listens, in seconds.
pub const LIVE_ONCE_WAIT: u64 = 30;

/// Discord: its gateway for messages, in servers' channels and direct
/// messages ([`crate::live`]), and its HTTP API to post them. The bot
/// needs the message-content intent to read what people write.
pub struct Discord {
    pub(crate) client: sion_http::Client,
    pub(crate) base: String,
    pub(crate) token: String,
    live: OnceLock<crate::live::Live>,
}

impl std::fmt::Debug for Discord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Discord({})", self.base)
    }
}

impl Discord {
    /// The bot whose token is in `DISCORD_BOT_TOKEN`, at `DISCORD_API_BASE`
    /// (default `https://discord.com/api/v10`).
    pub fn from_env() -> Result<Discord, String> {
        Ok(Discord {
            client: client(30)?,
            base: env("DISCORD_API_BASE")
                .unwrap_or_else(|| "https://discord.com/api/v10".into())
                .trim_end_matches('/')
                .to_string(),
            token: env("DISCORD_BOT_TOKEN").ok_or("set DISCORD_BOT_TOKEN")?,
            live: OnceLock::new(),
        })
    }

    /// The gateway, connected the first time it is needed.
    fn live(&self) -> &crate::live::Live {
        self.live.get_or_init(|| {
            crate::live::Live::start(
                "discord",
                crate::live::discord(self.client.clone(), self.base.clone(), self.token.clone()),
            )
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
        // A reaction's answer, say, has no body.
        if fetched.body.is_empty() {
            return Ok(Value::Null);
        }
        json_of(&fetched.body, "Discord")
    }
}

impl Messenger for Discord {
    fn acknowledge(&self, message: &Incoming, ack: Ack) -> Result<(), String> {
        let path = |emoji: &str| {
            format!(
                "/channels/{}/messages/{}/reactions/{}/@me",
                segment(&message.chat),
                segment(&message.id),
                segment(emoji)
            )
        };
        match ack {
            Ack::Seen => self.call("PUT", &path(SEEN), None).map(|_| ()),
            Ack::Done => self.call("DELETE", &path(SEEN), None).map(|_| ()),
            Ack::Failed => {
                self.call("DELETE", &path(SEEN), None)?;
                self.call("PUT", &path(FAILED), None).map(|_| ())
            }
        }
    }

    fn name(&self) -> &'static str {
        "discord"
    }

    fn chat_word(&self) -> &'static str {
        "channel"
    }

    fn receive(&self, cursor: &str, wait: u64) -> Result<(Vec<Incoming>, String), String> {
        Ok((self.live().receive(wait)?, cursor.to_string()))
    }

    fn send(&self, chat: &str, text: &str) -> Result<(), String> {
        self.call(
            "POST",
            &format!("/channels/{}/messages", segment(chat)),
            Some(&json!({"content": cut(text, 2_000)})),
        )
        .map(|_| ())
    }

    fn room(&self) -> usize {
        2_000
    }

    fn typist(&self, message: &Incoming, _aloud: bool) -> Option<Box<dyn crate::showing::Typist>> {
        crate::drafts::discord_typist(self, message)
    }

    fn drafting(&self, message: &Incoming) -> Option<crate::showing::Drafting> {
        crate::drafts::discord_drafting(self, message)
    }

    fn long_polls(&self) -> bool {
        true
    }

    fn once_wait(&self) -> u64 {
        LIVE_ONCE_WAIT
    }

    fn me(&self) -> Result<Vec<String>, String> {
        self.live().me(Duration::from_secs(LIVE_ONCE_WAIT))
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

    fn send_voice(
        &self,
        chat: &str,
        audio: &crate::voice::Audio,
        caption: &str,
    ) -> Result<bool, String> {
        // The audio as an attachment, which Discord plays where it shows
        // the message; the reply as the message's text.
        self.attach(chat, caption, &audio.name, &audio.media_type, &audio.bytes)
            .map(|()| true)
    }

    fn send_file(
        &self,
        message: &Incoming,
        file: &crate::files::File,
        caption: &str,
    ) -> Result<bool, String> {
        self.attach(
            &message.chat,
            caption,
            &file.name,
            &file.media_type,
            &file.bytes,
        )
        .map(|()| true)
    }
}

impl Discord {
    /// Posts a message to `chat` with a file attached, `caption` its text.
    fn attach(
        &self,
        chat: &str,
        caption: &str,
        name: &str,
        media_type: &str,
        bytes: &[u8],
    ) -> Result<(), String> {
        // A file alone has no text.
        let payload = if caption.is_empty() {
            json!({})
        } else {
            json!({"content": cut(caption, 2_000)})
        };
        let (content_type, body) = crate::voice::Form::new()
            .text("payload_json", &payload.to_string())
            .bytes("files[0]", name, media_type, bytes)
            .finish();
        let auth = format!("Bot {}", self.token);
        self.client
            .request(
                "POST",
                &format!("{}/channels/{}/messages", self.base, segment(chat)),
                &[("Authorization", &auth), ("Content-Type", &content_type)],
                Some(&body),
                4 << 20,
            )
            .map(|_| ())
            .map_err(|e| hide(e.to_string(), &self.token))
    }
}

/// Matrix, through the client-server API: long polls `/sync`, and sends
/// `m.text` messages to rooms. With pairing, it joins the rooms it is
/// invited to (someone who writes to it first invites it to a room).
pub struct Matrix {
    pub(crate) client: sion_http::Client,
    pub(crate) base: String,
    pub(crate) token: String,
    pub(crate) me: String,
    join_invites: bool,
    /// Whether each room seen is a direct one (the account and one other).
    rooms: Mutex<BTreeMap<String, bool>>,
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
            rooms: Mutex::new(BTreeMap::new()),
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

    /// Whether `room` holds the account and one other, as the room's
    /// summary said, or as its members are now.
    fn direct(&self, room: &str) -> bool {
        if let Some(direct) = self.rooms.lock().ok().and_then(|r| r.get(room).copied()) {
            return direct;
        }
        let path = format!("/_matrix/client/v3/rooms/{}/joined_members", segment(room));
        match self.call("GET", &path, None) {
            Ok(members) => {
                let direct = members["joined"].as_object().is_some_and(|j| j.len() == 2);
                if let Ok(mut rooms) = self.rooms.lock() {
                    rooms.insert(room.to_string(), direct);
                }
                direct
            }
            // Not known: taken as a group's, and asked again next time.
            Err(_) => false,
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
    /// Read receipts, as a person's client sends them; a reaction on what
    /// failed.
    fn acknowledge(&self, message: &Incoming, ack: Ack) -> Result<(), String> {
        let room = segment(&message.chat);
        match ack {
            Ack::Seen => self
                .call(
                    "POST",
                    &format!(
                        "/_matrix/client/v3/rooms/{room}/receipt/m.read/{}",
                        segment(&message.id)
                    ),
                    Some(&json!({})),
                )
                .map(|_| ()),
            Ack::Done => Ok(()),
            Ack::Failed => self
                .call(
                    "PUT",
                    &format!(
                        "/_matrix/client/v3/rooms/{room}/send/m.reaction/{}",
                        crate::drafts::txn()
                    ),
                    Some(&json!({"m.relates_to": {"rel_type": "m.annotation",
                        "event_id": message.id, "key": FAILED}})),
                )
                .map(|_| ()),
        }
    }

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
        if let Some(rooms) = value["rooms"]["join"].as_object() {
            for (room, joined) in rooms {
                if let Some(count) = joined["summary"]["m.joined_member_count"].as_u64() {
                    if let Ok(mut known) = self.rooms.lock() {
                        known.insert(room.clone(), count == 2);
                    }
                }
                if cursor.is_empty() {
                    continue;
                }
                for event in joined["timeline"]["events"]
                    .as_array()
                    .into_iter()
                    .flatten()
                {
                    let sender = event["sender"].as_str().unwrap_or_default();
                    let content = &event["content"];
                    // A voice message is an `m.audio` one; its body is its
                    // caption only where a file name is given apart.
                    let voice = (content["msgtype"] == "m.audio")
                        .then(|| content["url"].as_str())
                        .flatten()
                        .map(|url| Note {
                            id: url.to_string(),
                            media_type: content["info"]["mimetype"].as_str().map(str::to_string),
                            name: None,
                        });
                    // A picture, a video or a file; in an encrypted room it
                    // has no plain address, and is not taken.
                    let file = matches!(
                        content["msgtype"].as_str(),
                        Some("m.image" | "m.file" | "m.video")
                    )
                    .then(|| content["url"].as_str())
                    .flatten()
                    .map(|url| Note {
                        id: url.to_string(),
                        media_type: content["info"]["mimetype"].as_str().map(str::to_string),
                        name: content["filename"]
                            .as_str()
                            .or(content["body"].as_str())
                            .map(str::to_string),
                    });
                    if event["type"] != "m.room.message"
                        || (content["msgtype"] != "m.text" && voice.is_none() && file.is_none())
                        || sender == self.me
                    {
                        continue;
                    }
                    let body = content["body"].as_str();
                    let captioned =
                        content["filename"].is_string() && content["filename"] != content["body"];
                    let said = match (&voice, &file) {
                        (None, None) => body,
                        _ if captioned => body,
                        _ => Some(""),
                    };
                    if let Some(text) = said {
                        // Mentions as the spec has them, and the account
                        // named in the text by older clients.
                        let mut mentions: Vec<String> = event["content"]["m.mentions"]["user_ids"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(|id| id.as_str().map(str::to_string))
                            .collect();
                        if text.contains(&self.me) {
                            mentions.push(self.me.clone());
                        }
                        out.push(Incoming {
                            id: event["event_id"].as_str().unwrap_or_default().to_string(),
                            thread: String::new(),
                            chat: room.clone(),
                            from: sender.to_string(),
                            text: text.to_string(),
                            voice: voice.clone(),
                            direct: self.direct(room),
                            mentions,
                            files: file.into_iter().collect(),
                        });
                    }
                }
            }
        }
        Ok((out, next))
    }

    fn me(&self) -> Result<Vec<String>, String> {
        Ok(vec![self.me.clone()])
    }

    fn send(&self, chat: &str, text: &str) -> Result<(), String> {
        self.send_content(
            chat,
            &json!({"msgtype": "m.text", "body": cut(text, 16_000)}),
        )
    }

    fn room(&self) -> usize {
        16_000
    }

    fn typist(&self, message: &Incoming, _aloud: bool) -> Option<Box<dyn crate::showing::Typist>> {
        crate::drafts::matrix_typist(self, message)
    }

    fn drafting(&self, message: &Incoming) -> Option<crate::showing::Drafting> {
        crate::drafts::matrix_drafting(self, message)
    }

    fn long_polls(&self) -> bool {
        true
    }

    fn fetch_voice(&self, note: &Note) -> Result<crate::voice::Audio, String> {
        // mxc://<server>/<media id>, each a plain name.
        let (server, media) = note
            .id
            .strip_prefix("mxc://")
            .and_then(|rest| rest.split_once('/'))
            .filter(|(server, media)| {
                !server.is_empty()
                    && !media.is_empty()
                    && !format!("{server}{media}").contains(['/', '?', '#', '\\'])
            })
            .ok_or("the voice message's address is not a Matrix one")?;
        let auth = format!("Bearer {}", self.token);
        let fetch = |path: String| {
            self.client.request(
                "GET",
                &format!("{}{path}", self.base),
                &[("Authorization", &auth)],
                None,
                crate::voice::MAX_HEARD as u64,
            )
        };
        // Authenticated media (Matrix 1.11), or the older path.
        let fetched = fetch(format!(
            "/_matrix/client/v1/media/download/{}/{}",
            segment(server),
            segment(media)
        ))
        .or_else(|_| {
            fetch(format!(
                "/_matrix/media/v3/download/{}/{}",
                segment(server),
                segment(media)
            ))
        })
        .map_err(|e| hide(e.to_string(), &self.token))?;
        let media_type = note
            .media_type
            .clone()
            .unwrap_or_else(|| crate::voice::Audio::sniff(&fetched.body).to_string());
        Ok(crate::voice::Audio::new(&media_type, fetched.body))
    }

    fn send_voice(
        &self,
        chat: &str,
        audio: &crate::voice::Audio,
        caption: &str,
    ) -> Result<bool, String> {
        let uri = self.upload(&audio.name, &audio.media_type, &audio.bytes)?;
        // A voice message (MSC3245), the reply as its caption.
        self.send_content(
            chat,
            &json!({
                "msgtype": "m.audio",
                "body": cut(caption, 16_000),
                "filename": audio.name,
                "url": uri,
                "info": {"mimetype": audio.media_type, "size": audio.bytes.len()},
                "org.matrix.msc1767.audio": {},
                "org.matrix.msc3245.voice": {},
            }),
        )?;
        Ok(true)
    }

    /// A picture as an `m.image`, anything else as an `m.file`; `caption`,
    /// if any, its body, the file's name apart.
    fn send_file(
        &self,
        message: &Incoming,
        file: &crate::files::File,
        caption: &str,
    ) -> Result<bool, String> {
        let uri = self.upload(&file.name, &file.media_type, &file.bytes)?;
        let kind = if file.is_picture() {
            "m.image"
        } else {
            "m.file"
        };
        let body = if caption.is_empty() {
            file.name.clone()
        } else {
            cut(caption, 16_000)
        };
        self.send_content(
            &message.chat,
            &json!({
                "msgtype": kind,
                "body": body,
                "filename": file.name,
                "url": uri,
                "info": {"mimetype": file.media_type, "size": file.bytes.len()},
            }),
        )?;
        Ok(true)
    }
}

impl Matrix {
    /// Uploads a file to the homeserver: its `mxc://` address.
    fn upload(&self, name: &str, media_type: &str, bytes: &[u8]) -> Result<String, String> {
        let auth = format!("Bearer {}", self.token);
        let uploaded = self
            .client
            .request(
                "POST",
                &format!(
                    "{}/_matrix/media/v3/upload?filename={}",
                    self.base,
                    segment(name)
                ),
                &[("Authorization", &auth), ("Content-Type", media_type)],
                Some(bytes),
                1 << 20,
            )
            .map_err(|e| hide(e.to_string(), &self.token))?;
        Ok(json_of(&uploaded.body, "the homeserver")?["content_uri"]
            .as_str()
            .ok_or("the homeserver gave no address for the upload")?
            .to_string())
    }

    fn send_content(&self, chat: &str, content: &Value) -> Result<(), String> {
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
            Some(content),
        )
        .map(|_| ())
    }
}

/// Signal, through signal-cli-rest-api (a local service holding the
/// number's keys): polls `receive` for direct messages, and sends with
/// `/v2/send`.
pub struct Signal {
    pub(crate) client: sion_http::Client,
    pub(crate) base: String,
    pub(crate) number: String,
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
    /// Read receipts; a reaction on what failed.
    fn acknowledge(&self, message: &Incoming, ack: Ack) -> Result<(), String> {
        let timestamp: u64 = message
            .id
            .parse()
            .map_err(|_| format!("{} is not a Signal message's timestamp", message.id))?;
        let (path, body) = match ack {
            Ack::Seen => (
                "receipts",
                json!({"receipt_type": "read", "recipient": message.chat, "timestamp": timestamp}),
            ),
            Ack::Done => return Ok(()),
            Ack::Failed => (
                "reactions",
                json!({"reaction": FAILED, "recipient": message.chat,
                    "target_author": message.from, "timestamp": timestamp}),
            ),
        };
        self.client
            .post_json(
                &format!("{}/v1/{path}/{}", self.base, segment(&self.number)),
                &[],
                body.to_string().as_bytes(),
                1 << 16,
            )
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

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
            // A voice note is an audio attachment.
            let voice = message["attachments"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|a| {
                    a["contentType"]
                        .as_str()
                        .is_some_and(|t| t.starts_with("audio/"))
                })
                .and_then(|a| {
                    Some(Note {
                        id: a["id"].as_str()?.to_string(),
                        media_type: a["contentType"].as_str().map(str::to_string),
                        name: None,
                    })
                });
            // Every other attachment is a file.
            let files: Vec<Note> = message["attachments"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|a| {
                    !a["contentType"]
                        .as_str()
                        .is_some_and(|t| t.starts_with("audio/"))
                })
                .filter_map(|a| {
                    Some(Note {
                        id: a["id"].as_str()?.to_string(),
                        media_type: a["contentType"].as_str().map(str::to_string),
                        name: a["filename"].as_str().map(str::to_string),
                    })
                })
                .collect();
            let text = message["message"].as_str().unwrap_or_default();
            if !from.is_empty() && (!text.is_empty() || voice.is_some() || !files.is_empty()) {
                out.push(Incoming {
                    // Signal names a message by when it was sent.
                    id: message["timestamp"]
                        .as_u64()
                        .or(envelope["timestamp"].as_u64())
                        .map(|at| at.to_string())
                        .unwrap_or_default(),
                    thread: String::new(),
                    chat: from.to_string(),
                    from: from.to_string(),
                    text: text.to_string(),
                    voice,
                    direct: true,
                    mentions: Vec::new(),
                    files,
                });
            }
        }
        // Received messages leave the service's queue: no cursor to keep.
        Ok((out, String::new()))
    }

    fn send(&self, chat: &str, text: &str) -> Result<(), String> {
        self.post(
            &json!({"message": cut(text, 4_000), "number": self.number, "recipients": [chat]}),
        )
    }

    fn typist(&self, message: &Incoming, _aloud: bool) -> Option<Box<dyn crate::showing::Typist>> {
        crate::drafts::signal_typist(self, message)
    }

    fn long_polls(&self) -> bool {
        false
    }

    fn fetch_voice(&self, note: &Note) -> Result<crate::voice::Audio, String> {
        if note.id.is_empty() || note.id.contains(['/', '?', '#', '\\']) {
            return Err("not an attachment's id".into());
        }
        let fetched = self
            .client
            .get(
                &format!("{}/v1/attachments/{}", self.base, segment(&note.id)),
                crate::voice::MAX_HEARD as u64,
            )
            .map_err(|e| e.to_string())?;
        let media_type = note
            .media_type
            .clone()
            .unwrap_or_else(|| crate::voice::Audio::sniff(&fetched.body).to_string());
        Ok(crate::voice::Audio::new(&media_type, fetched.body))
    }

    fn send_voice(
        &self,
        chat: &str,
        audio: &crate::voice::Audio,
        caption: &str,
    ) -> Result<bool, String> {
        self.attach(chat, caption, &audio.name, &audio.media_type, &audio.bytes)
            .map(|()| true)
    }

    fn send_file(
        &self,
        message: &Incoming,
        file: &crate::files::File,
        caption: &str,
    ) -> Result<bool, String> {
        self.attach(
            &message.chat,
            caption,
            &file.name,
            &file.media_type,
            &file.bytes,
        )
        .map(|()| true)
    }
}

impl Signal {
    /// Sends `chat` a message with a file attached, `caption` its text.
    fn attach(
        &self,
        chat: &str,
        caption: &str,
        name: &str,
        media_type: &str,
        bytes: &[u8],
    ) -> Result<(), String> {
        // The data URL's own separators may not be in the name.
        let name: String = name
            .chars()
            .map(|c| {
                if matches!(c, ';' | ',') || c.is_control() {
                    '_'
                } else {
                    c
                }
            })
            .collect();
        let attachment = format!(
            "data:{media_type};filename={name};base64,{}",
            sion_http::base64(bytes)
        );
        // A file alone has no text.
        let message: String = caption.chars().take(4_000).collect();
        self.post(&json!({"message": message, "number": self.number,
            "recipients": [chat], "base64_attachments": [attachment]}))
    }
}

impl Signal {
    fn post(&self, body: &Value) -> Result<(), String> {
        self.client
            .post_json(
                &format!("{}/v2/send", self.base),
                &[],
                body.to_string().as_bytes(),
                1 << 20,
            )
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

/// Telegram, through the Bot API ([`crate::channels::Telegram`]): long
/// polls `getUpdates`, keeping the offset as its cursor.
impl Messenger for crate::channels::Telegram {
    /// A bot's reaction on the message, which each mark replaces.
    fn acknowledge(&self, message: &Incoming, ack: Ack) -> Result<(), String> {
        let (chat, _) = crate::drafts::telegram_place(message)?;
        let id: i64 = message
            .id
            .parse()
            .map_err(|_| format!("{} is not a Telegram message id", message.id))?;
        let reaction = match ack {
            Ack::Seen => json!([{"type": "emoji", "emoji": SEEN}]),
            Ack::Done => json!([]),
            // Telegram offers bots no warning sign.
            Ack::Failed => json!([{"type": "emoji", "emoji": "😢"}]),
        };
        self.call(
            "setMessageReaction",
            &json!({"chat_id": chat, "message_id": id, "reaction": reaction}),
        )
        .map(|_| ())
    }

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
                id: u.message.to_string(),
                thread: u.topic.map(|t| t.to_string()).unwrap_or_default(),
                chat: u.chat.to_string(),
                from: u.from.map(|f| f.to_string()).unwrap_or_default(),
                text: u.text,
                voice: u.voice.map(|(id, media_type)| Note {
                    id,
                    media_type: Some(media_type),
                    name: None,
                }),
                direct: u.direct,
                mentions: u.mentions,
                files: u.files,
            })
            .collect();
        Ok((messages, next.to_string()))
    }

    fn me(&self) -> Result<Vec<String>, String> {
        crate::channels::Telegram::me(self)
    }

    fn send(&self, chat: &str, text: &str) -> Result<(), String> {
        let chat: i64 = chat
            .parse()
            .map_err(|_| format!("{chat} is not a Telegram chat id"))?;
        crate::channels::Telegram::send(self, chat, text)
    }

    fn reply(&self, message: &Incoming, text: &str) -> Result<(), String> {
        let (chat, topic) = crate::drafts::telegram_place(message)?;
        self.send_in(chat, topic, text).map(|_| ())
    }

    fn room(&self) -> usize {
        crate::drafts::TELEGRAM_ROOM
    }

    fn typist(&self, message: &Incoming, aloud: bool) -> Option<Box<dyn crate::showing::Typist>> {
        crate::drafts::telegram_typist(self, message, aloud)
    }

    fn drafting(&self, message: &Incoming) -> Option<crate::showing::Drafting> {
        crate::drafts::telegram_drafting(self, message)
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

    /// A picture as a photo, anything else as a document, in the
    /// message's topic.
    fn send_file(
        &self,
        message: &Incoming,
        file: &crate::files::File,
        caption: &str,
    ) -> Result<bool, String> {
        let (chat, topic) = crate::drafts::telegram_place(message)?;
        self.send_file_in(chat, topic, file, caption).map(|()| true)
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
    let listen = if options.listen.is_empty() {
        crate::webhooks::LISTEN
    } else {
        &options.listen
    };
    Ok(match name {
        "telegram" => Box::new(crate::channels::Telegram::from_env()?),
        "slack" => Box::new(Slack::from_env(chats)?),
        "discord" => Box::new(Discord::from_env()?),
        "matrix" if options.pairing => Box::new(Matrix::from_env()?.joining_invites()),
        "matrix" => Box::new(Matrix::from_env()?),
        "signal" => Box::new(Signal::from_env()?),
        "whatsapp" => Box::new(crate::webhooks::WhatsApp::from_env(listen)?),
        "line" => Box::new(crate::webhooks::Line::from_env(listen)?),
        "sms" => Box::new(crate::webhooks::Sms::from_env(listen)?),
        "mattermost" => Box::new(crate::mattermost::Mattermost::from_env()?),
        "irc" => Box::new(crate::irc::Irc::irc_from_env(chats)?),
        "twitch" => Box::new(crate::irc::Irc::twitch_from_env(chats)?),
        "email" => Box::new(crate::email::Email::from_env()?),
        "imessage" => Box::new(crate::imessage::IMessage::from_env()?),
        "dingtalk" => Box::new(crate::dingtalk::DingTalk::from_env()?),
        "feishu" | "lark" => Box::new(crate::feishu::Feishu::from_env(listen, name == "lark")?),
        "nextcloud-talk" => Box::new(crate::nextcloud::NextcloudTalk::from_env(listen)?),
        "teams" => Box::new(crate::teams::Teams::from_env(listen)?),
        "google-chat" => Box::new(crate::google_chat::GoogleChat::from_env(listen)?),
        other => {
            return Err(format!(
                "{other} is not a messenger: telegram, slack, discord, matrix, signal, mattermost, irc, twitch, whatsapp, line, sms, teams, google-chat, feishu, nextcloud-talk, dingtalk, email or imessage"
            ))
        }
    })
}

/// Seconds between polls of a messenger that does not wait for messages.
pub const POLL_PAUSE: u64 = 3;

/// `--pairing`, `--hear` and `--speak`, for a channel's command.
#[derive(Clone, Debug, Default, clap::Args)]
pub struct ChannelArgs {
    /// Give someone who writes to it directly, and whom it does not know,
    /// a pairing code, and answer them once you approve it (`sion agent
    /// pairing approve`). Matrix joins the rooms it is invited to; Slack
    /// needs Socket Mode (SLACK_APP_TOKEN).
    #[arg(long)]
    pub pairing: bool,
    /// In the group chats it answers, answer only what mentions it or
    /// answers one of its messages (direct messages are always answered).
    #[arg(long)]
    pub require_mention: bool,
    /// Hear voice notes: openai:<model>, groq:<model>, or
    /// command:<program> (given the audio's file, it prints the text).
    #[arg(long, value_name = "PROVIDER")]
    pub hear: Option<String>,
    /// Answer voice notes aloud: openai:<model>:<voice>,
    /// elevenlabs:<voice id>[:<model>], or command:<program> (given the
    /// text, it writes Ogg Opus or MP3).
    #[arg(long, value_name = "PROVIDER", requires = "hear")]
    pub speak: Option<String>,
    /// How a reply shows while the agent writes it, where the messenger
    /// can show it: `progress`, the text as the model writes it and the
    /// tool the agent runs; `partial`, the text only; `off`, the reply once
    /// it is done.
    #[arg(long, value_enum, default_value_t = Streaming::Progress)]
    pub stream: Streaming,
    /// Show no typing indicator while the agent works.
    #[arg(long)]
    pub no_typing: bool,
    /// What a message does that comes while the agent works on its chat:
    /// `steer`, it is told to the agent as it works; `queue`, answered
    /// after, one at a time; `collect`, answered after, together;
    /// `interrupt`, it stops the agent, which takes it up. `/stop` always
    /// stops it.
    #[arg(long, value_enum, default_value_t = Busy::Steer)]
    pub busy: Busy,
    /// Milliseconds a chat's messages are gathered before the agent takes
    /// them up, so that a burst of them is one turn.
    #[arg(long, value_name = "MS", default_value_t = 500)]
    pub debounce: u64,
    /// How many chats are answered at once.
    #[arg(long, value_name = "N", default_value_t = 4)]
    pub parallel: usize,
    /// Mark no message received, answered or failed (with a reaction, or a
    /// read receipt).
    #[arg(long)]
    pub no_reactions: bool,
    /// A chat that another profile answers: `<chat>=<profile>`, as often as
    /// needed (comma-separated too). The chat is answered as --allow's.
    #[arg(long, value_name = "CHAT=PROFILE", value_delimiter = ',')]
    pub route: Vec<String>,
}

/// What a bridge marks a message it took with ([`Messenger::acknowledge`]):
/// a reaction (👀 while the agent is at it, gone once it answered, ⚠️ if
/// that failed), or a read receipt where the messenger has them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ack {
    /// Taken: the agent will answer it, or is at it.
    Seen,
    /// Answered.
    Done,
    /// Not answered: something failed.
    Failed,
}

/// The reaction on a message the agent will answer, or is at ([`Ack::Seen`]).
pub const SEEN: &str = "👀";

/// The reaction on a message that could not be answered ([`Ack::Failed`]).
pub const FAILED: &str = "⚠️";

/// What a message does that comes while the agent works on its chat
/// (`--busy`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum Busy {
    /// Told to the agent as it works: it takes it in before its next step.
    Steer,
    /// Answered once the agent is done, one at a time.
    #[default]
    Queue,
    /// Answered once the agent is done, all of them together.
    Collect,
    /// Stops the agent, which then takes it up.
    Interrupt,
}

/// How a reply shows while the agent writes it (`--stream`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum Streaming {
    /// The text as the model writes it, and the tool the agent runs.
    #[default]
    Progress,
    /// The text as the model writes it.
    Partial,
    /// The reply once it is done.
    Off,
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
            require_mention: self.require_mention,
            listen: String::new(),
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
            stream: self.stream,
            no_typing: self.no_typing,
            busy: self.busy,
            debounce_ms: self.debounce,
            parallel: self.parallel,
            acknowledge: !self.no_reactions,
            routes: self
                .route
                .iter()
                .map(|route| {
                    let (chat, profile) = route.split_once('=').ok_or_else(|| {
                        CliError::Usage(format!("a route is <chat>=<profile>, not {route}"))
                    })?;
                    Ok((
                        chat.trim().to_string(),
                        crate::profiles::get(profile.trim())?,
                    ))
                })
                .collect::<Result<_, CliError>>()?,
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
    /// Give a direct chat it does not know a pairing code
    /// ([`crate::pairing`]), and answer it once the operator approves the
    /// code.
    pub pairing: bool,
    /// In group chats, answer only what mentions the bot or answers it.
    pub require_mention: bool,
    /// Where messengers that send webhooks reach the bridge
    /// ([`crate::webhooks::LISTEN`] if empty).
    pub listen: String,
    /// What hears voice notes, if any are heard.
    pub hearing: Option<crate::voice::Hearing>,
    /// What speaks the replies to voice notes, if they are spoken.
    pub speaking: Option<crate::voice::Speaking>,
    /// How a reply shows while the agent writes it.
    pub stream: Streaming,
    /// Show no typing indicator.
    pub no_typing: bool,
    /// What a message does that comes while the agent works on its chat.
    pub busy: Busy,
    /// Milliseconds a chat's messages are gathered before the agent takes
    /// them up (none: each is taken up as it comes).
    pub debounce_ms: u64,
    /// How many chats are answered at once (0 is one).
    pub parallel: usize,
    /// Mark each message received, and answered or failed, where the
    /// messenger can ([`Messenger::acknowledge`]).
    pub acknowledge: bool,
    /// The chats another profile answers than the bridge's, by chat: one
    /// messenger account, many profiles. They are answered as `allow`'s.
    pub routes: BTreeMap<String, Profile>,
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
/// voice notes heard and answered aloud, chats answered side by side, and
/// what is said while the agent works told to it, queued or collected
/// ([`crate::bridge`]).
pub fn bridge_with(
    ledger: SharedLedger,
    profile: &Profile,
    messenger: &dyn Messenger,
    options: &Options,
) -> Result<(), CliError> {
    crate::bridge::run(ledger, profile, messenger, options)
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
