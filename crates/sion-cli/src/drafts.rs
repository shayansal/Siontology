//! Typing indicators and replies shown as they are written, messenger by
//! messenger ([`crate::showing`]), each the way its platform's API offers:
//!
//! | Messenger | Typing | The reply as it is written |
//! |---|---|---|
//! | Telegram | `sendChatAction`, again every 4 s (each lasts 5 s) | in a private chat, `sendMessageDraft`: an animated draft that the message replaces; in a group, a message edited every 3 s |
//! | Slack | in a thread, Slack's own status line (`assistant.threads.setStatus`), where the app may set it; otherwise an hourglass on the message | in a thread, Slack's stream (`chat.startStream`), the tools the agent runs as tasks; elsewhere a message edited |
//! | Discord | `POST …/typing`, again every 9 s (each lasts 10 s) | a message edited |
//! | Matrix | `typing` for 30 s at a time, off at the end | a message replaced (`m.replace`), the updates marked live (MSC4357), at most one every 4 s |
//! | Signal | signal-cli's typing indicator, again every 9 s, off at the end | none: Signal lets a message be edited ten times |
//!
//! The others are in their own modules. A draft that a platform refuses
//! (`sendMessageDraft` where drafts are not allowed, a stream outside a
//! thread) turns into an edited message where one can be; a draft that
//! fails is not updated again, and the bridge sends the reply as it would
//! have.

use std::time::Duration;

use serde_json::{json, Value};

use crate::channels::Telegram;
use crate::messengers::{Incoming, Matrix, Signal, Slack};
use crate::showing::{with_status, Draft, Drafting, Typist};

/// The fewest characters a message edited as it grows is first sent with,
/// so that the notification it makes has something to read.
pub const FIRST: usize = 40;

/// The longest wait on a platform's "retry after" that a draft takes.
const MOST_RETRY: Duration = Duration::from_secs(5);

/// A messenger's HTTP API, copied for a thread of its own: its client, its
/// address, and the `Authorization` it takes.
#[derive(Clone)]
pub(crate) struct Api {
    pub client: sion_http::Client,
    pub base: String,
    pub auth: String,
    /// The secret in `auth`, kept out of errors.
    pub secret: String,
    /// The service's name, for errors.
    pub service: &'static str,
}

