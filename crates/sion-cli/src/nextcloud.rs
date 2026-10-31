//! Nextcloud Talk: a bot installed on the server (`occ talk:bot:install`)
//! and enabled in the conversations it answers. Talk sends it what is said
//! there as webhooks signed with the bot's secret (HMAC-SHA256 over a
//! random value and the body), and the bridge replies through Talk's bot
//! API, signed the same way (over a random value and the message).
//!
//! | Variable | What it is |
//! |---|---|
//! | `NEXTCLOUD_URL` | the server, as Talk names itself (`https://cloud.example.org`) |
//! | `NEXTCLOUD_TALK_SECRET` | the bot's secret, given at its installation |
//!
//! A chat is a conversation's token. The webhooks do not say whether a
//! conversation is one person's, so pairing is not offered here.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use crate::live::Live;
use crate::messengers::{segment, Incoming, Messenger, LIVE_ONCE_WAIT};
use crate::showing::Typist;
use crate::webhooks::{client, cut, inbox, needed, Answer, Hook, Seen};

/// The message each conversation's reply answers.
type Replies = Arc<Mutex<BTreeMap<String, u64>>>;

/// A Talk bot.
pub struct NextcloudTalk {
    client: sion_http::Client,
    server: String,
    secret: String,
    replies: Replies,
    live: Live,
}

impl std::fmt::Debug for NextcloudTalk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "NextcloudTalk({})", self.server)
    }
}

/// HMAC-SHA256 of `random` then `data` under the bot's secret, in hex.
pub fn talk_signature(secret: &str, random: &str, data: &[u8]) -> String {
    let mut signed = random.as_bytes().to_vec();
    signed.extend_from_slice(data);
    hex::encode(sion_http::hmac_sha256(secret.as_bytes(), &signed))
}

/// A message's text, its placeholders (`{mention-user1}`) filled from its
/// parameters, and whom it mentions.
pub fn talk_text(content: &Value) -> (String, Vec<String>) {
    let mut text = content["message"].as_str().unwrap_or_default().to_string();
    let mut mentions = Vec::new();
    for (key, parameter) in content["parameters"].as_object().into_iter().flatten() {
        let name = parameter["name"].as_str().unwrap_or_default();
        text = text.replace(&format!("{{{key}}}"), &format!("@{name}"));
        if let Some(id) = parameter["id"].as_str() {
            mentions.push(id.to_string());
        }
    }
    (text.trim().to_string(), mentions)
}

