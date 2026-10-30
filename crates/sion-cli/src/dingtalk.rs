//! DingTalk (钉钉): an internal app's robot, through Stream mode. The
//! bridge opens a WebSocket of its own to DingTalk, so there is no address
//! to expose, takes the robot's messages over it and acknowledges each,
//! and replies through the message's session webhook, or, once that has
//! expired, through the robot's API with the app's access token.
//!
//! | Variable | What it is |
//! |---|---|
//! | `DINGTALK_CLIENT_ID` | the app's Client ID (its AppKey, and the robot's code) |
//! | `DINGTALK_CLIENT_SECRET` | the app's Client Secret |
//!
//! DingTalk transcribes voice messages itself: what it heard is what the
//! agent is given, marked as heard.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde_json::{json, Value};
use sion_http::ws::WebSocket;
use sion_http::HttpError;

use crate::jwt::Bearer;
use crate::live::{again, keep, text_of, Fault, Link, Live, Next, MAX_EVENT};
use crate::messengers::{Incoming, Messenger, LIVE_ONCE_WAIT};
use crate::webhooks::{client, cut, env, json_of, needed, Seen};

/// The topic of a robot's messages.
pub const TOPIC: &str = "/v1.0/im/bot/messages/get";

/// What a mention of the robot is called in [`Incoming::mentions`].
const ROBOT: &str = "robot";

/// Where a chat's replies go: the session webhook of its last message,
/// until when (milliseconds since the epoch), and who wrote it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Session {
    /// The session webhook.
    pub webhook: String,
    /// When it expires.
    pub until_ms: u64,
    /// Whether the chat is a direct one.
    pub direct: bool,
    /// Who wrote, by their staff id, for a reply after the webhook.
    pub staff: String,
}

type Sessions = Arc<Mutex<BTreeMap<String, Session>>>;

/// A robot in DingTalk.
pub struct DingTalk {
    client: sion_http::Client,
    base: String,
    id: String,
    secret: String,
    sessions: Sessions,
    token: Mutex<Bearer>,
    live: OnceLock<Live>,
}

impl std::fmt::Debug for DingTalk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "DingTalk({})", self.id)
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// The text a rich-text message's parts say.
fn rich_text(content: &Value) -> String {
    content["richText"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|part| part["text"].as_str())
        .collect::<Vec<_>>()
        .join("")
}

