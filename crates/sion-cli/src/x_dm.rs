//! X's direct messages, through the X API v2: the account's DM events
//! polled (`GET /2/dm_events`), and replies sent in the conversation with
//! whoever wrote (`POST /2/dm_conversations/with/:id/messages`), with a
//! user's OAuth 2.0 access token (`dm.read`, `dm.write`, `users.read`,
//! `tweet.read`), renewed and kept as [`crate::x_api`] says.
//!
//! X limits how often DM events are read (fifteen requests in fifteen
//! minutes on its smaller tiers): the bridge polls once a minute.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::messengers::{Incoming, Messenger};

pub use crate::x_api::X_API;

/// How often the DM events are read, unless `X_POLL_SECS` says.
const POLL_EVERY: Duration = Duration::from_secs(60);

/// The most characters of a direct message.
const X_TEXT: usize = 10_000;

/// An X account's direct messages.
pub struct XDm {
    api: crate::x_api::XApi,
    last_poll: Mutex<Option<Instant>>,
    poll_every: Duration,
}

impl std::fmt::Debug for XDm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "XDm({:?})", self.api)
    }
}

/// The messages of a `dm_events` answer newer than `after` (an event id),
/// from others than `me`, oldest first; and the newest id.
pub fn x_messages(answer: &Value, me: &str, after: Option<u64>) -> (Vec<Incoming>, Option<u64>) {
    let mut events: Vec<(u64, &Value)> = answer["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|e| e["event_type"] == "MessageCreate")
        .filter_map(|e| Some((e["id"].as_str()?.parse::<u64>().ok()?, e)))
        .collect();
    events.sort_by_key(|(id, _)| *id);
    let newest = events.last().map(|(id, _)| *id).max(after);
    let messages = events
        .into_iter()
        .filter(|(id, _)| after.map_or(true, |after| *id > after))
        .filter_map(|(id, e)| {
            let from = e["sender_id"].as_str()?;
            if from == me {
                return None;
            }
            let text = e["text"].as_str().unwrap_or_default().trim().to_string();
            if text.is_empty() {
                return None;
            }
            Some(Incoming {
                id: id.to_string(),
                thread: String::new(),
                // The conversation with the writer is named by them.
                chat: from.to_string(),
                from: from.to_string(),
                text,
                voice: None,
                direct: true,
                mentions: Vec::new(),
                files: Vec::new(),
            })
        })
        .collect();
    (messages, newest)
}

impl XDm {
    /// The account whose token is in `X_ACCESS_TOKEN` ([`crate::x_api`]).
    pub fn from_env() -> Result<XDm, String> {
        Ok(XDm {
            api: crate::x_api::XApi::from_env("dm.read and dm.write")?,
            last_poll: Mutex::new(None),
            poll_every: std::env::var("X_POLL_SECS")
                .ok()
                .and_then(|s| s.parse().ok())
                .map(Duration::from_secs)
                .unwrap_or(POLL_EVERY),
        })
    }

    fn call(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Value, String> {
        self.api.call(method, path, body).map_err(|e| e.to_string())
    }

    fn me_id(&self) -> Result<String, String> {
        self.api.me().map(|(id, _)| id)
    }
}

impl Messenger for XDm {
    fn name(&self) -> &'static str {
        "x"
    }

    fn receive(&self, cursor: &str, _wait: u64) -> Result<(Vec<Incoming>, String), String> {
        {
            let mut last = self.last_poll.lock().unwrap_or_else(|e| e.into_inner());
            if last.is_some_and(|at| at.elapsed() < self.poll_every) {
                return Ok((Vec::new(), cursor.to_string()));
            }
            *last = Some(Instant::now());
        }
        let me = self.me_id()?;
        let answer = self.call(
            "GET",
            "/2/dm_events?event_types=MessageCreate&dm_event.fields=id,text,sender_id,created_at,dm_conversation_id&max_results=50",
            None,
        )?;
        let after: Option<u64> = cursor.parse().ok();
        let (messages, newest) = x_messages(&answer, &me, after);
        // The first time, from the newest on: nothing older is answered.
        let messages = if after.is_none() {
            Vec::new()
        } else {
            messages
        };
        Ok((
            messages,
            newest
                .map(|n| n.to_string())
                .unwrap_or_else(|| cursor.to_string()),
        ))
    }

    fn send(&self, chat: &str, text: &str) -> Result<(), String> {
        if chat.is_empty() || !chat.bytes().all(|b| b.is_ascii_digit()) {
            return Err(format!("{chat} is not an X user's id"));
        }
        let text: String = text.chars().take(X_TEXT).collect();
        self.call(
            "POST",
            &format!("/2/dm_conversations/with/{chat}/messages"),
            Some(&json!({"text": if text.is_empty() { "(nothing)".to_string() } else { text }})),
        )
        .map(|_| ())
    }

    fn room(&self) -> usize {
        X_TEXT
    }

    fn long_polls(&self) -> bool {
        false
    }

    fn me(&self) -> Result<Vec<String>, String> {
        Ok(vec![self.me_id()?])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dm_events_are_taken_after_the_place_and_not_the_accounts_own() {
        let answer = json!({"data": [
            {"id": "1003", "event_type": "MessageCreate", "text": "And now?", "sender_id": "42"},
            {"id": "1001", "event_type": "MessageCreate", "text": "How much is left?", "sender_id": "42"},
            {"id": "1002", "event_type": "MessageCreate", "text": "85 USD.", "sender_id": "7"},
            {"id": "1004", "event_type": "ParticipantsJoin", "sender_id": "42"}
        ]});
        let (messages, newest) = x_messages(&answer, "7", Some(1000));
        assert_eq!(newest, Some(1003));
        let texts: Vec<&str> = messages.iter().map(|m| m.text.as_str()).collect();
        assert_eq!(texts, ["How much is left?", "And now?"]);
        assert_eq!(messages[0].chat, "42");
        let (later, _) = x_messages(&answer, "7", Some(1001));
        assert_eq!(later.len(), 1);
    }
}