impl Api {
    /// `method path` with `body` as JSON if any: the answer's JSON (null
    /// for none). A refusal for going too fast is waited out once, for as
    /// long as the platform says (up to [`MOST_RETRY`]).
    pub fn call(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Value, String> {
        match self.once(method, path, body) {
            Err((Some(wait), _)) => {
                std::thread::sleep(wait.min(MOST_RETRY));
                self.once(method, path, body).map_err(|(_, why)| why)
            }
            other => other.map_err(|(_, why)| why),
        }
    }

    fn once(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Value, (Option<Duration>, String)> {
        let bytes = body.map(|b| b.to_string().into_bytes());
        let mut headers = vec![("Authorization", self.auth.as_str())];
        if bytes.is_some() {
            headers.push(("Content-Type", "application/json"));
        }
        let hide = |text: String| {
            if self.secret.is_empty() {
                text
            } else {
                text.replace(&self.secret, "<token>")
            }
        };
        match self.client.request(
            method,
            &format!("{}{path}", self.base),
            &headers,
            bytes.as_deref(),
            1 << 20,
        ) {
            Ok(fetched) if fetched.body.is_empty() => Ok(Value::Null),
            Ok(fetched) => serde_json::from_slice(&fetched.body).map_err(|e| {
                (
                    None,
                    format!(
                        "{} answered with something other than JSON: {e}",
                        self.service
                    ),
                )
            }),
            Err(sion_http::HttpError::Status {
                status: 429, body, ..
            }) => Err((
                Some(retry_after(&body)),
                hide(format!("{}: HTTP 429: {body}", self.service)),
            )),
            Err(e) => Err((None, hide(e.to_string()))),
        }
    }
}

/// A typing indicator that is one request, made again before it lapses:
/// `POST url` with `body` as JSON.
pub(crate) struct Poster {
    pub client: sion_http::Client,
    pub url: String,
    /// The `Authorization` it takes.
    pub auth: String,
    /// The secret in `auth`, kept out of errors.
    pub secret: String,
    pub body: Value,
    /// How long one lasts.
    pub lasts: Duration,
}

impl Typist for Poster {
    fn show(&mut self) -> Result<Duration, String> {
        self.client
            .request(
                "POST",
                &self.url,
                &[
                    ("Authorization", self.auth.as_str()),
                    ("Content-Type", "application/json"),
                ],
                Some(self.body.to_string().as_bytes()),
                1 << 16,
            )
            .map_err(|e| match self.secret.as_str() {
                "" => e.to_string(),
                secret => e.to_string().replace(secret, "<token>"),
            })?;
        Ok(self.lasts)
    }
}

/// How long a refusal for going too fast says to wait: Discord's
/// `retry_after` (seconds), Matrix's `retry_after_ms`, or a second.
fn retry_after(body: &str) -> Duration {
    let value: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    if let Some(ms) = value["retry_after_ms"].as_u64() {
        return Duration::from_millis(ms);
    }
    value["retry_after"]
        .as_f64()
        .filter(|s| s.is_finite() && *s >= 0.0)
        .map(Duration::from_secs_f64)
        .unwrap_or(Duration::from_secs(1))
}

/// `text` cut to `chars` characters, `(nothing)` if it is empty.
fn cut(text: &str, chars: usize) -> String {
    let text: String = text.chars().take(chars).collect();
    if text.trim().is_empty() {
        "(nothing)".into()
    } else {
        text
    }
}

/// A fresh nonzero number, for a draft's id.
fn fresh() -> i64 {
    let mut bytes = [0u8; 4];
    let _ = sion_http::random(&mut bytes);
    i64::from(u32::from_le_bytes(bytes)) + 1
}

// Telegram.

/// The most characters of a Telegram message (it takes 4,096).
pub const TELEGRAM_ROOM: usize = 4_000;

/// The chat of a Telegram message, and its topic if it is in one.
pub fn telegram_place(message: &Incoming) -> Result<(i64, Option<i64>), String> {
    let chat = message
        .chat
        .parse()
        .map_err(|_| format!("{} is not a Telegram chat id", message.chat))?;
    Ok((chat, message.thread.parse().ok()))
}

/// The body of a call to `chat`, in `topic` if given.
fn telegram_body(chat: i64, topic: Option<i64>, mut body: Value) -> Value {
    body["chat_id"] = json!(chat);
    if let Some(topic) = topic {
        body["message_thread_id"] = json!(topic);
    }
    body
}

struct TelegramTyping {
    bot: Telegram,
    chat: i64,
    topic: Option<i64>,
    action: &'static str,
}

impl Typist for TelegramTyping {
    fn show(&mut self) -> Result<Duration, String> {
        let body = telegram_body(self.chat, self.topic, json!({"action": self.action}));
        self.bot.call("sendChatAction", &body)?;
        Ok(Duration::from_secs(5))
    }
}

/// Telegram's typing indicator: `record_voice` for a reply it will hear.
pub fn telegram_typist(bot: &Telegram, message: &Incoming, aloud: bool) -> Option<Box<dyn Typist>> {
    let (chat, topic) = telegram_place(message).ok()?;
    Some(Box::new(TelegramTyping {
        bot: bot.clone(),
        chat,
        topic,
        action: if aloud { "record_voice" } else { "typing" },
    }))
}

/// A Telegram reply as it is written: a draft in a private chat, a message
/// edited otherwise (or once drafts are refused).
struct TelegramDraft {
    bot: Telegram,
    chat: i64,
    topic: Option<i64>,
    /// The draft's id, while the reply is a draft.
    draft: Option<i64>,
    /// The message edited, once there is one.
    message: Option<i64>,
    /// When it was last edited, so that edits come a second apart.
    edited: Option<std::time::Instant>,
    /// What the message shows.
    shown: String,
}

impl TelegramDraft {
    fn edit(&self, message: i64, text: &str) -> Result<(), String> {
        let body = telegram_body(
            self.chat,
            None,
            json!({"message_id": message, "text": cut(text, 4_096)}),
        );
        match self.bot.call("editMessageText", &body) {
            // The same text again is no change, not a failure.
            Err(why) if why.contains("message is not modified") => Ok(()),
            other => other.map(|_| ()),
        }
    }
}

impl Draft for TelegramDraft {
    fn show(&mut self, text: &str, status: Option<&str>) -> Result<(), String> {
        let shown = cut(&with_status(text, status), 4_096);
        if let Some(id) = self.draft {
            let body = telegram_body(
                self.chat,
                self.topic,
                json!({"draft_id": id, "text": shown}),
            );
            match self.bot.call("sendMessageDraft", &body) {
                Ok(_) => return Ok(()),
                Err(why) => {
                    println!("  (no draft here, the reply is edited instead: {why})");
                    self.draft = None;
                }
            }
        }
        let now = std::time::Instant::now();
        if self
            .edited
            .is_some_and(|at| now.duration_since(at) < Duration::from_secs(1))
        {
            return Ok(());
        }
        self.edited = Some(now);
        let result = match self.message {
            Some(message) => self.edit(message, &shown),
            None => {
                self.message = Some(self.bot.send_in(self.chat, self.topic, &shown)?);
                Ok(())
            }
        };
        self.shown = shown;
        result
    }

    fn finish(&mut self, text: &str) -> Result<(), String> {
        match self.message {
            Some(_) if self.shown == cut(text, 4_096) => Ok(()),
            Some(message) => self.edit(message, text),
            // A draft becomes the message sent: the draft goes.
            None => self.bot.send_in(self.chat, self.topic, text).map(|_| ()),
        }
    }
}

/// How a Telegram reply shows as it is written: drafts in a private chat,
/// which lapse unless shown again within 30 seconds; edits in a group,
/// which takes twenty messages a minute.
pub fn telegram_drafting(bot: &Telegram, message: &Incoming) -> Option<Drafting> {
    let (chat, topic) = telegram_place(message).ok()?;
    let bot = bot.clone();
    let private = message.direct;
    Some(Drafting {
        open: Box::new(move || {
            Box::new(TelegramDraft {
                bot: bot.clone(),
                chat,
                topic,
                draft: private.then(fresh),
                message: None,
                edited: None,
                shown: String::new(),
            })
        }),
        every: if private {
            Duration::from_millis(400)
        } else {
            Duration::from_secs(3)
        },
        most: None,
        keep: private.then_some(Duration::from_secs(20)),
        first: if private { 0 } else { FIRST },
    })
}

// Slack.

/// What the bot puts on a message it works on, outside a thread.
const HOURGLASS: &str = "hourglass_flowing_sand";

/// What Slack's status line says while the agent works.
const THINKING: &str = "is thinking…";

impl Slack {
    /// The Web API, for a thread of its own.
    pub(crate) fn api(&self) -> Api {
        Api {
            client: self.client.clone(),
            base: self.base.clone(),
            auth: format!("Bearer {}", self.token),
            secret: self.token.clone(),
            service: "Slack",
        }
    }
}

/// Calls Slack's `method`: its answer, if Slack says it is ok.
fn slack(api: &Api, method: &str, body: &Value) -> Result<Value, String> {
    let value = api.call("POST", &format!("/{method}"), Some(body))?;
    if value["ok"] != true {
        return Err(format!(
            "Slack refused {method}: {}",
            value["error"].as_str().unwrap_or("no reason given")
        ));
    }
    Ok(value)
}

struct SlackTyping {
    api: Api,
    channel: String,
    thread: String,
    message: String,
    /// Whether Slack's status line is set, or was refused.
    status: Option<bool>,
    reacted: bool,
}

impl Typist for SlackTyping {
    fn show(&mut self) -> Result<Duration, String> {
        if !self.thread.is_empty() && self.status != Some(false) {
            let set = slack(
                &self.api,
                "assistant.threads.setStatus",
                &json!({"channel_id": self.channel, "thread_ts": self.thread, "status": THINKING}),
            );
            self.status = Some(set.is_ok());
            if set.is_ok() {
                // It clears after two minutes, or when the bot replies.
                return Ok(Duration::from_secs(100));
            }
        }
        if self.message.is_empty() {
            return Err("Slack named no message to show it on".into());
        }
        if !self.reacted {
            slack(
                &self.api,
                "reactions.add",
                &json!({"channel": self.channel, "timestamp": self.message, "name": HOURGLASS}),
            )?;
            self.reacted = true;
        }
        Ok(Duration::from_secs(3_600))
    }

    fn stop(&mut self) {
        if self.reacted {
            let _ = slack(
                &self.api,
                "reactions.remove",
                &json!({"channel": self.channel, "timestamp": self.message, "name": HOURGLASS}),
            );
        }
        if self.status == Some(true) {
            let _ = slack(
                &self.api,
                "assistant.threads.setStatus",
                &json!({"channel_id": self.channel, "thread_ts": self.thread, "status": ""}),
            );
        }
    }
}

/// Slack's typing: its status line in a thread, an hourglass elsewhere.
pub fn slack_typist(slack: &Slack, message: &Incoming) -> Option<Box<dyn Typist>> {
    Some(Box::new(SlackTyping {
        api: slack.api(),
        channel: message.chat.clone(),
        thread: message.thread.clone(),
        message: message.id.clone(),
        status: None,
        reacted: false,
    }))
}

/// A task of a Slack stream: the tool the agent runs.
fn slack_task(id: u32, title: &str, status: &str) -> Value {
    json!({"type": "task_update", "id": format!("t{id}"), "title": title, "status": status})
}

/// A reply in a Slack thread on Slack's own stream: the text appended as
/// it grows, and each tool the agent runs a task of the stream's timeline.
struct SlackStream {
    api: Api,
    channel: String,
    thread: String,
    user: String,
    team: Option<String>,
    /// The stream's message, once started.
    ts: Option<String>,
    /// The text appended so far.
    sent: String,
    /// Whether the text changed rather than grew: the end puts it right.
    off: bool,
    /// The task running, and how many there were.
    task: Option<(u32, String)>,
    tasks: u32,
}

impl SlackStream {
    fn post(&self, text: &str) -> Result<(), String> {
        slack(
            &self.api,
            "chat.postMessage",
            &json!({"channel": self.channel, "thread_ts": self.thread, "text": cut(text, 3_000)}),
        )
        .map(|_| ())
    }
}

impl Draft for SlackStream {
    fn show(&mut self, text: &str, status: Option<&str>) -> Result<(), String> {
        let mut chunks = Vec::new();
        let grew = match text.strip_prefix(self.sent.as_str()) {
            Some(more) if !self.off => {
                if !more.is_empty() {
                    chunks.push(json!({"type": "markdown_text", "text": more}));
                }
                true
            }
            _ => {
                self.off = true;
                false
            }
        };
        if self.task.as_ref().map(|(_, title)| title.as_str()) != status {
            if let Some((id, title)) = self.task.take() {
                chunks.push(slack_task(id, &title, "complete"));
            }
            if let Some(status) = status {
                self.tasks += 1;
                chunks.push(slack_task(self.tasks, status, "in_progress"));
                self.task = Some((self.tasks, status.to_string()));
            }
        }
        if chunks.is_empty() {
            return Ok(());
        }
        match &self.ts {
            None => {
                let mut body = json!({"channel": self.channel, "thread_ts": self.thread,
                    "recipient_user_id": self.user, "task_display_mode": "timeline",
                    "chunks": chunks});
                if let Some(team) = &self.team {
                    body["recipient_team_id"] = json!(team);
                }
                let started = slack(&self.api, "chat.startStream", &body)?;
                self.ts = Some(
                    started["ts"]
                        .as_str()
                        .ok_or("Slack started no stream")?
                        .to_string(),
                );
            }
            Some(ts) => {
                slack(
                    &self.api,
                    "chat.appendStream",
                    &json!({"channel": self.channel, "ts": ts, "chunks": chunks}),
                )?;
            }
        }
        if grew {
            self.sent = text.to_string();
        }
        Ok(())
    }

    fn finish(&mut self, text: &str) -> Result<(), String> {
        let Some(ts) = self.ts.clone() else {
            return self.post(text);
        };
        let mut chunks = Vec::new();
        if let Some((id, title)) = self.task.take() {
            chunks.push(slack_task(id, &title, "complete"));
        }
        let rest = text
            .strip_prefix(self.sent.as_str())
            .filter(|_| !self.off && text.chars().count() <= 12_000);
        if let Some(more) = rest.filter(|more| !more.is_empty()) {
            chunks.push(json!({"type": "markdown_text", "text": more}));
        }
        let mut body = json!({"channel": self.channel, "ts": ts});
        if !chunks.is_empty() {
            body["chunks"] = json!(chunks);
        }
        slack(&self.api, "chat.stopStream", &body)?;
        if rest.is_none() {
            // What streamed was not the start of the reply: the message is
            // the reply now.
            slack(
                &self.api,
                "chat.update",
                &json!({"channel": self.channel, "ts": ts, "text": cut(text, 4_000)}),
            )?;
        }
        Ok(())
    }
}

/// A Slack reply as a message edited, outside a thread.
struct SlackEdit {
    api: Api,
    channel: String,
    ts: Option<String>,
    shown: String,
}

impl SlackEdit {
    fn put(&mut self, text: &str) -> Result<(), String> {
        if self.ts.is_some() && self.shown == text {
            return Ok(());
        }
        self.shown = text.to_string();
        let text = cut(text, 3_000);
        match &self.ts {
            Some(ts) => slack(
                &self.api,
                "chat.update",
                &json!({"channel": self.channel, "ts": ts, "text": text}),
            )
            .map(|_| ()),
            None => {
                let posted = slack(
                    &self.api,
                    "chat.postMessage",
                    &json!({"channel": self.channel, "text": text}),
                )?;
                self.ts = posted["ts"].as_str().map(str::to_string);
                Ok(())
            }
        }
    }
}

impl Draft for SlackEdit {
    fn show(&mut self, text: &str, status: Option<&str>) -> Result<(), String> {
        self.put(&with_status(text, status))
    }

    fn finish(&mut self, text: &str) -> Result<(), String> {
        self.put(text)
    }
}

/// How a Slack reply shows as it is written: Slack's stream in a thread
/// (it streams nowhere else), a message edited elsewhere (`chat.update`
/// takes about fifty a minute).
pub fn slack_drafting(slack: &Slack, message: &Incoming) -> Option<Drafting> {
    let api = slack.api();
    let channel = message.chat.clone();
    if message.thread.is_empty() {
        return Some(Drafting {
            open: Box::new(move || {
                Box::new(SlackEdit {
                    api: api.clone(),
                    channel: channel.clone(),
                    ts: None,
                    shown: String::new(),
                })
            }),
            every: Duration::from_millis(1_500),
            most: None,
            keep: None,
            first: FIRST,
        });
    }
    let (thread, user, team) = (message.thread.clone(), message.from.clone(), slack.team());
    Some(Drafting {
        open: Box::new(move || {
            Box::new(SlackStream {
                api: api.clone(),
                channel: channel.clone(),
                thread: thread.clone(),
                user: user.clone(),
                team: team.clone(),
                ts: None,
                sent: String::new(),
                off: false,
                task: None,
                tasks: 0,
            })
        }),
        every: Duration::from_millis(500),
        most: None,
        keep: None,
        first: 0,
    })
}

/// Sends `text` to the chat of `message`, in its thread if it is in one.
pub fn slack_reply(slack: &Slack, message: &Incoming, text: &str) -> Result<(), String> {
    let mut body = json!({"channel": message.chat, "text": cut(text, 3_000)});
    if !message.thread.is_empty() {
        body["thread_ts"] = json!(message.thread);
    }
    self::slack(&slack.api(), "chat.postMessage", &body).map(|_| ())
}

// Discord.

impl crate::messengers::Discord {
    /// The HTTP API, for a thread of its own.
    pub(crate) fn api(&self) -> Api {
        Api {
            client: self.client.clone(),
            base: self.base.clone(),
            auth: format!("Bot {}", self.token),
            secret: self.token.clone(),
            service: "Discord",
        }
    }
}

struct DiscordTyping {
    api: Api,
    channel: String,
}

impl Typist for DiscordTyping {
    fn show(&mut self) -> Result<Duration, String> {
        self.api.call(
            "POST",
            &format!(
                "/channels/{}/typing",
                crate::messengers::segment(&self.channel)
            ),
            None,
        )?;
        Ok(Duration::from_secs(10))
    }
}

/// Discord's typing indicator in the message's channel.
pub fn discord_typist(
    discord: &crate::messengers::Discord,
    message: &Incoming,
) -> Option<Box<dyn Typist>> {
    Some(Box::new(DiscordTyping {
        api: discord.api(),
        channel: message.chat.clone(),
    }))
}

/// A Discord reply as a message edited.
struct DiscordEdit {
    api: Api,
    channel: String,
    message: Option<String>,
    shown: String,
}

impl DiscordEdit {
    fn put(&mut self, text: &str) -> Result<(), String> {
        if self.message.is_some() && self.shown == text {
            return Ok(());
        }
        self.shown = text.to_string();
        let content = json!({"content": cut(text, 2_000)});
        let channel = crate::messengers::segment(&self.channel);
        match &self.message {
            Some(id) => self
                .api
                .call(
                    "PATCH",
                    &format!(
                        "/channels/{channel}/messages/{}",
                        crate::messengers::segment(id)
                    ),
                    Some(&content),
                )
                .map(|_| ()),
            None => {
                let posted = self.api.call(
                    "POST",
                    &format!("/channels/{channel}/messages"),
                    Some(&content),
                )?;
                self.message = posted["id"].as_str().map(str::to_string);
                Ok(())
            }
        }
    }
}

impl Draft for DiscordEdit {
    fn show(&mut self, text: &str, status: Option<&str>) -> Result<(), String> {
        self.put(&with_status(text, status))
    }

    fn finish(&mut self, text: &str) -> Result<(), String> {
        self.put(text)
    }
}

/// How a Discord reply shows as it is written: a message edited at most
/// about once a second.
pub fn discord_drafting(
    discord: &crate::messengers::Discord,
    message: &Incoming,
) -> Option<Drafting> {
    let (api, channel) = (discord.api(), message.chat.clone());
    Some(Drafting {
        open: Box::new(move || {
            Box::new(DiscordEdit {
                api: api.clone(),
                channel: channel.clone(),
                message: None,
                shown: String::new(),
            })
        }),
        every: Duration::from_millis(1_200),
        most: None,
        keep: None,
        first: FIRST,
    })
}

// Matrix.

/// What marks a Matrix message's updates as a live one (MSC4357).
const LIVE: &str = "org.matrix.msc4357.live";

impl Matrix {
    /// The client-server API, for a thread of its own.
    pub(crate) fn api(&self) -> Api {
        Api {
            client: self.client.clone(),
            base: self.base.clone(),
            auth: format!("Bearer {}", self.token),
            secret: self.token.clone(),
            service: "the homeserver",
        }
    }
}

/// A transaction id for an event sent to Matrix.
pub(crate) fn txn() -> String {
    format!("sion-{}-{}", crate::runtime::new_id(), fresh())
}

struct MatrixTyping {
    api: Api,
    room: String,
    me: String,
}

impl MatrixTyping {
    fn typing(&self, on: bool) -> Result<Value, String> {
        let mut body = json!({"typing": on});
        if on {
            body["timeout"] = json!(30_000);
        }
        self.api.call(
            "PUT",
            &format!(
                "/_matrix/client/v3/rooms/{}/typing/{}",
                crate::messengers::segment(&self.room),
                crate::messengers::segment(&self.me)
            ),
            Some(&body),
        )
    }
}

impl Typist for MatrixTyping {
    fn show(&mut self) -> Result<Duration, String> {
        self.typing(true)?;
        Ok(Duration::from_secs(25))
    }

    fn stop(&mut self) {
        let _ = self.typing(false);
    }
}

/// Matrix's typing notification, for 30 seconds at a time.
pub fn matrix_typist(matrix: &Matrix, message: &Incoming) -> Option<Box<dyn Typist>> {
    Some(Box::new(MatrixTyping {
        api: matrix.api(),
        room: message.chat.clone(),
        me: matrix.me.clone(),
    }))
}

/// A Matrix reply as a message replaced: each update an `m.replace` of the
/// first event, the whole text each time, marked live until the last.
struct MatrixEdit {
    api: Api,
    room: String,
    first: Option<String>,
}

impl MatrixEdit {
    fn put(&mut self, text: &str, live: bool) -> Result<(), String> {
        let text = cut(text, 16_000);
        let mut content = json!({"msgtype": "m.text", "body": text});
        if live {
            content[LIVE] = json!({});
        }
        let event = match &self.first {
            None => content,
            Some(first) => json!({
                "msgtype": "m.text",
                "body": format!("* {text}"),
                "m.new_content": content,
                "m.relates_to": {"rel_type": "m.replace", "event_id": first},
            }),
        };
        let sent = self.api.call(
            "PUT",
            &format!(
                "/_matrix/client/v3/rooms/{}/send/m.room.message/{}",
                crate::messengers::segment(&self.room),
                txn()
            ),
            Some(&event),
        )?;
        if self.first.is_none() {
            self.first = sent["event_id"].as_str().map(str::to_string);
        }
        Ok(())
    }
}

impl Draft for MatrixEdit {
    fn show(&mut self, text: &str, status: Option<&str>) -> Result<(), String> {
        self.put(&with_status(text, status), true)
    }

    fn finish(&mut self, text: &str) -> Result<(), String> {
        self.put(text, false)
    }
}

/// How a Matrix reply shows as it is written: every update is an event of
/// its own, which homeservers limit (Synapse: a burst of ten, then one
/// every five seconds), so at most one every four seconds, and twelve.
pub fn matrix_drafting(matrix: &Matrix, message: &Incoming) -> Option<Drafting> {
    let (api, room) = (matrix.api(), message.chat.clone());
    Some(Drafting {
        open: Box::new(move || {
            Box::new(MatrixEdit {
                api: api.clone(),
                room: room.clone(),
                first: None,
            })
        }),
        every: Duration::from_secs(4),
        most: Some(12),
        keep: None,
        first: FIRST,
    })
}

// Signal.

struct SignalTyping {
    client: sion_http::Client,
    url: String,
    recipient: String,
}

impl SignalTyping {
    fn typing(&self, method: &str) -> Result<(), String> {
        self.client
            .request(
                method,
                &self.url,
                &[("Content-Type", "application/json")],
                Some(json!({"recipient": self.recipient}).to_string().as_bytes()),
                1 << 16,
            )
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

impl Typist for SignalTyping {
    fn show(&mut self) -> Result<Duration, String> {
        self.typing("PUT")?;
        // signal-cli's lasts 15 seconds unless stopped.
        Ok(Duration::from_secs(10))
    }

    fn stop(&mut self) {
        let _ = self.typing("DELETE");
    }
}

/// Signal's typing indicator, through signal-cli-rest-api.
pub fn signal_typist(signal: &Signal, message: &Incoming) -> Option<Box<dyn Typist>> {
    Some(Box::new(SignalTyping {
        client: signal.client.clone(),
        url: format!(
            "{}/v1/typing-indicator/{}",
            signal.base,
            crate::messengers::segment(&signal.number)
        ),
        recipient: message.chat.clone(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waits_are_read_as_each_platform_gives_them() {
        assert_eq!(
            retry_after(r#"{"retry_after": 0.25, "global": false}"#),
            Duration::from_millis(250)
        );
        assert_eq!(
            retry_after(r#"{"errcode": "M_LIMIT_EXCEEDED", "retry_after_ms": 1500}"#),
            Duration::from_millis(1_500)
        );
        assert_eq!(retry_after("busy"), Duration::from_secs(1));
        assert_eq!(cut("  ", 10), "(nothing)");
        assert!(fresh() > 0);
    }

    #[test]
    fn telegram_places_are_a_chat_and_maybe_a_topic() {
        let message = |chat: &str, thread: &str| Incoming {
            id: "7".into(),
            thread: thread.into(),
            chat: chat.into(),
            from: "1".into(),
            text: "hi".into(),
            voice: None,
            direct: true,
            mentions: Vec::new(),
            files: Vec::new(),
        };
        assert_eq!(telegram_place(&message("-100", "")), Ok((-100, None)));
        assert_eq!(telegram_place(&message("42", "9")), Ok((42, Some(9))));
        assert!(telegram_place(&message("not a chat", "")).is_err());
        assert_eq!(
            telegram_body(5, Some(9), json!({"action": "typing"})),
            json!({"chat_id": 5, "message_thread_id": 9, "action": "typing"})
        );
    }
}
