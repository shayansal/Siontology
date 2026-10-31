//! SimpleX Chat, through the WebSocket API of its terminal client,
//! `simplex-chat -p 5225`, run on this machine with the bot's profile: the
//! API has no authentication, so the client listens on localhost only,
//! and the bridge must be its one program (the client gives each event to
//! one connection). What people write comes as `newChatItems` events, and
//! replies go as `/_send` commands; a reply is shown as it is written as
//! a SimpleX live message (`live=on`), updated until it is done.
//!
//! A chat is `@<contact id>` for a contact and `#<group id>` for a group,
//! as the client numbers them; in a group, the bot answers what mentions
//! it. The profile's address, and accepting whoever asks to connect, are
//! set up in the client (`/address`, `/auto_accept on`): the bridge
//! answers only the chats it is let into, or pairs them. `SIMPLEX_URL`
//! names the API, `ws://127.0.0.1:5225` by default.

use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde_json::{json, Value};
use sion_http::ws::WebSocket;
use sion_http::HttpError;

use crate::live::{again, keep, text_of, Fault, Link, Live, Next};
use crate::messengers::{Incoming, Messenger, LIVE_ONCE_WAIT};
use crate::showing::{with_status, Draft, Drafting};

/// The client's API, unless `SIMPLEX_URL` names another.
pub const SIMPLEX_URL: &str = "ws://127.0.0.1:5225";

/// The most characters of one message: SimpleX takes about 15 kB of
/// text, and a character may be four bytes.
const SIMPLEX_TEXT: usize = 3_500;

/// The longest event taken from the client.
const MAX_EVENT: usize = 8 << 20;

/// How long a command waits for the client's answer.
const ANSWER_WAIT: Duration = Duration::from_secs(30);

/// A command for the connection, and where its answer goes.
struct Command {
    cmd: String,
    answer: Sender<Result<Value, String>>,
    /// Set when nobody waits for it any more: not sent, if not yet.
    dropped: Arc<AtomicBool>,
}

type Queue = Arc<Mutex<VecDeque<Command>>>;

/// A SimpleX Chat profile, through its terminal client.
pub struct SimpleX {
    client: sion_http::Client,
    url: String,
    queue: Queue,
    live: OnceLock<Live>,
}

impl std::fmt::Debug for SimpleX {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SimpleX({})", self.url)
    }
}

/// Whether `chat` names a SimpleX chat: `@<contact id>` or `#<group id>`.
pub fn is_chat(chat: &str) -> bool {
    let mut bytes = chat.bytes();
    matches!(bytes.next(), Some(b'@' | b'#')) && chat.len() > 1 && bytes.all(|b| b.is_ascii_digit())
}

/// A text message as SimpleX composes one.
fn composed(text: &str) -> Value {
    let mut text: String = text.chars().take(SIMPLEX_TEXT).collect();
    if text.is_empty() {
        text = "(nothing)".into();
    }
    json!({"msgContent": {"type": "text", "text": text}, "mentions": {}})
}

/// The response or event a message from the client carries, or the error
/// it reports: `resp` tagged by its `type`, as the API has it (and its
/// `Right`/`Left` form, as some versions wrap it).
fn response(message: &Value) -> Result<&Value, String> {
    let resp = &message["resp"];
    let resp = if resp["Right"].is_object() {
        &resp["Right"]
    } else if !resp["Left"].is_null() {
        return Err(said_error(&resp["Left"]));
    } else {
        resp
    };
    match resp["type"].as_str() {
        Some("chatCmdError" | "chatError") => Err(said_error(resp)),
        _ => Ok(resp),
    }
}

fn said_error(error: &Value) -> String {
    let text = error.to_string();
    let short: String = text.chars().take(300).collect();
    format!("SimpleX refused it: {short}")
}

