//! Live connections to messengers that push what is said: Discord's
//! gateway and Slack's Socket Mode, both WebSockets ([`sion_http::ws`]). A
//! thread of its own keeps each connection (heartbeats, acknowledgements,
//! reconnecting and resuming) and hands the bridge the messages as they
//! come; a token the service refuses ends it, and the bridge says why.
//!
//! What is said while no bridge listens is not passed on later: these
//! services keep no queue for a bot.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use sion_http::ws::{Message, WebSocket};
use sion_http::HttpError;

use crate::messengers::{Incoming, Note};

/// The longest event taken from a connection, in bytes.
const MAX_EVENT: usize = 8 << 20;

/// How long a batch keeps gathering messages that follow the first
/// closely, and for how long at most.
const GATHER: Duration = Duration::from_millis(200);
const GATHER_AT_MOST: Duration = Duration::from_secs(1);

/// Messages recently passed on, by key, so a service's repeats are not
/// answered twice.
const REMEMBERED: usize = 256;

/// What a connection's thread hands on.
enum Event {
    Message(Incoming),
    Failed(String),
}

/// A connection kept by a thread of its own, closed when dropped.
pub struct Live {
    events: Mutex<Receiver<Event>>,
    me: Arc<OnceLock<Vec<String>>>,
    stop: Arc<AtomicBool>,
    failed: Mutex<Option<String>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

/// How long dropping a connection waits for its thread to close it.
const CLOSING: Duration = Duration::from_secs(3);

impl std::fmt::Debug for Live {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Live").finish_non_exhaustive()
    }
}

impl Live {
    /// Runs `keep` on a thread named after `name`, with the link it hands
    /// messages through.
    pub fn start(name: &str, keep: impl FnOnce(Link) + Send + 'static) -> Live {
        let (sender, events) = channel();
        let me = Arc::new(OnceLock::new());
        let stop = Arc::new(AtomicBool::new(false));
        let link = Link {
            sender,
            me: me.clone(),
            stop: stop.clone(),
        };
        // A thread that cannot start leaves its link dropped: the bridge
        // sees the connection stop at once.
        let thread = std::thread::Builder::new()
            .name(format!("sion-{name}"))
            .spawn(move || keep(link))
            .ok();
        Live {
            events: Mutex::new(events),
            me,
            stop,
            failed: Mutex::new(None),
            thread,
        }
    }

    /// The messages that come within `wait` seconds: those waiting, or
    /// the first to come and those that follow it closely.
    pub fn receive(&self, wait: u64) -> Result<Vec<Incoming>, String> {
        if let Some(why) = self.failed.lock().map_err(|_| "poisoned")?.take() {
            return Err(why);
        }
        let events = self.events.lock().map_err(|_| "poisoned")?;
        let mut out = Vec::new();
        let mut next = match events.recv_timeout(Duration::from_secs(wait)) {
            Ok(event) => Some(event),
            Err(RecvTimeoutError::Timeout) => return Ok(out),
            Err(RecvTimeoutError::Disconnected) => return Err("the connection stopped".into()),
        };
        let started = Instant::now();
        while let Some(event) = next {
            match event {
                Event::Message(message) => out.push(message),
                Event::Failed(why) if out.is_empty() => return Err(why),
                Event::Failed(why) => {
                    *self.failed.lock().map_err(|_| "poisoned")? = Some(why);
                    break;
                }
            }
            next = if started.elapsed() < GATHER_AT_MOST {
                events.recv_timeout(GATHER).ok()
            } else {
                events.try_recv().ok()
            };
        }
        Ok(out)
    }