/// Answers one of Talk's webhooks: refused unless signed with the bot's
/// secret by the server named; a message, passed on.
pub fn talk_hook(
    hook: &Hook,
    secret: &str,
    server: &str,
    replies: &Replies,
    seen: &mut Seen,
) -> (Answer, Vec<Incoming>) {
    let refused = |status: u16, why: &str| (Answer::text(status, why), Vec::new());
    if hook.method != "POST" {
        return refused(405, "only POST");
    }
    let random = hook.header("x-nextcloud-talk-random").unwrap_or_default();
    let signature = hook
        .header("x-nextcloud-talk-signature")
        .unwrap_or_default()
        .to_ascii_lowercase();
    let expected = talk_signature(secret, random, &hook.body);
    if random.len() < 32 || !sion_http::same_bytes(expected.as_bytes(), signature.as_bytes()) {
        return refused(401, "not signed with the bot's secret");
    }
    let backend = hook.header("x-nextcloud-talk-backend").unwrap_or_default();
    if !backend.trim_end_matches('/').eq_ignore_ascii_case(server) {
        return refused(401, "from another server");
    }
    let Ok(activity) = serde_json::from_slice::<Value>(&hook.body) else {
        return refused(400, "not JSON");
    };
    let nothing = (Answer::ok(), Vec::new());
    if activity["type"] != "Create"
        || activity["object"]["name"] != "message"
        || activity["actor"]["type"] != "Person"
    {
        return nothing;
    }
    let (Some(chat), Some(id)) = (
        activity["target"]["id"].as_str(),
        activity["object"]["id"].as_str(),
    ) else {
        return nothing;
    };
    if !seen.first(&format!("{chat} {id}")) {
        return nothing;
    }
    let content: Value = activity["object"]["content"]
        .as_str()
        .and_then(|c| serde_json::from_str(c).ok())
        .unwrap_or_default();
    let (text, mentions) = talk_text(&content);
    if text.is_empty() {
        return nothing;
    }
    if let Ok(id) = id.parse() {
        replies
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(chat.to_string(), id);
    }
    (
        Answer::ok(),
        vec![Incoming {
            id: id.to_string(),
            thread: String::new(),
            chat: chat.to_string(),
            from: activity["actor"]["id"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            text,
            voice: None,
            direct: false,
            mentions,
            files: Vec::new(),
        }],
    )
}

/// The reaction that stands for typing.
const WORKING: &str = "⏳";

/// A reaction a bot puts on a message, and takes off: signed as Talk signs
/// a bot's reactions, over the random and the reaction.
struct Reacting {
    client: sion_http::Client,
    url: String,
    secret: String,
    on: bool,
}

impl Reacting {
    fn react(&self, method: &str) -> Result<(), String> {
        let random = random_hex()?;
        let signature = talk_signature(&self.secret, &random, WORKING.as_bytes());
        self.client
            .request(
                method,
                &self.url,
                &[
                    ("Content-Type", "application/json"),
                    ("Accept", "application/json"),
                    ("OCS-APIRequest", "true"),
                    ("X-Nextcloud-Talk-Bot-Random", &random),
                    ("X-Nextcloud-Talk-Bot-Signature", &signature),
                ],
                Some(json!({"reaction": WORKING}).to_string().as_bytes()),
                1 << 16,
            )
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

impl Typist for Reacting {
    fn show(&mut self) -> Result<std::time::Duration, String> {
        if !self.on {
            self.react("POST")?;
            self.on = true;
        }
        Ok(std::time::Duration::from_secs(3_600))
    }

    fn stop(&mut self) {
        if self.on {
            let _ = self.react("DELETE");
        }
    }
}

fn random_hex() -> Result<String, String> {
    let mut bytes = [0u8; 32];
    sion_http::random(&mut bytes).map_err(|e| e.to_string())?;
    Ok(hex::encode(bytes))
}

impl NextcloudTalk {
    /// The bot of the server `NEXTCLOUD_URL`, with the secret
    /// `NEXTCLOUD_TALK_SECRET`, listening on `listen`.
    pub fn from_env(listen: &str) -> Result<NextcloudTalk, String> {
        let server = needed("NEXTCLOUD_URL", "the server, https://cloud.example.org")?
            .trim_end_matches('/')
            .to_string();
        let secret = needed("NEXTCLOUD_TALK_SECRET", "the bot's secret")?;
        let replies: Replies = Arc::new(Mutex::new(BTreeMap::new()));
        let (kept, key, named) = (replies.clone(), secret.clone(), server.clone());
        let mut seen = Seen::default();
        let live = inbox("nextcloud-talk", listen, move |hook| {
            talk_hook(hook, &key, &named, &kept, &mut seen)
        })?;
        Ok(NextcloudTalk {
            client: client()?,
            server,
            secret,
            replies,
            live,
        })
    }
}

impl Messenger for NextcloudTalk {
    fn name(&self) -> &'static str {
        "nextcloud-talk"
    }

    fn chat_word(&self) -> &'static str {
        "conversation"
    }

    fn receive(&self, cursor: &str, wait: u64) -> Result<(Vec<Incoming>, String), String> {
        Ok((self.live.receive(wait)?, cursor.to_string()))
    }

    fn send(&self, chat: &str, text: &str) -> Result<(), String> {
        let message = cut(text, 32_000);
        let random = random_hex()?;
        let signature = talk_signature(&self.secret, &random, message.as_bytes());
        let mut body = json!({"message": message, "referenceId": random_hex()?});
        if let Some(id) = self
            .replies
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(chat)
        {
            body["replyTo"] = json!(id);
        }
        self.client
            .request(
                "POST",
                &format!(
                    "{}/ocs/v2.php/apps/spreed/api/v1/bot/{}/message",
                    self.server,
                    segment(chat)
                ),
                &[
                    ("Content-Type", "application/json"),
                    ("Accept", "application/json"),
                    ("OCS-APIRequest", "true"),
                    ("X-Nextcloud-Talk-Bot-Random", &random),
                    ("X-Nextcloud-Talk-Bot-Signature", &signature),
                ],
                Some(body.to_string().as_bytes()),
                1 << 20,
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    fn room(&self) -> usize {
        32_000
    }

    /// A Talk bot shows no typing and edits nothing: an hourglass reaction
    /// on the message it works on stands for typing, taken off at the end.
    fn typist(&self, message: &Incoming, _aloud: bool) -> Option<Box<dyn Typist>> {
        (!message.id.is_empty() && message.id.bytes().all(|b| b.is_ascii_digit())).then(
            || -> Box<dyn Typist> {
                Box::new(Reacting {
                    client: self.client.clone(),
                    url: format!(
                        "{}/ocs/v2.php/apps/spreed/api/v1/bot/{}/reaction/{}",
                        self.server,
                        segment(&message.chat),
                        message.id
                    ),
                    secret: self.secret.clone(),
                    on: false,
                })
            },
        )
    }

    fn long_polls(&self) -> bool {
        true
    }

    fn sees_strangers(&self) -> bool {
        // No webhook says a conversation is one person's.
        false
    }

    fn once_wait(&self) -> u64 {
        LIVE_ONCE_WAIT
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mentions_are_filled_in_and_signatures_made_as_talk_makes_them() {
        let content = json!({"message": "hi {mention-user1}, how much is left?",
            "parameters": {"mention-user1": {"type": "user", "id": "lead-bot", "name": "Lead"}}});
        assert_eq!(
            talk_text(&content),
            (
                "hi @Lead, how much is left?".to_string(),
                vec!["lead-bot".to_string()]
            )
        );
        // RFC 4231's first case: the key, then "Hi " and "There".
        assert_eq!(
            talk_signature(&"\u{b}".repeat(20), "Hi ", b"There"),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
    }
}
