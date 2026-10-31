//! Zalo, through the Zalo Bot Platform's API, which has Telegram's shape:
//! `getUpdates` long polled for messages, `sendMessage` to answer,
//! `sendChatAction` to show the bot typing. The bot's token is in
//! `ZALO_BOT_TOKEN` (`ZALO_API_BASE` for another server). A picture or a
//! voice message comes as an address on Zalo's servers, fetched to be shown
//! or heard; the bot sends no files, since Zalo takes a bot's pictures and
//! voice only from a public address.

use std::time::Duration;

use serde_json::{json, Value};

use crate::messengers::{Incoming, Messenger, Note};
use crate::showing::Typist;

/// Zalo's API, unless `ZALO_API_BASE` names another.
pub const ZALO_API: &str = "https://bot-api.zaloplatforms.com";

/// The most characters Zalo takes in one message.
pub const ZALO_TEXT: usize = 2_000;

/// The longest a poll waits for a message, in seconds.
const POLL_MOST: u64 = 30;

/// A Zalo bot.
pub struct Zalo {
    client: sion_http::Client,
    /// What fetches the pictures and voice messages Zalo points to: public
    /// addresses only, but for the API's own server.
    media: sion_http::Client,
    base: String,
    token: String,
}

impl std::fmt::Debug for Zalo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Zalo({})", self.base)
    }
}

/// An id Zalo gives as a string or a number, as a string.
fn id_of(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::to_string)
        .or_else(|| value.as_i64().map(|n| n.to_string()))
        .filter(|id| !id.is_empty())
}

/// The message an update carries, unless a bot wrote it or it says
/// nothing: its text (or a picture's caption), its picture and its voice.
pub fn zalo_message(update: &Value) -> Option<Incoming> {
    let message = &update["message"];
    let chat = id_of(&message["chat"]["id"])?;
    let from = &message["from"];
    if from["is_bot"] == true {
        return None;
    }
    let text = message["text"]
        .as_str()
        .or(message["caption"].as_str())
        .unwrap_or_default()
        .trim()
        .to_string();
    let voice = message["voice_url"].as_str().map(|url| Note {
        id: url.to_string(),
        media_type: None,
        name: None,
    });
    let files: Vec<Note> = message["photo"]
        .as_str()
        .or(message["photo_url"].as_str())
        .map(|url| Note {
            id: url.to_string(),
            media_type: None,
            name: None,
        })
        .into_iter()
        .collect();
    if text.is_empty() && voice.is_none() && files.is_empty() {
        return None;
    }
    Some(Incoming {
        id: id_of(&message["message_id"]).unwrap_or_default(),
        thread: String::new(),
        chat: chat.clone(),
        from: id_of(&from["id"]).unwrap_or(chat),
        text,
        voice,
        direct: message["chat"]["chat_type"] != "GROUP",
        mentions: Vec::new(),
        files,
    })
}

impl Zalo {
    /// The bot whose token is in `ZALO_BOT_TOKEN`, at `ZALO_API_BASE`
    /// (default [`ZALO_API`]).
    pub fn from_env() -> Result<Zalo, String> {
        let env = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
        let client = |allow_private: bool| {
            sion_http::Client::new(sion_http::Options {
                timeout: Duration::from_secs(POLL_MOST + 30),
                allow_private,
                ..sion_http::Options::default()
            })
            .map_err(|e| e.to_string())
        };
        Ok(Zalo {
            // The person names the API's address.
            client: client(true)?,
            media: client(false)?,
            base: env("ZALO_API_BASE")
                .unwrap_or_else(|| ZALO_API.into())
                .trim_end_matches('/')
                .to_string(),
            token: env("ZALO_BOT_TOKEN").ok_or("set ZALO_BOT_TOKEN (the bot's token)")?,
        })
    }

    /// Calls `method` with `body`: its `result`, or why it failed (the
    /// token kept out).
    fn call(&self, method: &str, body: &Value) -> Result<Value, String> {
        let url = format!("{}/bot{}/{method}", self.base, self.token);
        let hide = |e: String| e.replace(&self.token, "<token>");
        let answer = match self
            .client
            .post_json(&url, &[], body.to_string().as_bytes(), 4 << 20)
        {
            Ok(fetched) => fetched.body,
            // Zalo says why in the body of an error status too.
            Err(sion_http::HttpError::Status { body, .. }) => body.into_bytes(),
            Err(e) => return Err(hide(e.to_string())),
        };
        let value: Value = serde_json::from_slice(&answer)
            .map_err(|e| format!("Zalo answered with something other than JSON: {e}"))?;
        if value["ok"] == true {
            return Ok(value["result"].clone());
        }
        // A poll that waited and got nothing.
        if value["error_code"] == 408 {
            return Ok(Value::Null);
        }
        Err(hide(format!(
            "Zalo refused {method}: {}",
            value["description"].as_str().unwrap_or("no reason given")
        )))
    }

