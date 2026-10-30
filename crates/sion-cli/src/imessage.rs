//! iMessage (and SMS, through a paired iPhone), through `imsg` on the Mac
//! that runs Messages (github.com/steipete/imsg): its JSON-RPC over stdio
//! (`imsg rpc`), run as a command of the bridge's, on the Mac itself or
//! through SSH from another machine.
//!
//! | Variable | What it is |
//! |---|---|
//! | `IMSG_COMMAND` | the command that runs `imsg rpc` (`imsg rpc` by default; `ssh mac imsg rpc` for a Mac elsewhere) |
//! | `IMSG_SERVICE` | how replies go: `imessage`, `sms` or `auto` (imsg's default) |
//!
//! The bridge subscribes to the messages after its place (a message's row
//! id; the first time, the latest message), so what came while it was
//! stopped is answered. A chat is named by its identifier: a handle
//! (`+15551234567`, `alice@icloud.com`) for a direct chat, `chat…` for a
//! group. Voice notes are read as imsg converts them (on the Mac the
//! bridge runs on); spoken answers go as audio messages. imsg needs Full
//! Disk Access to read Messages' database, and Automation (Messages) to
//! send.

use std::collections::VecDeque;
use std::io::{BufRead, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::messengers::{Incoming, Messenger, Note};
use crate::voice::{Audio, MAX_HEARD};

/// How long imsg has to answer a request.
const ANSWER: Duration = Duration::from_secs(60);

/// How long a batch waits for more messages after one came.
const GATHER: Duration = Duration::from_millis(400);

/// How long one batch (`--once`) waits: imsg sends what came since the
/// bridge's place at once.
const ONCE_WAIT: u64 = 10;

/// A running `imsg rpc`: requests in, answers and notifications out.
struct Rpc {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<Result<Value, String>>,
    /// Notifications read while waiting for an answer.
    queue: VecDeque<Value>,
    next: u64,
}

impl Drop for Rpc {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Rpc {
    fn start(command: &str) -> Result<Rpc, String> {
        let mut child = Command::new("sh")
            .arg("-c")
            .arg(command)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| format!("imessage: {command}: {e}"))?;
        let stdout = child.stdout.take().ok_or("imessage: no output from imsg")?;
        let stdin = child.stdin.take().ok_or("imessage: no input to imsg")?;
        let (send, lines) = channel();
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(stdout).lines() {
                match line {
                    // What is not JSON is not imsg's answer: let be.
                    Ok(line) => {
                        if let Ok(value) = serde_json::from_str::<Value>(&line) {
                            if send.send(Ok(value)).is_err() {
                                return;
                            }
                        }
                    }
                    Err(e) => {
                        let _ = send.send(Err(format!("imessage: reading imsg: {e}")));
                        return;
                    }
                }
            }
            let _ = send.send(Err("imessage: imsg ended".into()));
        });
        Ok(Rpc {
            child,
            stdin,
            lines,
            queue: VecDeque::new(),
            next: 0,
        })
    }

    /// Calls `method`, keeping the notifications that come meanwhile.
    fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.next += 1;
        let id = self.next;
        let request = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        writeln!(self.stdin, "{request}")
            .and_then(|()| self.stdin.flush())
            .map_err(|e| format!("imessage: writing to imsg: {e}"))?;
        let until = Instant::now() + ANSWER;
        loop {
            let left = until.saturating_duration_since(Instant::now());
            let value = match self.lines.recv_timeout(left) {
                Ok(read) => read?,
                Err(RecvTimeoutError::Timeout) => {
                    return Err(format!("imessage: imsg did not answer {method} in time"))
                }
                Err(RecvTimeoutError::Disconnected) => return Err("imessage: imsg ended".into()),
            };
            if value["id"] == id {
                if let Some(error) = value.get("error") {
                    return Err(format!(
                        "imessage: imsg refused {method}: {} ({})",
                        error["message"].as_str().unwrap_or("no reason given"),
                        error["code"]
                    ));
                }
                return Ok(value["result"].clone());
            }
            if value.get("method").is_some() {
                self.queue.push_back(value);
            }
        }
    }

    /// The next notification, waiting up to `wait`.
    fn notification(&mut self, wait: Duration) -> Result<Option<Value>, String> {
        if let Some(value) = self.queue.pop_front() {
            return Ok(Some(value));
        }
        match self.lines.recv_timeout(wait) {
            Ok(read) => {
                let value = read?;
                let notification = value.get("method").is_some();
                Ok(notification.then_some(value))
            }
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => Err("imessage: imsg ended".into()),
        }
    }
}

/// Messages, through imsg (see the module's documentation).
pub struct IMessage {
    command: String,
    service: Option<String>,
    rpc: Mutex<Option<Rpc>>,
}

impl std::fmt::Debug for IMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "IMessage({})", self.command)
    }
}

/// Whether an attachment is audio (a voice note: Messages keeps them as
/// Core Audio files).
fn audio(attachment: &Value) -> bool {
    attachment["missing"] != true
        && (attachment["mime_type"]
            .as_str()
            .is_some_and(|t| t.starts_with("audio/"))
            || attachment["uti"] == "com.apple.coreaudio-format")
}

