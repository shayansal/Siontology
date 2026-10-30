//! Feishu (飞书) and Lark: an app's bot. Its events come to the bridge as
//! webhooks (`--listen`), each carrying the event subscription's
//! verification token, and its replies go through the Open Platform's API
//! with the app's tenant access token.
//!
//! | Variable | What it is |
//! |---|---|
//! | `FEISHU_APP_ID`, `FEISHU_APP_SECRET` | the app's credentials |
//! | `FEISHU_VERIFICATION_TOKEN` | the Verification Token of its event subscription |
//! | `FEISHU_DOMAIN` | `feishu` (`open.feishu.cn`, the default) or `lark` (`open.larksuite.com`) |
//!
//! An event encrypted with an Encrypt Key is refused, as the bridge reads
//! no AES: leave the Encrypt Key empty, the webhook being behind TLS.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};

use serde_json::{json, Value};

use crate::jwt::Bearer;
use crate::live::Live;
use crate::messengers::{Incoming, Messenger, Note, LIVE_ONCE_WAIT};
use crate::voice::{Audio, MAX_HEARD};
use crate::webhooks::{client, cut, env, inbox, json_of, needed, Answer, Hook, Seen};

/// The message each chat's reply answers, by chat.
type Replies = Arc<Mutex<BTreeMap<String, String>>>;

/// A bot of Feishu or Lark.
pub struct Feishu {
    client: sion_http::Client,
    base: String,
    app_id: String,
    secret: String,
    token: Mutex<Bearer>,
    replies: Replies,
    me: OnceLock<Vec<String>>,
    live: Live,
}

impl std::fmt::Debug for Feishu {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Feishu({}, {})", self.app_id, self.base)
    }
}

/// Whether `id` is an id Feishu gives (a message's, a file's).
fn plain_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// The text of a rich-text (`post`) message, in whatever language it
/// comes.
fn post_text(content: &Value) -> String {
    let body = if content["content"].is_array() {
        content
    } else {
        content
            .as_object()
            .and_then(|languages| languages.values().find(|v| v["content"].is_array()))
            .unwrap_or(content)
    };
    let mut lines = Vec::new();
    if let Some(title) = body["title"].as_str().filter(|t| !t.is_empty()) {
        lines.push(title.to_string());
    }
    for paragraph in body["content"].as_array().into_iter().flatten() {
        let line: String = paragraph
            .as_array()
            .into_iter()
            .flatten()
            .filter(|element| matches!(element["tag"].as_str(), Some("text" | "a")))
            .filter_map(|element| element["text"].as_str())
            .collect();
        lines.push(line);
    }
    lines.join("\n")
}

/// What a message says, the bot's mentions cut, and its voice note.
pub fn feishu_said(message: &Value) -> (String, Option<Note>) {
    let content: Value = message["content"]
        .as_str()
        .and_then(|c| serde_json::from_str(c).ok())
        .unwrap_or_default();
    let id = message["message_id"].as_str().unwrap_or_default();
    match message["message_type"].as_str().unwrap_or_default() {
        "text" => {
            let mut text = content["text"].as_str().unwrap_or_default().to_string();
            for mention in message["mentions"].as_array().into_iter().flatten() {
                if let Some(key) = mention["key"].as_str().filter(|k| !k.is_empty()) {
                    text = text.replace(key, "");
                }
            }
            (text.trim().to_string(), None)
        }
        "post" => (post_text(&content).trim().to_string(), None),
        "audio" => {
            let note = content["file_key"]
                .as_str()
                .filter(|key| plain_id(key) && plain_id(id))
                .map(|key| Note {
                    id: format!("{id}/{key}"),
                    media_type: None,
                });
            (String::new(), note)
        }
        other => (format!("(a message of type {other}, not read)"), None),
    }
}