    /// The names messages give the bot, once the connection has learned
    /// them (waiting up to `wait`).
    pub fn me(&self, wait: Duration) -> Result<Vec<String>, String> {
        let deadline = Instant::now() + wait;
        loop {
            if let Some(names) = self.me.get() {
                return Ok(names.clone());
            }
            if Instant::now() >= deadline {
                return Err("the connection has not said who the bot is yet".into());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // The thread notices within a second and closes the connection,
        // so the service knows the bot has gone (Discord shows it offline).
        if let Some(thread) = self.thread.take() {
            let until = Instant::now() + CLOSING;
            while !thread.is_finished() && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }
}

/// What a connection's thread holds: where its messages go.
pub struct Link {
    sender: Sender<Event>,
    me: Arc<OnceLock<Vec<String>>>,
    stop: Arc<AtomicBool>,
}

impl std::fmt::Debug for Link {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Link").finish_non_exhaustive()
    }
}

impl Link {
    fn stopped(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }

    /// Hands on `message`: false once nobody listens.
    fn pass(&self, message: Incoming) -> bool {
        self.sender.send(Event::Message(message)).is_ok()
    }

    fn fail(&self, why: String) {
        let _ = self.sender.send(Event::Failed(why));
    }

    fn known_me(&self) -> Option<String> {
        self.me.get().and_then(|names| names.first().cloned())
    }

    /// Waits `pause`, or less if stopped meanwhile.
    fn rest(&self, pause: Duration) {
        let until = Instant::now() + pause;
        while !self.stopped() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

/// Why a connection ended badly.
enum Fault {
    /// The service refused: no use trying again.
    Fatal(String),
    /// Worth trying again.
    Again(String),
}

/// What comes after a connection ends.
enum Next {
    /// Connect again, resuming where the service allows.
    Resume,
    /// Connect again as new.
    Identify,
    Stop,
}

fn again(why: impl std::fmt::Display) -> Fault {
    Fault::Again(why.to_string())
}

/// The pause before the `failures`th attempt again: 1, 2, 4… seconds, at
/// most a minute.
fn backoff(failures: u32) -> Duration {
    Duration::from_secs(1u64 << failures.min(6)).min(Duration::from_secs(60))
}

/// Keeps a connection with `session` until stopped or refused, trying
/// again after faults.
fn keep(name: &str, link: &Link, mut session: impl FnMut(&Link, bool) -> Result<Next, Fault>) {
    let mut failures = 0;
    let mut resume = false;
    while !link.stopped() {
        match session(link, resume) {
            Ok(Next::Stop) => return,
            Ok(Next::Resume) => {
                failures = 0;
                resume = true;
            }
            Ok(Next::Identify) => {
                failures = 0;
                resume = false;
            }
            Err(Fault::Fatal(why)) => {
                link.fail(format!("{name}: {why}"));
                return;
            }
            Err(Fault::Again(why)) => {
                failures += 1;
                let pause = backoff(failures);
                println!("{name}: {why}; connecting again in {} s", pause.as_secs());
                link.rest(pause);
            }
        }
    }
}

fn text_of(received: Result<Option<Message>, HttpError>) -> Result<Option<Value>, HttpError> {
    match received? {
        Some(Message::Text(text)) => Ok(serde_json::from_str(&text).ok()),
        _ => Ok(None),
    }
}

/// Discord's gateway intents: servers, their messages, direct messages,
/// and what messages say (a privileged intent, turned on for the bot in
/// Discord's developer portal).
pub const DISCORD_INTENTS: u64 = 1 | (1 << 9) | (1 << 12) | (1 << 15);

/// A Discord gateway session to resume.
struct Resumable {
    id: String,
    url: String,
    seq: Option<u64>,
}

/// Keeps Discord's gateway for the bot with `token`, its HTTP API at
/// `base`.
pub fn discord(client: sion_http::Client, base: String, token: String) -> impl FnOnce(Link) {
    move |link| {
        let mut resumable: Option<Resumable> = None;
        keep("discord", &link, |link, resume| {
            if !resume {
                resumable = None;
            }
            discord_session(&client, &base, &token, &mut resumable, link)
        });
    }
}

/// The gateway's address, from the HTTP API.
fn gateway_url(client: &sion_http::Client, base: &str, token: &str) -> Result<String, Fault> {
    let auth = format!("Bot {token}");
    match client.request(
        "GET",
        &format!("{base}/gateway/bot"),
        &[("Authorization", &auth)],
        None,
        1 << 20,
    ) {
        Ok(fetched) => serde_json::from_slice::<Value>(&fetched.body)
            .ok()
            .and_then(|v| v["url"].as_str().map(str::to_string))
            .ok_or_else(|| again("Discord gave no gateway address")),
        Err(HttpError::Status { status: 401, .. }) => {
            Err(Fault::Fatal("Discord refused the bot's token".into()))
        }
        Err(e) => Err(again(e.to_string().replace(token, "<token>"))),
    }
}

/// What a close from Discord's gateway means.
fn discord_closed(code: u16, reason: &str) -> Result<Next, Fault> {
    match code {
        4004 => Err(Fault::Fatal("Discord refused the bot's token (4004)".into())),
        4014 => Err(Fault::Fatal(
            "Discord did not allow the intents (4014): turn on the message content intent for the bot in Discord's developer portal".into(),
        )),
        4010..=4013 => Err(Fault::Fatal(format!(
            "Discord closed the gateway ({code}): {reason}"
        ))),
        4007 | 4009 => Ok(Next::Identify),
        _ => Ok(Next::Resume),
    }
}

/// A message from a MESSAGE_CREATE event, unless it is a bot's or says
/// nothing; mentions of the bot are taken out of its text.
pub fn discord_message(d: &Value, me: Option<&str>) -> Option<Incoming> {
    let author = &d["author"];
    let from = author["id"].as_str()?;
    if author["bot"] == true || Some(from) == me {
        return None;
    }
    let mut text = d["content"].as_str().unwrap_or_default().to_string();
    if let Some(me) = me {
        text = text
            .replace(&format!("<@{me}>"), "")
            .replace(&format!("<@!{me}>"), "")
            .trim()
            .to_string();
    }
    // A voice message is an audio attachment.
    let voice = d["attachments"]
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
    if text.is_empty() && voice.is_none() {
        return None;
    }
    let mut mentions: Vec<String> = d["mentions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|user| user["id"].as_str().map(str::to_string))
        .collect();
    if let Some(replied) = d["referenced_message"]["author"]["id"].as_str() {
        mentions.push(replied.to_string());
    }
    Some(Incoming {
        chat: d["channel_id"].as_str()?.to_string(),
        from: from.to_string(),
        text,
        voice,
        direct: d["guild_id"].is_null(),
        mentions,
    })
}

fn discord_session(
    client: &sion_http::Client,
    base: &str,
    token: &str,
    resumable: &mut Option<Resumable>,
    link: &Link,
) -> Result<Next, Fault> {
    let url = match resumable {
        Some(session) => session.url.clone(),
        None => gateway_url(client, base, token)?,
    };
    let address = format!("{}/?v=10&encoding=json", url.trim_end_matches('/'));
    let mut socket = WebSocket::connect(client, &address, &[], MAX_EVENT).map_err(again)?;
    let hello = text_of(socket.receive(Duration::from_secs(30)))
        .map_err(again)?
        .filter(|hello| hello["op"] == 10)
        .ok_or_else(|| again("the gateway did not say hello"))?;
    let interval =
        Duration::from_millis(hello["d"]["heartbeat_interval"].as_u64().unwrap_or(41_250));
    let send = |socket: &mut WebSocket, value: Value| {
        socket
            .send_text(&value.to_string())
            .map_err(|e| again(e.to_string()))
    };
    match resumable {
        Some(session) => send(
            &mut socket,
            json!({"op": 6, "d": {"token": token, "session_id": session.id, "seq": session.seq}}),
        )?,
        None => send(
            &mut socket,
            json!({"op": 2, "d": {"token": token, "intents": DISCORD_INTENTS,
                "properties": {"os": std::env::consts::OS, "browser": "sion", "device": "sion"}}}),
        )?,
    }
    // The first heartbeat comes at a random point of the first interval.
    let mut jitter = [0u8; 1];
    let _ = sion_http::random(&mut jitter);
    let mut beat_at = Instant::now() + interval.mul_f64(f64::from(jitter[0]) / 255.0);
    let mut acknowledged = true;
    loop {
        if link.stopped() {
            socket.close(1000, "stopped");
            return Ok(Next::Stop);
        }
        if Instant::now() >= beat_at {
            if !acknowledged {
                // A connection that stopped answering: resume on another.
                socket.close(4000, "no heartbeat acknowledged");
                return Ok(Next::Resume);
            }
            let seq = resumable.as_ref().and_then(|s| s.seq);
            send(&mut socket, json!({"op": 1, "d": seq}))?;
            acknowledged = false;
            beat_at = Instant::now() + interval;
        }
        let wait = beat_at
            .saturating_duration_since(Instant::now())
            .min(Duration::from_secs(1));
        let event = match text_of(socket.receive(wait)) {
            Ok(Some(event)) => event,
            Ok(None) => continue,
            Err(HttpError::Closed { code, reason }) => return discord_closed(code, &reason),
            Err(e) => return Err(again(e)),
        };
        if let (Some(seq), Some(session)) = (event["s"].as_u64(), resumable.as_mut()) {
            session.seq = Some(seq);
        }
        match event["op"].as_u64() {
            Some(0) => match event["t"].as_str() {
                Some("READY") => {
                    let d = &event["d"];
                    *resumable = Some(Resumable {
                        id: d["session_id"].as_str().unwrap_or_default().to_string(),
                        url: d["resume_gateway_url"].as_str().unwrap_or(&url).to_string(),
                        seq: event["s"].as_u64(),
                    });
                    if let Some(me) = d["user"]["id"].as_str() {
                        let _ = link.me.set(vec![me.to_string()]);
                    }
                }
                Some("MESSAGE_CREATE") => {
                    let me = link.known_me();
                    if let Some(message) = discord_message(&event["d"], me.as_deref()) {
                        if !link.pass(message) {
                            socket.close(1000, "stopped");
                            return Ok(Next::Stop);
                        }
                    }
                }
                _ => {}
            },
            Some(1) => {
                let seq = resumable.as_ref().and_then(|s| s.seq);
                send(&mut socket, json!({"op": 1, "d": seq}))?;
            }
            Some(7) => {
                socket.close(4000, "reconnecting");
                return Ok(Next::Resume);
            }
            Some(9) => {
                socket.close(4000, "session not valid");
                if event["d"] == true {
                    return Ok(Next::Resume);
                }
                link.rest(Duration::from_secs(2));
                return Ok(Next::Identify);
            }
            Some(11) => acknowledged = true,
            _ => {}
        }
    }
}

/// The `<@U…>` mentions in Slack's text.
pub fn slack_mentions(text: &str) -> Vec<String> {
    text.split("<@")
        .skip(1)
        .filter_map(|rest| rest.split(['>', '|']).next())
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .collect()
}

/// The audio clip a Slack message shares, if it shares one: its private
/// address (fetched with the bot's token) and media type.
pub fn slack_voice(message: &Value) -> Option<Note> {
    let file = message["files"].as_array()?.iter().find(|file| {
        file["subtype"] == "slack_audio"
            || file["mimetype"]
                .as_str()
                .is_some_and(|m| m.starts_with("audio/"))
    })?;
    Some(Note {
        id: file["url_private_download"]
            .as_str()
            .or(file["url_private"].as_str())?
            .to_string(),
        media_type: file["mimetype"].as_str().map(str::to_string),
    })
}

/// Whether a Slack message is what someone said to the bot: not an edit,
/// a join or a bot's, though a shared audio clip is.
pub fn slack_said(message: &Value, voice: &Option<Note>) -> bool {
    message["bot_id"].is_null()
        && match message["subtype"].as_str() {
            None => true,
            Some("file_share") => voice.is_some(),
            Some(_) => false,
        }
}

/// A message from a Socket Mode event, keyed by its channel and time so
/// that the same message as a `message` and an `app_mention` is passed on
/// once.
pub fn slack_message(event: &Value, me: Option<&str>) -> Option<(String, Incoming)> {
    let kind = event["type"].as_str()?;
    if kind != "message" && kind != "app_mention" {
        return None;
    }
    let voice = slack_voice(event);
    if !slack_said(event, &voice) {
        return None;
    }
    let chat = event["channel"].as_str()?;
    let from = event["user"].as_str()?;
    if Some(from) == me {
        return None;
    }
    let ts = event["ts"].as_str()?;
    let raw = event["text"].as_str().unwrap_or_default();
    let mut mentions = slack_mentions(raw);
    if let Some(parent) = event["parent_user_id"].as_str() {
        mentions.push(parent.to_string());
    }
    let mut text = raw.to_string();
    if let Some(me) = me {
        if kind == "app_mention" {
            mentions.push(me.to_string());
        }
        text = text.replace(&format!("<@{me}>"), "").trim().to_string();
    }
    if text.is_empty() && voice.is_none() {
        return None;
    }
    Some((
        format!("{chat}:{ts}"),
        Incoming {
            chat: chat.to_string(),
            from: from.to_string(),
            text,
            voice,
            direct: event["channel_type"] == "im",
            mentions,
        },
    ))
}

/// Keeps a Socket Mode connection for the Slack app with `app_token`, its
/// Web API at `base`; `bot_token` says who the bot is (`auth.test`).
pub fn slack(
    client: sion_http::Client,
    base: String,
    app_token: String,
    bot_token: String,
) -> impl FnOnce(Link) {
    move |link| {
        let mut passed: VecDeque<String> = VecDeque::new();
        keep("slack", &link, |link, _| {
            slack_session(&client, &base, &app_token, &bot_token, &mut passed, link)
        });
    }
}

/// Calls Slack's Web API `method` with `token`.
fn slack_call(
    client: &sion_http::Client,
    base: &str,
    token: &str,
    method: &str,
) -> Result<Value, Fault> {
    let auth = format!("Bearer {token}");
    let fetched = client
        .post_json(
            &format!("{base}/{method}"),
            &[("Authorization", &auth)],
            b"{}",
            1 << 20,
        )
        .map_err(|e| again(e.to_string().replace(token, "<token>")))?;
    let value: Value = serde_json::from_slice(&fetched.body).map_err(|_| {
        again(format!(
            "Slack's {method} answered with something other than JSON"
        ))
    })?;
    if value["ok"] != true {
        let error = value["error"].as_str().unwrap_or("no reason given");
        let refused = matches!(
            error,
            "invalid_auth"
                | "not_authed"
                | "not_allowed_token_type"
                | "account_inactive"
                | "token_revoked"
        );
        let why = format!("Slack refused {method}: {error}");
        return Err(if refused {
            Fault::Fatal(why)
        } else {
            again(why)
        });
    }
    Ok(value)
}

fn slack_session(
    client: &sion_http::Client,
    base: &str,
    app_token: &str,
    bot_token: &str,
    passed: &mut VecDeque<String>,
    link: &Link,
) -> Result<Next, Fault> {
    if link.known_me().is_none() {
        let who = slack_call(client, base, bot_token, "auth.test")?;
        if let Some(me) = who["user_id"].as_str() {
            let _ = link.me.set(vec![me.to_string()]);
        }
    }
    let opened = slack_call(client, base, app_token, "apps.connections.open")?;
    let url = opened["url"]
        .as_str()
        .ok_or_else(|| again("Slack gave no Socket Mode address"))?;
    let mut socket = WebSocket::connect(client, url, &[], MAX_EVENT).map_err(again)?;
    loop {
        if link.stopped() {
            socket.close(1000, "stopped");
            return Ok(Next::Stop);
        }
        let envelope = match text_of(socket.receive(Duration::from_secs(1))) {
            Ok(Some(envelope)) => envelope,
            Ok(None) => continue,
            // Slack closes connections now and then: a new one is opened.
            Err(HttpError::Closed { .. }) => return Ok(Next::Resume),
            Err(e) => return Err(again(e)),
        };
        // Every envelope is acknowledged at once, or Slack sends it again.
        if let Some(id) = envelope["envelope_id"].as_str() {
            socket
                .send_text(&json!({"envelope_id": id}).to_string())
                .map_err(again)?;
        }
        match envelope["type"].as_str() {
            Some("disconnect") => {
                socket.close(1000, "refreshing");
                return Ok(Next::Resume);
            }
            Some("events_api") => {
                let me = link.known_me();
                let Some((key, message)) =
                    slack_message(&envelope["payload"]["event"], me.as_deref())
                else {
                    continue;
                };
                if passed.contains(&key) {
                    continue;
                }
                passed.push_back(key);
                if passed.len() > REMEMBERED {
                    passed.pop_front();
                }
                if !link.pass(message) {
                    socket.close(1000, "stopped");
                    return Ok(Next::Stop);
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discord_messages_are_read_from_events() {
        let event = json!({"channel_id": "900", "guild_id": "1", "author": {"id": "A1"},
            "content": "<@B7> how much is left?", "mentions": [{"id": "B7"}],
            "referenced_message": {"author": {"id": "C3"}}});
        let message = discord_message(&event, Some("B7")).unwrap();
        assert_eq!(message.text, "how much is left?");
        assert_eq!(message.mentions, ["B7", "C3"]);
        assert!(!message.direct);
        let direct = json!({"channel_id": "55", "author": {"id": "A1"}, "content": "hi"});
        assert!(discord_message(&direct, Some("B7")).unwrap().direct);
        let own = json!({"channel_id": "55", "author": {"id": "B7"}, "content": "mine"});
        assert!(discord_message(&own, Some("B7")).is_none());
        let bot =
            json!({"channel_id": "55", "author": {"id": "X", "bot": true}, "content": "beep"});
        assert!(discord_message(&bot, Some("B7")).is_none());
        let voice = json!({"channel_id": "55", "author": {"id": "A1"}, "content": "",
            "attachments": [{"url": "https://cdn/v.ogg", "content_type": "audio/ogg"}]});
        assert_eq!(
            discord_message(&voice, None).unwrap().voice.unwrap().id,
            "https://cdn/v.ogg"
        );
    }

    #[test]
    fn slack_messages_are_read_from_events() {
        assert_eq!(
            slack_mentions("<@U1> and <@U2|ann>, not <#C1>"),
            ["U1", "U2"]
        );
        let mention = json!({"type": "app_mention", "channel": "C1", "user": "U9",
            "text": "<@UBOT> budget?", "ts": "1.2", "channel_type": "channel"});
        let (key, message) = slack_message(&mention, Some("UBOT")).unwrap();
        assert_eq!(key, "C1:1.2");
        assert_eq!(message.text, "budget?");
        assert!(message.mentions.contains(&"UBOT".to_string()));
        let direct = json!({"type": "message", "channel": "D1", "user": "U9", "text": "hi",
            "ts": "1.3", "channel_type": "im"});
        assert!(slack_message(&direct, Some("UBOT")).unwrap().1.direct);
        for skipped in [
            json!({"type": "message", "channel": "C1", "user": "U9", "text": "x", "ts": "1", "subtype": "message_changed"}),
            json!({"type": "message", "channel": "C1", "bot_id": "B1", "text": "x", "ts": "1"}),
            json!({"type": "reaction_added", "user": "U9"}),
        ] {
            assert!(slack_message(&skipped, Some("UBOT")).is_none(), "{skipped}");
        }
    }

    #[test]
    fn closes_and_pauses_are_told_apart() {
        assert!(matches!(discord_closed(4004, ""), Err(Fault::Fatal(_))));
        assert!(
            matches!(discord_closed(4014, ""), Err(Fault::Fatal(why)) if why.contains("message content"))
        );
        assert!(matches!(discord_closed(4009, ""), Ok(Next::Identify)));
        assert!(matches!(discord_closed(1001, ""), Ok(Next::Resume)));
        assert_eq!(backoff(1), Duration::from_secs(2));
        assert_eq!(backoff(20), Duration::from_secs(60));
    }

    #[test]
    fn a_live_connection_gathers_what_comes_close_together() {
        let live = Live::start("test", |link| {
            for text in ["one", "two"] {
                link.pass(Incoming {
                    chat: "c".into(),
                    from: "f".into(),
                    text: text.into(),
                    voice: None,
                    direct: true,
                    mentions: Vec::new(),
                });
            }
            let _ = link.me.set(vec!["B7".into()]);
            link.rest(Duration::from_millis(500));
            link.fail("refused".into());
        });
        let batch = live.receive(5).unwrap();
        assert_eq!(
            batch.iter().map(|m| m.text.as_str()).collect::<Vec<_>>(),
            ["one", "two"]
        );
        assert_eq!(live.me(Duration::from_secs(1)).unwrap(), ["B7"]);
        assert_eq!(live.receive(5).unwrap_err(), "refused");
    }
}