/// The messages of a `newChatItems` event that others wrote: text, with
/// what is not text named; in a group, the names it mentions (the bot's
/// own, `me`, when SimpleX says the bot is mentioned).
pub fn simplex_messages(resp: &Value, me: Option<&str>) -> Vec<Incoming> {
    if resp["type"] != "newChatItems" {
        return Vec::new();
    }
    resp["chatItems"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let info = &item["chatInfo"];
            let chat_item = &item["chatItem"];
            let direction = &chat_item["chatDir"];
            let content = &chat_item["content"];
            if content["type"] != "rcvMsgContent" {
                return None;
            }
            let id = chat_item["meta"]["itemId"].as_i64()?;
            let (chat, from, direct) = match (info["type"].as_str()?, direction["type"].as_str()?) {
                ("direct", "directRcv") => {
                    let contact = info["contact"]["contactId"].as_i64()?;
                    (format!("@{contact}"), format!("@{contact}"), true)
                }
                ("group", "groupRcv") => {
                    let group = info["groupInfo"]["groupId"].as_i64()?;
                    let member = &direction["groupMember"];
                    // A member who is also a contact is named as one.
                    let from = match member["memberContactId"].as_i64() {
                        Some(contact) => format!("@{contact}"),
                        None => format!("member:{}", member["groupMemberId"].as_i64()?),
                    };
                    (format!("#{group}"), from, false)
                }
                _ => return None,
            };
            let said = &content["msgContent"];
            let mut text = said["text"].as_str().unwrap_or_default().trim().to_string();
            let unfetched = match said["type"].as_str() {
                Some("text" | "link") | None => None,
                Some("image") => Some("(a picture, not fetched)"),
                Some("video") => Some("(a video, not fetched)"),
                Some("voice") => Some("(a voice message, not heard)"),
                Some("file") => Some("(a file, not fetched)"),
                Some(_) => Some("(a message the bridge does not read)"),
            };
            if let Some(unfetched) = unfetched {
                text = format!("{text}\n{unfetched}").trim().to_string();
            }
            if text.is_empty() {
                return None;
            }
            let mut mentions: Vec<String> = chat_item["mentions"]
                .as_object()
                .map(|named| named.keys().cloned().collect())
                .unwrap_or_default();
            if let (Some(me), true) = (me, chat_item["meta"]["userMention"] == true) {
                mentions.push(me.to_string());
            }
            if let Some(me) = me {
                if text.contains(&format!("@{me}")) || text.contains(&format!("@'{me}'")) {
                    mentions.push(me.to_string());
                }
            }
            Some(Incoming {
                id: format!("{chat}/{id}"),
                thread: String::new(),
                chat,
                from,
                text,
                voice: None,
                direct,
                mentions,
                files: Vec::new(),
            })
        })
        .collect()
}

impl SimpleX {
    /// The client at `SIMPLEX_URL`.
    pub fn from_env() -> Result<SimpleX, String> {
        let url = crate::webhooks::env("SIMPLEX_URL").unwrap_or_else(|| SIMPLEX_URL.into());
        if !url.starts_with("ws://") && !url.starts_with("wss://") {
            return Err(format!("SIMPLEX_URL is {url}: a ws:// address"));
        }
        Ok(SimpleX {
            client: crate::webhooks::client()?,
            url,
            queue: Arc::new(Mutex::new(VecDeque::new())),
            live: OnceLock::new(),
        })
    }

    fn live(&self) -> &Live {
        self.live.get_or_init(|| {
            let (client, url, queue) = (self.client.clone(), self.url.clone(), self.queue.clone());
            Live::start("simplex", move |link| {
                keep("simplex", &link, |link, _| {
                    session(&client, &url, &queue, link)
                });
            })
        })
    }

    /// Sends `cmd` to the client and waits for its answer.
    fn command(&self, cmd: String) -> Result<Value, String> {
        command(&self.queue, cmd, || {
            self.live();
        })
    }
}

fn command(queue: &Queue, cmd: String, start: impl FnOnce()) -> Result<Value, String> {
    start();
    let (answer, answered) = mpsc::channel();
    let dropped = Arc::new(AtomicBool::new(false));
    queue
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push_back(Command {
            cmd,
            answer,
            dropped: dropped.clone(),
        });
    match answered.recv_timeout(ANSWER_WAIT) {
        Ok(answer) => answer,
        Err(RecvTimeoutError::Timeout) => {
            dropped.store(true, Ordering::SeqCst);
            Err("SimpleX did not answer in 30 s (is `simplex-chat -p` running?)".into())
        }
        Err(RecvTimeoutError::Disconnected) => {
            Err("the connection to SimpleX closed before it answered".into())
        }
    }
}