/// Answers one of Feishu's webhooks: refused unless it carries the
/// verification token, or if it is encrypted; the address's check
/// answered; a message, passed on, kept for the reply.
pub fn feishu_hook(
    hook: &Hook,
    token: &str,
    replies: &Replies,
    seen: &mut Seen,
) -> (Answer, Vec<Incoming>) {
    let refused = |status: u16, why: &str| (Answer::text(status, why), Vec::new());
    if hook.method != "POST" {
        return refused(405, "only POST");
    }
    let Ok(body) = serde_json::from_slice::<Value>(&hook.body) else {
        return refused(400, "not JSON");
    };
    if body.get("encrypt").is_some() {
        return refused(
            400,
            "an encrypted event: the bridge reads none (leave the Encrypt Key empty)",
        );
    }
    let given = body["header"]["token"]
        .as_str()
        .or(body["token"].as_str())
        .unwrap_or_default();
    if !sion_http::same_bytes(given.as_bytes(), token.as_bytes()) {
        return refused(401, "not the app's verification token");
    }
    if body["type"] == "url_verification" {
        return (
            Answer::json(200, &json!({"challenge": body["challenge"]})),
            Vec::new(),
        );
    }
    let nothing = (Answer::json(200, &json!({})), Vec::new());
    if body["header"]["event_type"] != "im.message.receive_v1" {
        return nothing;
    }
    let event = &body["event"];
    let message = &event["message"];
    let id = body["header"]["event_id"]
        .as_str()
        .or(message["message_id"].as_str())
        .unwrap_or_default();
    if id.is_empty() || !seen.first(id) || event["sender"]["sender_type"] != "user" {
        return nothing;
    }
    let (Some(chat), Some(message_id)) =
        (message["chat_id"].as_str(), message["message_id"].as_str())
    else {
        return nothing;
    };
    let (text, voice) = feishu_said(message);
    if text.is_empty() && voice.is_none() {
        return nothing;
    }
    replies
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(chat.to_string(), message_id.to_string());
    let mentions = message["mentions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| m["id"]["open_id"].as_str().map(str::to_string))
        .collect();
    (
        Answer::json(200, &json!({})),
        vec![Incoming {
            chat: chat.to_string(),
            from: event["sender"]["sender_id"]["open_id"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            text,
            voice,
            direct: message["chat_type"] == "p2p",
            mentions,
        }],
    )
}

impl Feishu {
    /// The app `FEISHU_APP_ID` (with `FEISHU_APP_SECRET`), its webhooks
    /// checked against `FEISHU_VERIFICATION_TOKEN`, on Feishu, or on Lark
    /// if `lark` (unless `FEISHU_DOMAIN` says otherwise; `FEISHU_API_BASE`
    /// names another server), listening on `listen`.
    pub fn from_env(listen: &str, lark: bool) -> Result<Feishu, String> {
        let app_id = needed("FEISHU_APP_ID", "the app's App ID")?;
        let secret = needed("FEISHU_APP_SECRET", "the app's App Secret")?;
        let token = needed(
            "FEISHU_VERIFICATION_TOKEN",
            "the event subscription's Verification Token",
        )?;
        let domain =
            env("FEISHU_DOMAIN").unwrap_or_else(|| if lark { "lark" } else { "feishu" }.into());
        let base = match (env("FEISHU_API_BASE"), domain.as_str()) {
            (Some(base), _) => base,
            (None, "feishu") => "https://open.feishu.cn".into(),
            (None, "lark") => "https://open.larksuite.com".into(),
            (None, other) => return Err(format!("FEISHU_DOMAIN is {other}: feishu or lark")),
        };
        let replies: Replies = Arc::new(Mutex::new(BTreeMap::new()));
        let kept = replies.clone();
        let mut seen = Seen::default();
        let live = inbox("feishu", listen, move |hook| {
            feishu_hook(hook, &token, &kept, &mut seen)
        })?;
        Ok(Feishu {
            client: client()?,
            base: base.trim_end_matches('/').to_string(),
            app_id,
            secret,
            token: Mutex::new(Bearer::default()),
            replies,
            me: OnceLock::new(),
            live,
        })
    }

    /// The app's tenant access token.
    fn bearer(&self) -> Result<String, String> {
        self.token
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(|| {
                let body = json!({"app_id": self.app_id, "app_secret": self.secret}).to_string();
                let fetched = self
                    .client
                    .post_json(
                        &format!(
                            "{}/open-apis/auth/v3/tenant_access_token/internal",
                            self.base
                        ),
                        &[],
                        body.as_bytes(),
                        1 << 16,
                    )
                    .map_err(|e| {
                        format!(
                            "the app's token: {}",
                            e.to_string().replace(&self.secret, "<secret>")
                        )
                    })?;
                let value = json_of(&fetched.body, "Feishu")?;
                let token = value["tenant_access_token"]
                    .as_str()
                    .ok_or_else(|| format!("Feishu gave no token: {}", value["msg"]))?;
                Ok((token.to_string(), value["expire"].as_u64().unwrap_or(7200)))
            })
    }

    /// Calls the API with the tenant token: `Ok` when it answers code 0.
    fn call(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Value, String> {
        let token = self.bearer()?;
        let auth = format!("Bearer {token}");
        let bytes = body.map(|b| b.to_string().into_bytes());
        let mut headers = vec![("Authorization", auth.as_str())];
        if bytes.is_some() {
            headers.push(("Content-Type", "application/json; charset=utf-8"));
        }
        let fetched = self
            .client
            .request(
                method,
                &format!("{}{path}", self.base),
                &headers,
                bytes.as_deref(),
                1 << 20,
            )
            .map_err(|e| e.to_string().replace(&token, "<token>"))?;
        let value = json_of(&fetched.body, "Feishu")?;
        if value["code"].as_i64().unwrap_or(0) != 0 {
            return Err(format!(
                "Feishu refused it: {} ({})",
                value["msg"], value["code"]
            ));
        }
        Ok(value)
    }
}

impl Messenger for Feishu {
    fn name(&self) -> &'static str {
        "feishu"
    }

    fn receive(&self, cursor: &str, wait: u64) -> Result<(Vec<Incoming>, String), String> {
        Ok((self.live.receive(wait)?, cursor.to_string()))
    }

    fn send(&self, chat: &str, text: &str) -> Result<(), String> {
        let content = json!({"text": cut(text, 30_000)}).to_string();
        let answered = self
            .replies
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(chat)
            .cloned();
        match answered {
            Some(message) if plain_id(&message) => self.call(
                "POST",
                &format!("/open-apis/im/v1/messages/{message}/reply"),
                Some(&json!({"msg_type": "text", "content": content})),
            ),
            _ => self.call(
                "POST",
                "/open-apis/im/v1/messages?receive_id_type=chat_id",
                Some(&json!({"receive_id": chat, "msg_type": "text", "content": content})),
            ),
        }
        .map(|_| ())
    }

    fn long_polls(&self) -> bool {
        true
    }

    fn once_wait(&self) -> u64 {
        LIVE_ONCE_WAIT
    }

    fn me(&self) -> Result<Vec<String>, String> {
        if let Some(me) = self.me.get() {
            return Ok(me.clone());
        }
        let info = self.call("GET", "/open-apis/bot/v3/info", None)?;
        let names: Vec<String> = info["bot"]["open_id"]
            .as_str()
            .map(str::to_string)
            .into_iter()
            .collect();
        Ok(self.me.get_or_init(|| names).clone())
    }

    fn fetch_voice(&self, note: &Note) -> Result<Audio, String> {
        let (message, key) = note.id.split_once('/').ok_or("not a voice note's id")?;
        if !plain_id(message) || !plain_id(key) {
            return Err("not a voice note's id".into());
        }
        let token = self.bearer()?;
        let auth = format!("Bearer {token}");
        let fetched = self
            .client
            .request(
                "GET",
                &format!(
                    "{}/open-apis/im/v1/messages/{message}/resources/{key}?type=file",
                    self.base
                ),
                &[("Authorization", &auth)],
                None,
                MAX_HEARD as u64,
            )
            .map_err(|e| e.to_string().replace(&token, "<token>"))?;
        let media_type = Audio::sniff(&fetched.body).to_string();
        Ok(Audio::new(&media_type, fetched.body))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_read_without_the_bots_mention() {
        let message = json!({"message_id": "om_1", "message_type": "text", "chat_type": "group",
            "content": "{\"text\":\"@_user_1 How much budget is left?\"}",
            "mentions": [{"key": "@_user_1", "id": {"open_id": "ou_bot"}, "name": "Lead"}]});
        assert_eq!(feishu_said(&message).0, "How much budget is left?");
        let post = json!({"message_id": "om_2", "message_type": "post",
            "content": "{\"zh_cn\":{\"title\":\"Budget\",\"content\":[[{\"tag\":\"text\",\"text\":\"How much \"},{\"tag\":\"a\",\"text\":\"is left\",\"href\":\"x\"}]]}}"});
        assert_eq!(feishu_said(&post).0, "Budget\nHow much is left");
        let audio = json!({"message_id": "om_3", "message_type": "audio",
            "content": "{\"file_key\":\"file_v3_abc\",\"duration\":2000}"});
        assert_eq!(feishu_said(&audio).1.unwrap().id, "om_3/file_v3_abc");
        let image = json!({"message_id": "om_4", "message_type": "image", "content": "{}"});
        assert_eq!(feishu_said(&image).0, "(a message of type image, not read)");
        assert!(plain_id("om_dc13264520392913993dd051dba21dcf") && !plain_id("../x"));
    }
}