/// A robot's message, as the bridge takes it, and where its reply goes.
pub fn dingtalk_message(data: &Value) -> Option<(Incoming, Session)> {
    let chat = data["conversationId"].as_str().filter(|c| !c.is_empty())?;
    let text = match data["msgtype"].as_str()? {
        "text" => data["text"]["content"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        "richText" => rich_text(&data["content"]),
        "audio" => match data["content"]["recognition"].as_str() {
            Some(heard) if !heard.trim().is_empty() => {
                format!("(voice note, as DingTalk heard it) {}", heard.trim())
            }
            _ => "(a voice note DingTalk did not make out)".to_string(),
        },
        other => format!("(a message of type {other}, not read)"),
    };
    let text = text.trim().to_string();
    if text.is_empty() {
        return None;
    }
    let staff = data["senderStaffId"].as_str().unwrap_or_default();
    let from = if staff.is_empty() {
        data["senderId"].as_str().unwrap_or_default()
    } else {
        staff
    };
    let direct = data["conversationType"] == "1";
    let incoming = Incoming {
        chat: chat.to_string(),
        from: from.to_string(),
        text,
        voice: None,
        direct,
        mentions: if data["isInAtList"] == true {
            vec![ROBOT.to_string()]
        } else {
            Vec::new()
        },
    };
    let session = Session {
        webhook: data["sessionWebhook"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        until_ms: data["sessionWebhookExpiredTime"].as_u64().unwrap_or(0),
        direct,
        staff: staff.to_string(),
    };
    Some((incoming, session))
}

/// The acknowledgement of a frame: a system frame's echoes its data.
pub fn ack(frame: &Value) -> Value {
    let message_id = frame["headers"]["messageId"].as_str().unwrap_or_default();
    let data = if frame["type"] == "SYSTEM" {
        frame["data"].clone()
    } else {
        json!(json!({"response": "OK"}).to_string())
    };
    json!({
        "code": 200,
        "headers": {"contentType": "application/json", "messageId": message_id},
        "message": "OK",
        "data": data,
    })
}

/// The scheme and authority of `url`.
fn origin(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    (!authority.is_empty()).then(|| {
        format!(
            "{}://{}",
            scheme.to_ascii_lowercase(),
            authority.to_ascii_lowercase()
        )
    })
}

/// Whether a session webhook may be posted to: DingTalk's, over TLS (or
/// the API's own origin, for a stand-in).
fn webhook_allowed(webhook: &str, base: &str) -> bool {
    let Some(origin) = origin(webhook) else {
        return false;
    };
    if Some(&origin) == self::origin(base).as_ref() {
        return true;
    }
    origin
        .strip_prefix("https://")
        .map(|host| host.split(':').next().unwrap_or(host))
        .is_some_and(|host| host == "dingtalk.com" || host.ends_with(".dingtalk.com"))
}

/// One connection: opened with the app's credentials, then its frames.
fn connection(
    client: &sion_http::Client,
    base: &str,
    credentials: (&str, &str),
    sessions: &Sessions,
    seen: &mut Seen,
    link: &Link,
) -> Result<Next, Fault> {
    let (id, secret) = credentials;
    let body = json!({
        "clientId": id, "clientSecret": secret,
        "subscriptions": [{"type": "CALLBACK", "topic": TOPIC}],
        "ua": format!("sion/{}", env!("CARGO_PKG_VERSION")), "localIp": "",
    })
    .to_string();
    let opened = client
        .post_json(
            &format!("{base}/v1.0/gateway/connections/open"),
            &[("Accept", "application/json")],
            body.as_bytes(),
            1 << 16,
        )
        .map_err(|e| match e {
            HttpError::Status {
                status: 400 | 401 | 403,
                ..
            } => Fault::Fatal("DingTalk refused the app's credentials".into()),
            e => again(e.to_string().replace(secret, "<secret>")),
        })?;
    let opened = json_of(&opened.body, "DingTalk").map_err(again)?;
    let (Some(endpoint), Some(ticket)) = (opened["endpoint"].as_str(), opened["ticket"].as_str())
    else {
        return Err(again("DingTalk gave no endpoint or ticket"));
    };
    let url = format!("{endpoint}?ticket={}", crate::messengers::segment(ticket));
    let mut socket = WebSocket::connect(client, &url, &[], MAX_EVENT).map_err(again)?;
    loop {
        if link.stopped() {
            socket.close(1000, "stopped");
            return Ok(Next::Stop);
        }
        let frame = match text_of(socket.receive(Duration::from_secs(1))) {
            Ok(Some(frame)) => frame,
            Ok(None) => continue,
            // A connection is not taken up again: a new one is opened.
            Err(HttpError::Closed { .. }) => return Ok(Next::Identify),
            Err(e) => return Err(again(e)),
        };
        // Acknowledged at once, so that DingTalk does not send it again.
        socket.send_text(&ack(&frame).to_string()).map_err(again)?;
        let topic = frame["headers"]["topic"].as_str().unwrap_or_default();
        if frame["type"] == "SYSTEM" && topic == "disconnect" {
            socket.close(1000, "asked to");
            return Ok(Next::Identify);
        }
        if frame["type"] != "CALLBACK" || topic != TOPIC {
            continue;
        }
        let Ok(data) = serde_json::from_str::<Value>(frame["data"].as_str().unwrap_or_default())
        else {
            continue;
        };
        let id = data["msgId"].as_str().unwrap_or_default();
        if id.is_empty() || !seen.first(id) {
            continue;
        }
        if let Some((incoming, session)) = dingtalk_message(&data) {
            sessions
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(incoming.chat.clone(), session);
            if !link.pass(incoming) {
                socket.close(1000, "stopped");
                return Ok(Next::Stop);
            }
        }
    }
}

impl DingTalk {
    /// The robot of the app `DINGTALK_CLIENT_ID`, with its secret
    /// `DINGTALK_CLIENT_SECRET`; `DINGTALK_API_BASE` points it at another
    /// server (a stand-in).
    pub fn from_env() -> Result<DingTalk, String> {
        Ok(DingTalk {
            client: client()?,
            base: env("DINGTALK_API_BASE")
                .unwrap_or_else(|| "https://api.dingtalk.com".into())
                .trim_end_matches('/')
                .to_string(),
            id: needed("DINGTALK_CLIENT_ID", "the app's Client ID")?,
            secret: needed("DINGTALK_CLIENT_SECRET", "the app's Client Secret")?,
            sessions: Arc::new(Mutex::new(BTreeMap::new())),
            token: Mutex::new(Bearer::default()),
            live: OnceLock::new(),
        })
    }

    fn live(&self) -> &Live {
        self.live.get_or_init(|| {
            let (client, base) = (self.client.clone(), self.base.clone());
            let (id, secret, sessions) =
                (self.id.clone(), self.secret.clone(), self.sessions.clone());
            Live::start("dingtalk", move |link| {
                let mut seen = Seen::default();
                keep("dingtalk", &link, |link, _| {
                    connection(&client, &base, (&id, &secret), &sessions, &mut seen, link)
                });
            })
        })
    }

    /// The app's access token.
    fn bearer(&self) -> Result<String, String> {
        self.token
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(|| {
                let body = json!({"appKey": self.id, "appSecret": self.secret}).to_string();
                let fetched = self
                    .client
                    .post_json(
                        &format!("{}/v1.0/oauth2/accessToken", self.base),
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
                let value = json_of(&fetched.body, "DingTalk")?;
                let token = value["accessToken"]
                    .as_str()
                    .ok_or("DingTalk gave no access token")?;
                Ok((
                    token.to_string(),
                    value["expireIn"].as_u64().unwrap_or(7200),
                ))
            })
    }
}

impl Messenger for DingTalk {
    fn name(&self) -> &'static str {
        "dingtalk"
    }

    fn chat_word(&self) -> &'static str {
        "conversation"
    }

    fn receive(&self, cursor: &str, wait: u64) -> Result<(Vec<Incoming>, String), String> {
        Ok((self.live().receive(wait)?, cursor.to_string()))
    }

    fn send(&self, chat: &str, text: &str) -> Result<(), String> {
        let session = self
            .sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(chat)
            .cloned()
            .ok_or_else(|| format!("{chat}: nothing came from that conversation yet"))?;
        let text = cut(text, 18_000);
        // The session webhook while it lasts (a minute's margin).
        if now_ms() + 60_000 < session.until_ms && webhook_allowed(&session.webhook, &self.base) {
            let body = json!({"msgtype": "text", "text": {"content": text}}).to_string();
            let fetched = self
                .client
                .post_json(&session.webhook, &[], body.as_bytes(), 1 << 16)
                .map_err(|e| e.to_string())?;
            let answer = json_of(&fetched.body, "DingTalk")?;
            if answer["errcode"].as_i64().unwrap_or(0) != 0 {
                return Err(format!("DingTalk refused the reply: {}", answer["errmsg"]));
            }
            return Ok(());
        }
        // After it, the robot's API.
        let token = self.bearer()?;
        let message = json!({"content": text}).to_string();
        let (path, body) = if session.direct {
            (
                "/v1.0/robot/oToMessages/batchSend",
                json!({"robotCode": self.id, "userIds": [session.staff], "msgKey": "sampleText", "msgParam": message}),
            )
        } else {
            (
                "/v1.0/robot/groupMessages/send",
                json!({"robotCode": self.id, "openConversationId": chat, "msgKey": "sampleText", "msgParam": message}),
            )
        };
        self.client
            .post_json(
                &format!("{}{path}", self.base),
                &[("x-acs-dingtalk-access-token", &token)],
                body.to_string().as_bytes(),
                1 << 16,
            )
            .map_err(|e| e.to_string().replace(&token, "<token>"))?;
        Ok(())
    }

    fn long_polls(&self) -> bool {
        true
    }

    fn once_wait(&self) -> u64 {
        LIVE_ONCE_WAIT
    }

    fn me(&self) -> Result<Vec<String>, String> {
        Ok(vec![ROBOT.to_string()])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_and_acknowledgements_are_as_stream_mode_has_them() {
        let data = json!({
            "conversationId": "cid1", "conversationType": "2", "msgId": "m1", "msgtype": "text",
            "text": {"content": " How much budget is left? "}, "senderStaffId": "staff7",
            "senderId": "$:LWCP:x", "isInAtList": true,
            "sessionWebhook": "https://oapi.dingtalk.com/robot/sendBySession?session=s1",
            "sessionWebhookExpiredTime": 1_900_000_000_000_u64
        });
        let (incoming, session) = dingtalk_message(&data).unwrap();
        assert_eq!(incoming.text, "How much budget is left?");
        assert_eq!((incoming.from.as_str(), incoming.direct), ("staff7", false));
        assert_eq!(incoming.mentions, [ROBOT]);
        assert_eq!(session.until_ms, 1_900_000_000_000);
        let voice = json!({"conversationId": "cid2", "conversationType": "1", "msgtype": "audio",
            "content": {"recognition": "how much is left", "downloadCode": "d1"}, "senderId": "u1"});
        assert_eq!(
            dingtalk_message(&voice).unwrap().0.text,
            "(voice note, as DingTalk heard it) how much is left"
        );
        let system = json!({"type": "SYSTEM", "headers": {"topic": "ping", "messageId": "p1"}, "data": "{\"opaque\":1}"});
        assert_eq!(
            ack(&system),
            json!({"code": 200, "headers": {"contentType": "application/json", "messageId": "p1"},
                "message": "OK", "data": "{\"opaque\":1}"})
        );
        let callback = json!({"type": "CALLBACK", "headers": {"topic": TOPIC, "messageId": "c1"}, "data": "{}"});
        assert_eq!(ack(&callback)["data"], "{\"response\":\"OK\"}");
        assert!(webhook_allowed(
            "https://oapi.dingtalk.com/robot/sendBySession?session=s",
            "https://api.dingtalk.com"
        ));
        assert!(!webhook_allowed(
            "http://oapi.dingtalk.com/x",
            "https://api.dingtalk.com"
        ));
        assert!(!webhook_allowed(
            "https://dingtalk.com.evil.example/x",
            "https://api.dingtalk.com"
        ));
        assert!(webhook_allowed(
            "http://127.0.0.1:9/robot",
            "http://127.0.0.1:9"
        ));
    }
}