/// One connection to the client: who the bot is, then its events, with
/// the commands waiting sent between them.
fn session(
    client: &sion_http::Client,
    url: &str,
    queue: &Queue,
    link: &Link,
) -> Result<Next, Fault> {
    let mut socket = WebSocket::connect(client, url, &[], MAX_EVENT)
        .map_err(|e| again(format!("{e} (is `simplex-chat -p` running?)")))?;
    socket
        .send_text(&json!({"corrId": "me", "cmd": "/user"}).to_string())
        .map_err(again)?;
    let mut waiting: BTreeMap<String, Sender<Result<Value, String>>> = BTreeMap::new();
    let mut sent = 0u64;
    loop {
        if link.stopped() {
            socket.close(1000, "stopped");
            return Ok(Next::Stop);
        }
        let commands: Vec<Command> = queue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .drain(..)
            .collect();
        for command in commands {
            if command.dropped.load(Ordering::SeqCst) {
                continue;
            }
            sent += 1;
            let corr = format!("sion-{sent}");
            match socket.send_text(&json!({"corrId": corr, "cmd": command.cmd}).to_string()) {
                Ok(()) => {
                    waiting.insert(corr, command.answer);
                }
                Err(e) => {
                    let _ = command.answer.send(Err(e.to_string()));
                    return Err(again(e));
                }
            }
        }
        let message = match text_of(socket.receive(Duration::from_millis(200))) {
            Ok(Some(message)) => message,
            Ok(None) => continue,
            Err(HttpError::Closed { .. }) => return Err(again("SimpleX closed the connection")),
            Err(e) => return Err(again(e)),
        };
        match message["corrId"].as_str() {
            Some("me") => {
                let user = response(&message).map_err(Fault::Fatal)?;
                let name = user["user"]["localDisplayName"]
                    .as_str()
                    .or(user["user"]["profile"]["displayName"].as_str())
                    .ok_or_else(|| {
                        Fault::Fatal(
                            "SimpleX has no active profile: create one in the client first".into(),
                        )
                    })?;
                link.set_me(vec![name.to_string()]);
            }
            Some(corr) => {
                if let Some(answer) = waiting.remove(corr) {
                    let _ = answer.send(response(&message).cloned());
                }
            }
            None => {
                let Ok(event) = response(&message) else {
                    continue;
                };
                for incoming in simplex_messages(event, link.known_me().as_deref()) {
                    if !link.pass(incoming) {
                        socket.close(1000, "stopped");
                        return Ok(Next::Stop);
                    }
                }
            }
        }
    }
}

/// A reply shown as a SimpleX live message, updated as it grows.
struct LiveMessage {
    queue: Queue,
    chat: String,
    item: Option<i64>,
    shown: String,
}

impl LiveMessage {
    fn put(&mut self, text: &str, live: bool) -> Result<(), String> {
        if self.item.is_some() && self.shown == text && live {
            return Ok(());
        }
        let flag = if live { " live=on" } else { "" };
        let answer = match self.item {
            None => command(
                &self.queue,
                format!(
                    "/_send {}{flag} json {}",
                    self.chat,
                    json!([composed(text)])
                ),
                || {},
            )?,
            Some(item) => command(
                &self.queue,
                format!(
                    "/_update item {} {item}{flag} json {}",
                    self.chat,
                    composed(text)
                ),
                || {},
            )?,
        };
        if self.item.is_none() {
            self.item = answer["chatItems"][0]["chatItem"]["meta"]["itemId"].as_i64();
            if self.item.is_none() {
                return Err("SimpleX did not say which message it sent".into());
            }
        }
        self.shown = text.to_string();
        Ok(())
    }
}

impl Draft for LiveMessage {
    fn show(&mut self, text: &str, status: Option<&str>) -> Result<(), String> {
        self.put(&with_status(text, status), true)
    }

    fn finish(&mut self, text: &str) -> Result<(), String> {
        self.put(text, false)
    }
}

