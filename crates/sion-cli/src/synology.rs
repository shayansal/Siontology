//! Synology Chat, through a chatbot integration: Synology Chat posts what
//! people write to the bot to `--listen` (the bot's outgoing URL), with
//! the bot's token in each request, and the bridge answers through the
//! bot's incoming URL (`method=chatbot`), naming whom it writes to.
//!
//! `SYNOLOGY_CHAT_INCOMING_URL` is the bot's incoming URL, as Synology
//! Chat shows it (its token in it), and `SYNOLOGY_CHAT_TOKEN` the bot's
//! token, which every request must carry. A chat is a person, named by
//! the user id Synology Chat gives.

use serde_json::{json, Value};

use crate::live::Live;
use crate::messengers::{Incoming, Messenger, LIVE_ONCE_WAIT};
use crate::webhooks::{client, decode_form, encode_form, inbox, needed, Answer, Hook, Seen};

/// The most characters of one message.
const SYNOLOGY_TEXT: usize = 2_000;

/// What a request from Synology Chat says, if it carries the bot's
/// `token`: who wrote, and what.
pub fn synology_hook(hook: &Hook, token: &str, seen: &mut Seen) -> (Answer, Vec<Incoming>) {
    let form = decode_form(&hook.body);
    let field = |name: &str| {
        form.iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    };
    if !sion_http::same_bytes(field("token").as_bytes(), token.as_bytes()) {
        return (Answer::text(401, "not the bot's token"), Vec::new());
    }
    let user = field("user_id");
    let text = field("text").trim().to_string();
    if user.is_empty() || !user.bytes().all(|b| b.is_ascii_digit()) || text.is_empty() {
        return (Answer::json(200, &json!({})), Vec::new());
    }
    let id = match field("post_id") {
        id if !id.is_empty() => id,
        _ => format!("{user}:{}", field("timestamp")),
    };
    if !seen.first(&id) {
        return (Answer::json(200, &json!({})), Vec::new());
    }
    (
        // Answered at once; the reply goes through the incoming URL.
        Answer::json(200, &json!({})),
        vec![Incoming {
            id,
            thread: String::new(),
            chat: user.clone(),
            from: user,
            text,
            voice: None,
            direct: true,
            mentions: Vec::new(),
            files: Vec::new(),
        }],
    )
}

/// A Synology Chat chatbot.
pub struct SynologyChat {
    client: sion_http::Client,
    incoming: String,
    live: Live,
}

impl std::fmt::Debug for SynologyChat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SynologyChat")
    }
}

impl SynologyChat {
    /// The bot the variables above name, its requests taken on `listen`.
    pub fn from_env(listen: &str) -> Result<SynologyChat, String> {
        let incoming = needed("SYNOLOGY_CHAT_INCOMING_URL", "the bot's incoming URL")?;
        let token = needed("SYNOLOGY_CHAT_TOKEN", "the bot's token")?;
        let mut seen = Seen::default();
        let live = inbox("synology-chat", listen, move |hook| {
            synology_hook(hook, &token, &mut seen)
        })?;
        Ok(SynologyChat {
            // A NAS on the operator's own network, at the address they name.
            client: client()?,
            incoming,
            live,
        })
    }

    /// The incoming URL with its token hidden, for errors.
    fn hide(&self, error: String) -> String {
        match self.incoming.split_once("token=") {
            Some((_, token)) => error.replace(token.split('&').next().unwrap_or(token), "<token>"),
            None => error,
        }
    }
}

impl Messenger for SynologyChat {
    fn name(&self) -> &'static str {
        "synology-chat"
    }

    fn chat_word(&self) -> &'static str {
        "user"
    }

    fn receive(&self, cursor: &str, wait: u64) -> Result<(Vec<Incoming>, String), String> {
        Ok((self.live.receive(wait)?, cursor.to_string()))
    }

    fn send(&self, chat: &str, text: &str) -> Result<(), String> {
        let user: u64 = chat
            .parse()
            .map_err(|_| format!("{chat} is not a Synology Chat user id"))?;
        let mut text: String = text.chars().take(SYNOLOGY_TEXT).collect();
        if text.is_empty() {
            text = "(nothing)".into();
        }
        let payload = json!({"text": text, "user_ids": [user]});
        let body = encode_form(&[("payload", &payload.to_string())]);
        let fetched = self
            .client
            .request(
                "POST",
                &self.incoming,
                &[("Content-Type", "application/x-www-form-urlencoded")],
                Some(body.as_bytes()),
                1 << 16,
            )
            .map_err(|e| self.hide(e.to_string()))?;
        // Synology answers 200 either way, saying whether it took it.
        let answer: Value = serde_json::from_slice(&fetched.body).unwrap_or_default();
        if answer["success"] == false {
            let reason: String = match &answer["error"] {
                Value::Null => "no reason given".into(),
                error => error.to_string().chars().take(300).collect(),
            };
            return Err(format!("Synology Chat refused the message: {reason}"));
        }
        Ok(())
    }

    fn room(&self) -> usize {
        SYNOLOGY_TEXT
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_carry_the_bots_token_and_name_the_writer() {
        let hook = |body: &str| Hook {
            method: "POST".into(),
            url: "/".into(),
            headers: Vec::new(),
            body: body.as_bytes().to_vec(),
        };
        let mut seen = Seen::default();
        let body =
            "token=tok&user_id=5&username=ann&post_id=9&timestamp=1&text=How+much+is+left%3F";
        let (answer, messages) = synology_hook(&hook(body), "tok", &mut seen);
        assert_eq!(answer.status, 200);
        assert_eq!(
            (messages[0].chat.as_str(), messages[0].text.as_str()),
            ("5", "How much is left?")
        );
        assert!(
            synology_hook(&hook(body), "tok", &mut seen).1.is_empty(),
            "once"
        );
        assert_eq!(synology_hook(&hook(body), "other", &mut seen).0.status, 401);
    }
}