/// A message imsg gave, as the bridge takes it; `None` for the agent's own
/// messages, reactions, and messages with nothing in them.
pub fn imessage_message(message: &Value) -> Option<Incoming> {
    if message["is_from_me"] == true || message["is_reaction"] == true {
        return None;
    }
    let chat = message["chat_identifier"]
        .as_str()
        .filter(|c| !c.is_empty())?;
    let attachments = message["attachments"].as_array();
    let voice = attachments
        .into_iter()
        .flatten()
        .find(|a| audio(a))
        .and_then(|a| {
            Some(Note {
                id: message["id"].as_u64()?.to_string(),
                media_type: a["mime_type"].as_str().map(str::to_string),
            })
        });
    // Messages marks where an attachment goes with U+FFFC.
    let mut text = message["text"]
        .as_str()
        .unwrap_or_default()
        .replace('\u{FFFC}', "")
        .trim()
        .to_string();
    for attachment in attachments.into_iter().flatten().filter(|a| !audio(a)) {
        let name = attachment["transfer_name"]
            .as_str()
            .or(attachment["filename"].as_str())
            .unwrap_or("a file");
        text.push_str(&format!("\n(attached, not read: {name})"));
    }
    let text = text.trim().to_string();
    if text.is_empty() && voice.is_none() {
        return None;
    }
    Some(Incoming {
        chat: chat.to_string(),
        from: message["sender"].as_str().unwrap_or(chat).to_string(),
        text,
        voice,
        direct: message["is_group"] != true,
        mentions: Vec::new(),
    })
}

impl IMessage {
    /// Messages through `IMSG_COMMAND` (`imsg rpc` by default).
    pub fn from_env() -> Result<IMessage, String> {
        let env = |name: &str| std::env::var(name).ok().filter(|v| !v.trim().is_empty());
        let service = env("IMSG_SERVICE");
        if let Some(service) = &service {
            if !matches!(service.as_str(), "imessage" | "sms" | "auto") {
                return Err(format!("IMSG_SERVICE is {service}: imessage, sms or auto"));
            }
        }
        Ok(IMessage {
            command: env("IMSG_COMMAND").unwrap_or_else(|| "imsg rpc".into()),
            service,
            rpc: Mutex::new(None),
        })
    }

    /// The row id of the latest message: the bridge's first place.
    fn latest(rpc: &mut Rpc) -> Result<u64, String> {
        let chats = rpc.call("chats.list", json!({"limit": 1}))?;
        let Some(chat) = chats["chats"][0]["id"].as_u64() else {
            return Ok(0);
        };
        let history = rpc.call("messages.history", json!({"chat_id": chat, "limit": 1}))?;
        Ok(history["messages"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|m| m["id"].as_u64())
            .max()
            .unwrap_or(0))
    }

    /// The chat selector imsg takes for `chat`.
    fn to(&self, chat: &str, mut params: Value) -> Result<Value, String> {
        if chat.is_empty() || chat.contains(char::is_control) {
            return Err(format!("{chat:?} is not a chat"));
        }
        params["chat_identifier"] = json!(chat);
        if let Some(service) = &self.service {
            params["service"] = json!(service);
        }
        Ok(params)
    }

    fn with_rpc<T>(&self, act: impl FnOnce(&mut Rpc) -> Result<T, String>) -> Result<T, String> {
        let mut guard = self.rpc.lock().unwrap_or_else(|e| e.into_inner());
        let Some(rpc) = guard.as_mut() else {
            return Err("imessage: imsg is not running".into());
        };
        let done = act(rpc);
        if done.is_err() {
            // Started again, from the bridge's place, next time.
            *guard = None;
        }
        done
    }
}