impl Messenger for SimpleX {
    fn name(&self) -> &'static str {
        "simplex"
    }

    fn receive(&self, cursor: &str, wait: u64) -> Result<(Vec<Incoming>, String), String> {
        Ok((self.live().receive(wait)?, cursor.to_string()))
    }

    fn send(&self, chat: &str, text: &str) -> Result<(), String> {
        if !is_chat(chat) {
            return Err(format!(
                "{chat} is not a SimpleX chat: @<contact id> or #<group id>"
            ));
        }
        self.command(format!("/_send {chat} json {}", json!([composed(text)])))
            .map(|_| ())
    }

    fn room(&self) -> usize {
        SIMPLEX_TEXT
    }

    fn drafting(&self, message: &Incoming) -> Option<Drafting> {
        if !is_chat(&message.chat) {
            return None;
        }
        self.live();
        let (queue, chat) = (self.queue.clone(), message.chat.clone());
        Some(Drafting {
            open: Box::new(move || {
                Box::new(LiveMessage {
                    queue: queue.clone(),
                    chat: chat.clone(),
                    item: None,
                    shown: String::new(),
                })
            }),
            // Each update is a message on SimpleX's servers.
            every: Duration::from_secs(2),
            most: None,
            keep: None,
            first: crate::drafts::FIRST,
        })
    }

    fn long_polls(&self) -> bool {
        true
    }

    fn sees_strangers(&self) -> bool {
        true
    }

    fn once_wait(&self) -> u64 {
        LIVE_ONCE_WAIT
    }

    fn me(&self) -> Result<Vec<String>, String> {
        self.live().me(Duration::from_secs(10))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chats_are_contacts_or_groups_by_number() {
        assert!(is_chat("@3") && is_chat("#12"));
        assert!(!is_chat("@") && !is_chat("3") && !is_chat("@ann") && !is_chat("#1 2"));
    }

    #[test]
    fn new_items_give_what_others_wrote() {
        let event = json!({"type": "newChatItems", "chatItems": [
            {"chatInfo": {"type": "direct", "contact": {"contactId": 3, "localDisplayName": "ann"}},
             "chatItem": {"chatDir": {"type": "directRcv"}, "meta": {"itemId": 10},
                          "content": {"type": "rcvMsgContent", "msgContent": {"type": "text", "text": "How much is left?"}}}},
            {"chatInfo": {"type": "direct", "contact": {"contactId": 3}},
             "chatItem": {"chatDir": {"type": "directSnd"}, "meta": {"itemId": 11},
                          "content": {"type": "sndMsgContent", "msgContent": {"type": "text", "text": "mine"}}}},
            {"chatInfo": {"type": "group", "groupInfo": {"groupId": 5}},
             "chatItem": {"chatDir": {"type": "groupRcv", "groupMember": {"groupMemberId": 8, "memberId": "x", "localDisplayName": "bo"}},
                          "meta": {"itemId": 12, "userMention": true},
                          "content": {"type": "rcvMsgContent", "msgContent": {"type": "image", "text": "this?", "image": "data:"}}}},
            {"chatInfo": {"type": "group", "groupInfo": {"groupId": 5}},
             "chatItem": {"chatDir": {"type": "groupRcv", "groupMember": {"groupMemberId": 9, "memberContactId": 4}},
                          "meta": {"itemId": 13},
                          "content": {"type": "rcvMsgContent", "msgContent": {"type": "text", "text": "@sion and you?"}}}}
        ]});
        let messages = simplex_messages(&event, Some("sion"));
        assert_eq!(messages.len(), 3, "the bot's own left out");
        assert_eq!(
            (
                messages[0].chat.as_str(),
                messages[0].from.as_str(),
                messages[0].direct
            ),
            ("@3", "@3", true)
        );
        assert_eq!(messages[0].id, "@3/10");
        assert_eq!(
            (
                messages[1].chat.as_str(),
                messages[1].from.as_str(),
                messages[1].direct
            ),
            ("#5", "member:8", false)
        );
        assert_eq!(messages[1].text, "this?\n(a picture, not fetched)");
        assert_eq!(messages[1].mentions, ["sion"]);
        assert_eq!(messages[2].from, "@4");
        assert_eq!(messages[2].mentions, ["sion"]);
        assert!(simplex_messages(&json!({"type": "contactConnected"}), None).is_empty());
    }

    #[test]
    fn answers_and_errors_are_told_apart() {
        let ok = json!({"corrId": "1", "resp": {"type": "newChatItems", "chatItems": []}});
        assert_eq!(response(&ok).unwrap()["type"], "newChatItems");
        let wrapped = json!({"resp": {"Right": {"type": "activeUser", "user": {}}}});
        assert_eq!(response(&wrapped).unwrap()["type"], "activeUser");
        let refused = json!({"corrId": "2", "resp": {"type": "chatCmdError", "chatError": {"type": "error"}}});
        assert!(response(&refused).unwrap_err().contains("chatCmdError"));
        assert!(response(&json!({"resp": {"Left": {"type": "x"}}})).is_err());
    }
}
