//! X's direct messages, through the X API v2: the account's DM events
//! polled (`GET /2/dm_events`), and replies sent in the conversation with
//! whoever wrote (`POST /2/dm_conversations/with/:id/messages`), with a
//! user's OAuth 2.0 access token (`dm.read`, `dm.write`, `users.read`,
//! `tweet.read`). With `X_REFRESH_TOKEN` and `X_CLIENT_ID` (and
//! `X_CLIENT_SECRET` for a confidential app), the token is renewed when X
//! refuses it, and the new refresh token kept with it in memory.
//!
//! X limits how often DM events are read (fifteen requests in fifteen
//! minutes on its smaller tiers): the bridge polls once a minute.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::messengers::{Incoming, Messenger};

/// X's API, unless `X_API_BASE` names another.
pub const X_API: &str = "https://api.x.com";

/// How often the DM events are read, unless `X_POLL_SECS` says.
const POLL_EVERY: Duration = Duration::from_secs(60);

/// The most characters of a direct message.
const X_TEXT: usize = 10_000;

/// The access and refresh tokens, as they last were.
#[derive(Clone)]
struct Tokens {
    access: String,
    refresh: Option<String>,
}

/// An X account's direct messages.
pub struct XDm {
    client: sion_http::Client,
    base: String,
    tokens: Mutex<Tokens>,
    client_id: Option<String>,
    client_secret: Option<String>,
    me: Mutex<Option<String>>,
    last_poll: Mutex<Option<Instant>>,
    poll_every: Duration,
}

impl std::fmt::Debug for XDm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "XDm({})", self.base)
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
    /// The account whose token is in `X_ACCESS_TOKEN`.
    pub fn from_env() -> Result<XDm, String> {
        let env = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
        Ok(XDm {
            client: sion_http::Client::new(sion_http::Options {
                timeout: Duration::from_secs(30),
                allow_private: true,
                ..sion_http::Options::default()
            })
            .map_err(|e| e.to_string())?,
            base: env("X_API_BASE")
                .unwrap_or_else(|| X_API.into())
                .trim_end_matches('/')
                .to_string(),
            tokens: Mutex::new(Tokens {
                access: env("X_ACCESS_TOKEN").ok_or(
                    "set X_ACCESS_TOKEN (a user's OAuth 2.0 token, with dm.read and dm.write)",
                )?,
                refresh: env("X_REFRESH_TOKEN"),
            }),
            client_id: env("X_CLIENT_ID"),
            client_secret: env("X_CLIENT_SECRET"),
            me: Mutex::new(None),
            last_poll: Mutex::new(None),
            poll_every: env("X_POLL_SECS")
                .and_then(|s| s.parse().ok())
                .map(Duration::from_secs)
                .unwrap_or(POLL_EVERY),
        })
    }

    /// A new access token from the refresh token, if there is one.
    fn renew(&self) -> Result<(), String> {
        let mut tokens = self.tokens.lock().unwrap_or_else(|e| e.into_inner());
        let (Some(refresh), Some(client_id)) = (tokens.refresh.clone(), self.client_id.clone())
        else {
            return Err(
                "X refused the access token, and there is no refresh token to renew it".into(),
            );
        };
        let body = crate::webhooks::encode_form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", &refresh),
            ("client_id", &client_id),
        ]);
        let basic = self.client_secret.as_ref().map(|secret| {
            format!(
                "Basic {}",
                sion_http::base64(format!("{client_id}:{secret}").as_bytes())
            )
        });
        let mut headers = vec![("Content-Type", "application/x-www-form-urlencoded")];
        if let Some(basic) = &basic {
            headers.push(("Authorization", basic));
        }
        let fetched = self
            .client
            .request(
                "POST",
                &format!("{}/2/oauth2/token", self.base),
                &headers,
                Some(body.as_bytes()),
                1 << 16,
            )
            .map_err(|e| e.to_string().replace(&refresh, "<token>"))?;
        let answer: Value = serde_json::from_slice(&fetched.body).map_err(|e| e.to_string())?;
        tokens.access = answer["access_token"]
            .as_str()
            .ok_or("X gave no access token")?
            .to_string();
        if let Some(next) = answer["refresh_token"].as_str() {
            tokens.refresh = Some(next.to_string());
        }
        Ok(())
    }

    /// `method path` with the access token, renewed once if X refuses it.
    fn call(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Value, String> {
        for renewed in [false, true] {
            let access = self
                .tokens
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .access
                .clone();
            let auth = format!("Bearer {access}");
            let bytes = body.map(|b| b.to_string().into_bytes());
            let mut headers = vec![("Authorization", auth.as_str())];
            if bytes.is_some() {
                headers.push(("Content-Type", "application/json"));
            }
            match self.client.request(
                method,
                &format!("{}{path}", self.base),
                &headers,
                bytes.as_deref(),
                4 << 20,
            ) {
                Ok(fetched) => {
                    return serde_json::from_slice(&fetched.body)
                        .map_err(|e| format!("X answered with something other than JSON: {e}"))
                }
                Err(sion_http::HttpError::Status { status: 401, .. }) if !renewed => {
                    self.renew()?
                }
                Err(e) => return Err(e.to_string().replace(&access, "<token>")),
            }
        }
        Err("X refused the access token".into())
    }

    fn me_id(&self) -> Result<String, String> {
        if let Some(me) = self.me.lock().unwrap_or_else(|e| e.into_inner()).clone() {
            return Ok(me);
        }
        let me = self.call("GET", "/2/users/me", None)?;
        let id = me["data"]["id"]
            .as_str()
            .ok_or("X did not say who the token is for")?
            .to_string();
        *self.me.lock().unwrap_or_else(|e| e.into_inner()) = Some(id.clone());
        Ok(id)
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