impl Messenger for IMessage {
    fn name(&self) -> &'static str {
        "imessage"
    }

    fn receive(&self, cursor: &str, wait: u64) -> Result<(Vec<Incoming>, String), String> {
        let mut guard = self.rpc.lock().unwrap_or_else(|e| e.into_inner());
        let mut since: u64 = cursor.parse().unwrap_or(0);
        if guard.is_none() {
            let mut rpc = Rpc::start(&self.command)?;
            if since == 0 {
                since = IMessage::latest(&mut rpc)?;
            }
            rpc.call(
                "watch.subscribe",
                json!({"since_rowid": since, "attachments": true}),
            )?;
            *guard = Some(rpc);
        }
        let rpc = guard.as_mut().expect("started");
        let deadline = Instant::now() + Duration::from_secs(wait);
        let mut gathering: Option<Instant> = None;
        let mut messages = Vec::new();
        let mut upto = since;
        loop {
            let until = gathering.map_or(deadline, |g| g.min(deadline));
            let left = until.saturating_duration_since(Instant::now());
            let notification = match rpc.notification(left) {
                Ok(notification) => notification,
                Err(why) => {
                    *guard = None;
                    return Err(why);
                }
            };
            match notification {
                Some(n) if n["method"] == "message" => {
                    let message = &n["params"]["message"];
                    if let Some(id) = message["id"].as_u64() {
                        upto = upto.max(id);
                    }
                    if let Some(incoming) = imessage_message(message) {
                        messages.push(incoming);
                        gathering.get_or_insert_with(|| Instant::now() + GATHER);
                    }
                }
                Some(n) if n["method"] == "watch.overflow" => {
                    // imsg fell behind: subscribed again from where it says.
                    if let Some(resume) = n["params"]["resume_after_rowid"].as_u64() {
                        upto = resume;
                    }
                    *guard = None;
                    break;
                }
                _ => {}
            }
            if Instant::now() >= until {
                break;
            }
        }
        Ok((messages, upto.to_string()))
    }

    fn send(&self, chat: &str, text: &str) -> Result<(), String> {
        let params = self.to(chat, json!({"text": text}))?;
        self.with_rpc(|rpc| rpc.call("send", params)).map(|_| ())
    }

    fn long_polls(&self) -> bool {
        true
    }

    fn once_wait(&self) -> u64 {
        ONCE_WAIT
    }

    fn fetch_voice(&self, note: &Note) -> Result<Audio, String> {
        let id: u64 = note.id.parse().map_err(|_| "not a message's row id")?;
        let found = self.with_rpc(|rpc| {
            rpc.call(
                "messages.after",
                json!({"since_rowid": id.saturating_sub(1), "limit": 1,
                    "attachments": true, "convert_attachments": true}),
            )
        })?;
        let message = found["messages"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|m| m["id"].as_u64() == Some(id))
            .ok_or("that voice note is gone")?;
        let attachment = message["attachments"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|a| audio(a))
            .ok_or("that message holds no audio")?;
        // Converted (from Core Audio) where imsg could.
        let (path, media_type) = match attachment["converted_path"].as_str() {
            Some(path) => (path, attachment["converted_mime_type"].as_str()),
            None => (
                attachment["original_path"].as_str().unwrap_or_default(),
                attachment["mime_type"].as_str(),
            ),
        };
        if !path.contains("/Library/Messages/Attachments/") || path.contains("..") {
            return Err(format!("{path}: not one of Messages' attachments"));
        }
        let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
        if bytes.len() > MAX_HEARD {
            return Err("the voice note is too long to hear".into());
        }
        let media_type = media_type
            .map(str::to_string)
            .unwrap_or_else(|| Audio::sniff(&bytes).to_string());
        Ok(Audio::new(&media_type, bytes))
    }

    fn send_voice(&self, chat: &str, audio: &Audio, caption: &str) -> Result<bool, String> {
        // A file of its own for Messages to send, kept a while for it.
        let dir = crate::session::wallet_dir()
            .parent()
            .map(|home| home.join("agents").join("voice-out"))
            .ok_or("no home for the voice note")?;
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let old = entry
                    .metadata()
                    .and_then(|m| m.modified())
                    .is_ok_and(|t| t.elapsed().is_ok_and(|age| age > Duration::from_secs(3600)));
                if old {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
        let extension = audio.name.rsplit('.').next().unwrap_or("audio");
        let path = dir.join(format!("{}.{extension}", crate::runtime::new_id()));
        std::fs::write(&path, &audio.bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        let attachment = self.to(chat, json!({"file": path.to_string_lossy(), "audio": true}))?;
        let text = self.to(chat, json!({"text": caption}))?;
        self.with_rpc(|rpc| {
            rpc.call("send.attachment", attachment)?;
            rpc.call("send", text)
        })?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_read_as_imsg_gives_them() {
        let message = json!({
            "id": 42, "chat_id": 7, "chat_identifier": "+15550001111", "is_group": false,
            "sender": "+15550001111", "is_from_me": false, "text": "How much is left?\u{FFFC}",
            "attachments": [
                {"transfer_name": "IMG_1.HEIC", "mime_type": "image/heic", "missing": false},
                {"transfer_name": "Audio Message.caf", "uti": "com.apple.coreaudio-format", "mime_type": "audio/x-caf", "missing": false}
            ]
        });
        let incoming = imessage_message(&message).unwrap();
        assert_eq!(incoming.chat, "+15550001111");
        assert_eq!(
            incoming.text,
            "How much is left?\n(attached, not read: IMG_1.HEIC)"
        );
        assert!(incoming.direct);
        assert_eq!(
            incoming.voice,
            Some(Note {
                id: "42".into(),
                media_type: Some("audio/x-caf".into())
            })
        );
        let mut own = message.clone();
        own["is_from_me"] = json!(true);
        assert!(imessage_message(&own).is_none());
        let group = json!({"id": 43, "chat_identifier": "chat123", "is_group": true,
            "sender": "bob@example.org", "text": "lunch?", "attachments": []});
        let incoming = imessage_message(&group).unwrap();
        assert!(!incoming.direct);
        assert_eq!(incoming.from, "bob@example.org");
        let reaction =
            json!({"id": 44, "chat_identifier": "chat123", "is_reaction": true, "text": "Loved"});
        assert!(imessage_message(&reaction).is_none());
    }
}