    /// Whether `url` is on the API's own server, which the person named.
    fn is_own(&self, url: &str) -> bool {
        url.starts_with(&format!("{}/", self.base))
    }
}

/// Zalo's typing, shown with `sendChatAction`.
struct Typing {
    client: sion_http::Client,
    url: String,
    token: String,
    chat: String,
}

impl Typist for Typing {
    fn show(&mut self) -> Result<Duration, String> {
        self.client
            .post_json(
                &self.url,
                &[],
                json!({"chat_id": self.chat, "action": "typing"})
                    .to_string()
                    .as_bytes(),
                1 << 16,
            )
            .map_err(|e| e.to_string().replace(&self.token, "<token>"))?;
        Ok(Duration::from_secs(5))
    }
}

impl Messenger for Zalo {
    fn name(&self) -> &'static str {
        "zalo"
    }

    fn receive(&self, cursor: &str, wait: u64) -> Result<(Vec<Incoming>, String), String> {
        // Zalo hands each update once; the cursor stays as it is.
        let result = self.call(
            "getUpdates",
            &json!({"timeout": wait.min(POLL_MOST).to_string()}),
        )?;
        let updates = match result {
            Value::Array(updates) => updates,
            Value::Null => Vec::new(),
            update => vec![update],
        };
        Ok((
            updates.iter().filter_map(zalo_message).collect(),
            cursor.to_string(),
        ))
    }

    fn send(&self, chat: &str, text: &str) -> Result<(), String> {
        let text: String = text.chars().take(ZALO_TEXT).collect();
        self.call(
            "sendMessage",
            &json!({"chat_id": chat, "text": if text.is_empty() { "(nothing)".to_string() } else { text }}),
        )
        .map(|_| ())
    }

    fn room(&self) -> usize {
        ZALO_TEXT
    }

    fn long_polls(&self) -> bool {
        true
    }

    fn me(&self) -> Result<Vec<String>, String> {
        let me = self.call("getMe", &json!({}))?;
        Ok(id_of(&me["id"]).into_iter().collect())
    }

    fn typist(&self, message: &Incoming, _aloud: bool) -> Option<Box<dyn Typist>> {
        Some(Box::new(Typing {
            client: self.client.clone(),
            url: format!("{}/bot{}/sendChatAction", self.base, self.token),
            token: self.token.clone(),
            chat: message.chat.clone(),
        }))
    }

    /// A picture or a voice message, from the address Zalo gave: a public
    /// one, or the API's own server's.
    fn fetch_voice(&self, note: &Note) -> Result<crate::voice::Audio, String> {
        let client = if self.is_own(&note.id) {
            &self.client
        } else {
            &self.media
        };
        let fetched = client
            .get(&note.id, crate::files::MAX_RECEIVED)
            .map_err(|e| e.to_string().replace(&self.token, "<token>"))?;
        let media_type = note
            .media_type
            .clone()
            .or(fetched.content_type.clone())
            .unwrap_or_else(|| crate::voice::Audio::sniff(&fetched.body).to_string());
        Ok(crate::voice::Audio::new(&media_type, fetched.body))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn updates_are_read_as_zalo_sends_them() {
        let text = json!({"event_name": "message.text.received", "message": {
            "message_id": "m1", "date": 1, "text": "How much is left?",
            "from": {"id": "u1", "display_name": "Ann", "is_bot": false},
            "chat": {"id": "u1", "chat_type": "PRIVATE"}}});
        let message = zalo_message(&text).unwrap();
        assert_eq!(
            (
                message.chat.as_str(),
                message.from.as_str(),
                message.text.as_str(),
                message.direct
            ),
            ("u1", "u1", "How much is left?", true)
        );
        let picture = json!({"event_name": "message.image.received", "message": {
            "message_id": 7, "photo": "https://example.org/p.jpg", "caption": "This one",
            "from": {"id": "u2"}, "chat": {"id": "g1", "chat_type": "GROUP"}}});
        let message = zalo_message(&picture).unwrap();
        assert_eq!(message.id, "7");
        assert_eq!(message.text, "This one");
        assert!(!message.direct);
        assert_eq!(message.files[0].id, "https://example.org/p.jpg");
        let voice = json!({"event_name": "message.voice.received", "message": {
            "message_id": "m3", "voice_url": "https://example.org/v.aac",
            "from": {"id": "u1"}, "chat": {"id": "u1", "chat_type": "PRIVATE"}}});
        assert_eq!(
            zalo_message(&voice).unwrap().voice.unwrap().id,
            "https://example.org/v.aac"
        );
        let bot = json!({"message": {"message_id": "m4", "text": "hi",
            "from": {"id": "b1", "is_bot": true}, "chat": {"id": "u1"}}});
        assert!(zalo_message(&bot).is_none());
        let sticker = json!({"event_name": "message.sticker.received", "message": {
            "message_id": "m5", "sticker": "s1", "from": {"id": "u1"}, "chat": {"id": "u1"}}});
        assert!(zalo_message(&sticker).is_none());
    }
}
