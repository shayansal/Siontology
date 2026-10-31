//! QQ, through QQ's bot platform (QQ 机器人), in webhook mode: QQ posts
//! each event to the bridge, signed with Ed25519 under a key both sides
//! derive from the bot's secret, and the bridge answers through the bot API
//! (`api.sgroup.qq.com`) with an access token from the app id and secret.
//!
//! | Variable | What |
//! |---|---|
//! | `QQ_APP_ID` | the bot's app id |
//! | `QQ_APP_SECRET` | its secret: the token's, and the signatures' |
//! | `QQ_API_BASE` | another server than `https://api.sgroup.qq.com` |
//! | `QQ_TOKEN_URL` | another token endpoint than `https://bots.qq.com/app/getAppAccessToken` |
//!
//! A chat is named by its kind and QQ's id for it: `user:<openid>` for a
//! person's direct messages, `group:<openid>` for a group that mentions
//! the bot, `channel:<id>` for a channel of a QQ guild. QQ lets a bot
//! answer a message for a while after it (a passive reply, each part of it
//! numbered); later it sends unprompted, which QQ allows only a few times a
//! month.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::live::Live;
use crate::messengers::{Incoming, Messenger, Note, LIVE_ONCE_WAIT};
use crate::voice::Audio;
use crate::webhooks::{client, inbox, json_of, needed, Answer, Hook, Seen};

/// QQ's bot API, unless `QQ_API_BASE` names another.
pub const QQ_API: &str = "https://api.sgroup.qq.com";

/// Where access tokens come from, unless `QQ_TOKEN_URL` names another.
pub const QQ_TOKEN_URL: &str = "https://bots.qq.com/app/getAppAccessToken";

/// How long after a message the bot answers it as a reply.
const REPLY_FOR: Duration = Duration::from_secs(4 * 60);

/// The longest text QQ takes in one message, in characters.
const QQ_TEXT: usize = 2_000;

/// The Ed25519 seed QQ derives from a bot's secret: the secret repeated to
/// 32 bytes.
pub fn qq_seed(secret: &str) -> Result<[u8; 32], String> {
    if secret.is_empty() {
        return Err("the bot's secret is empty".into());
    }
    let mut seed = secret.to_string();
    while seed.len() < 32 {
        seed = seed.repeat(2);
    }
    seed.as_bytes()[..32]
        .try_into()
        .map_err(|_| "the bot's secret".to_string())
}

/// The message an event carries, its chat named with its kind; `None`
/// for other events, a bot's, and what says nothing.
pub fn qq_message(event: &Value) -> Option<Incoming> {
    let d = &event["d"];
    let (chat, from, direct) = match event["t"].as_str()? {
        "C2C_MESSAGE_CREATE" => {
            let user = d["author"]["user_openid"].as_str()?;
            (format!("user:{user}"), user.to_string(), true)
        }
        "GROUP_AT_MESSAGE_CREATE" => (
            format!("group:{}", d["group_openid"].as_str()?),
            d["author"]["member_openid"].as_str()?.to_string(),
            false,
        ),
        "AT_MESSAGE_CREATE" => {
            if d["author"]["bot"] == true {
                return None;
            }
            (
                format!("channel:{}", d["channel_id"].as_str()?),
                d["author"]["id"].as_str()?.to_string(),
                false,
            )
        }
        _ => return None,
    };
    // A guild's mention of the bot is in its text.
    let text: String = d["content"]
        .as_str()
        .unwrap_or_default()
        .split_whitespace()
        .filter(|word| !(word.starts_with("<@") && word.ends_with('>')))
        .collect::<Vec<_>>()
        .join(" ");
    let mut voice = None;
    let mut files = Vec::new();
    for attachment in d["attachments"].as_array().into_iter().flatten() {
        let Some(url) = attachment["url"].as_str() else {
            continue;
        };
        // QQ gives some addresses without their scheme.
        let url = if url.starts_with("//") {
            format!("https:{url}")
        } else {
            url.to_string()
        };
        let media_type = attachment["content_type"].as_str().map(str::to_string);
        let note = Note {
            id: url,
            media_type: media_type.clone(),
            name: attachment["filename"].as_str().map(str::to_string),
        };
        match media_type.as_deref() {
            Some(t) if (t.starts_with("audio/") || t == "voice") && voice.is_none() => {
                voice = Some(Note {
                    media_type: None,
                    name: None,
                    ..note
                })
            }
            _ => files.push(note),
        }
    }
    if text.is_empty() && voice.is_none() && files.is_empty() {
        return None;
    }
    Some(Incoming {
        id: d["id"].as_str().unwrap_or_default().to_string(),
        thread: String::new(),
        chat,
        from,
        text,
        voice,
        direct,
        // A group's and a channel's messages reach the bot only when they
        // mention it.
        mentions: if direct {
            Vec::new()
        } else {
            vec!["qq-bot".to_string()]
        },
        files,
    })
}

/// The messages lately answered, by chat: the last message's id, when it
/// came, and how many parts of its reply went.
type Replies = Arc<Mutex<BTreeMap<String, (String, Instant, u32)>>>;

/// Answers one of QQ's webhooks: the check of the address (op 13), its
/// token signed back, or an event, taken only when signed with the bot's
/// key.
pub fn qq_hook(
    hook: &Hook,
    seed: &[u8; 32],
    replies: &Replies,
    seen: &mut Seen,
) -> (Answer, Vec<Incoming>) {
    let refused = |status: u16, why: &str| (Answer::text(status, why), Vec::new());
    let Ok(public) = sion_http::cipher::ed25519_public(seed) else {
        return refused(500, "no key from the bot's secret");
    };
    let Ok(body) = serde_json::from_slice::<Value>(&hook.body) else {
        return refused(400, "not JSON");
    };
    let signed = || {
        let (Some(signature), Some(timestamp)) = (
            hook.header("x-signature-ed25519"),
            hook.header("x-signature-timestamp"),
        ) else {
            return false;
        };
        let Ok(signature) = hex::decode(signature) else {
            return false;
        };
        let mut message = timestamp.as_bytes().to_vec();
        message.extend_from_slice(&hook.body);
        sion_http::cipher::ed25519_verify(&public, &message, &signature)
    };
    if body["op"] == 13 {
        // The bot signs what QQ sends: only a plain token and a time, so
        // that what it signs can never pass for an event.
        let token = body["d"]["plain_token"].as_str().unwrap_or_default();
        let time = body["d"]["event_ts"].as_str().unwrap_or_default();
        let plain = |text: &str, digits: bool| {
            !text.is_empty()
                && text.len() <= 128
                && text.bytes().all(|b| {
                    if digits {
                        b.is_ascii_digit()
                    } else {
                        b.is_ascii_alphanumeric()
                    }
                })
        };
        if !plain(token, false) || !plain(time, true) {
            return refused(400, "not a plain token and time to sign");
        }
        let has_signature = hook.header("x-signature-ed25519").is_some();
        if has_signature && !signed() {
            return refused(401, "not signed with the bot's key");
        }
        return match sion_http::cipher::ed25519_sign(seed, format!("{time}{token}").as_bytes()) {
            Ok(signature) => (
                Answer::json(
                    200,
                    &json!({"plain_token": token, "signature": hex::encode(signature)}),
                ),
                Vec::new(),
            ),
            Err(e) => refused(500, &e.to_string()),
        };
    }
    if !signed() {
        return refused(401, "not signed with the bot's key");
    }
    let acknowledged = Answer::json(200, &json!({"op": 12}));
    let id = body["id"]
        .as_str()
        .or(body["d"]["id"].as_str())
        .unwrap_or_default();
    if id.is_empty() || !seen.first(id) {
        return (acknowledged, Vec::new());
    }
    let Some(message) = qq_message(&body) else {
        return (acknowledged, Vec::new());
    };
    replies.lock().unwrap_or_else(|e| e.into_inner()).insert(
        message.chat.clone(),
        (message.id.clone(), Instant::now(), 0),
    );
    (acknowledged, vec![message])
}

/// A QQ bot.
pub struct Qq {
    client: sion_http::Client,
    base: String,
    token_url: String,
    app: String,
    secret: String,
    token: Mutex<Option<(String, Instant)>>,
    replies: Replies,
    live: Live,
}

impl std::fmt::Debug for Qq {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Qq({}, app {})", self.base, self.app)
    }
}

impl Qq {
    /// The bot the variables above name, its webhooks taken on `listen`.
    pub fn from_env(listen: &str) -> Result<Qq, String> {
        let app = needed("QQ_APP_ID", "the bot's app id")?;
        let secret = needed("QQ_APP_SECRET", "the bot's secret")?;
        let seed = qq_seed(&secret)?;
        let env = |name: &str, default: &str| {
            std::env::var(name)
                .ok()
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| default.into())
                .trim_end_matches('/')
                .to_string()
        };
        let replies: Replies = Arc::default();
        let kept = replies.clone();
        let mut seen = Seen::default();
        let live = inbox("qq", listen, move |hook| {
            qq_hook(hook, &seed, &kept, &mut seen)
        })?;
        Ok(Qq {
            client: client()?,
            base: env("QQ_API_BASE", QQ_API),
            token_url: env("QQ_TOKEN_URL", QQ_TOKEN_URL),
            app,
            secret,
            token: Mutex::new(None),
            replies,
            live,
        })
    }

    /// An access token: the one kept, or a new one.
    fn token(&self) -> Result<String, String> {
        let mut kept = self.token.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((token, until)) = kept.as_ref() {
            if Instant::now() < *until {
                return Ok(token.clone());
            }
        }
        let fetched = self
            .client
            .post_json(
                &self.token_url,
                &[],
                json!({"appId": self.app, "clientSecret": self.secret})
                    .to_string()
                    .as_bytes(),
                1 << 16,
            )
            .map_err(|e| e.to_string().replace(&self.secret, "<secret>"))?;
        let answer = json_of(&fetched.body, "QQ")?;
        let token = answer["access_token"]
            .as_str()
            .ok_or_else(|| {
                format!(
                    "QQ gave no access token: {}",
                    answer["message"].as_str().unwrap_or("no reason given")
                )
            })?
            .to_string();
        // QQ writes the lifetime as a string.
        let lasts = answer["expires_in"]
            .as_u64()
            .or_else(|| answer["expires_in"].as_str().and_then(|s| s.parse().ok()))
            .unwrap_or(7_200)
            .saturating_sub(60);
        *kept = Some((token.clone(), Instant::now() + Duration::from_secs(lasts)));
        Ok(token)
    }

    /// The path a chat's messages go to.
    fn path(chat: &str) -> Result<String, String> {
        let plain = |id: &str| {
            !id.is_empty()
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
        };
        match chat.split_once(':') {
            Some(("user", id)) if plain(id) => Ok(format!("/v2/users/{id}/messages")),
            Some(("group", id)) if plain(id) => Ok(format!("/v2/groups/{id}/messages")),
            Some(("channel", id)) if plain(id) => Ok(format!("/channels/{id}/messages")),
            _ => Err(format!(
                "{chat} is not a QQ chat: user:<openid>, group:<openid> or channel:<id>"
            )),
        }
    }
}

impl Messenger for Qq {
    fn name(&self) -> &'static str {
        "qq"
    }

    fn receive(&self, cursor: &str, wait: u64) -> Result<(Vec<Incoming>, String), String> {
        Ok((self.live.receive(wait)?, cursor.to_string()))
    }

    fn send(&self, chat: &str, text: &str) -> Result<(), String> {
        let path = Qq::path(chat)?;
        let text: String = text.chars().take(QQ_TEXT).collect();
        let mut body = json!({"content": if text.is_empty() { "(nothing)".to_string() } else { text }, "msg_type": 0});
        // A reply to the chat's last message while QQ takes one, each part
        // numbered.
        if let Some((id, at, sent)) = self
            .replies
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_mut(chat)
        {
            if at.elapsed() < REPLY_FOR {
                *sent += 1;
                body["msg_id"] = json!(id);
                body["msg_seq"] = json!(*sent);
            }
        }
        let token = self.token()?;
        let auth = format!("QQBot {token}");
        self.client
            .request(
                "POST",
                &format!("{}{path}", self.base),
                &[
                    ("Authorization", &auth),
                    ("Content-Type", "application/json"),
                ],
                Some(body.to_string().as_bytes()),
                1 << 20,
            )
            .map(|_| ())
            .map_err(|e| e.to_string().replace(&token, "<token>"))
    }

    fn room(&self) -> usize {
        QQ_TEXT
    }

    fn long_polls(&self) -> bool {
        true
    }

    fn once_wait(&self) -> u64 {
        LIVE_ONCE_WAIT
    }

    fn me(&self) -> Result<Vec<String>, String> {
        Ok(vec!["qq-bot".to_string()])
    }

    /// A file a message carries, from QQ's media servers (which take no
    /// token).
    fn fetch_voice(&self, note: &Note) -> Result<Audio, String> {
        let fetched = self
            .client
            .get(&note.id, crate::files::MAX_RECEIVED)
            .map_err(|e| e.to_string())?;
        let media_type = note
            .media_type
            .clone()
            .filter(|t| t.contains('/'))
            .or(fetched.content_type.clone())
            .unwrap_or_else(|| Audio::sniff(&fetched.body).to_string());
        Ok(Audio::new(&media_type, fetched.body))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_secret_is_repeated_to_a_seed_as_qq_does_it() {
        // QQ's documented example.
        assert_eq!(
            &qq_seed("naOC0ocQE3shWLAfffVLB1rhYPG7").unwrap(),
            b"naOC0ocQE3shWLAfffVLB1rhYPG7naOC"
        );
        assert_eq!(&qq_seed(&"x".repeat(40)).unwrap(), &[b'x'; 32]);
        assert!(qq_seed("").is_err());
    }

    #[test]
    fn webhooks_are_signed_with_the_bots_key_and_the_check_answered() {
        let seed = qq_seed("naOC0ocQE3shWLAfffVLB1rhYPG7").unwrap();
        let public = sion_http::cipher::ed25519_public(&seed).unwrap();
        let replies: Replies = Arc::default();
        let mut seen = Seen::default();
        let hook = |headers: Vec<(String, String)>, body: &str| Hook {
            method: "POST".into(),
            url: "/".into(),
            headers,
            body: body.as_bytes().to_vec(),
        };
        // The check: the plain token, signed after the time.
        let check =
            r#"{"d":{"plain_token":"Arq0D5A61EgUu4OxUvOp","event_ts":"1725442341"},"op":13}"#;
        let (answer, _) = qq_hook(&hook(Vec::new(), check), &seed, &replies, &mut seen);
        let answered: Value = serde_json::from_slice(&answer.body).unwrap();
        assert_eq!(answered["plain_token"], "Arq0D5A61EgUu4OxUvOp");
        let signature = hex::decode(answered["signature"].as_str().unwrap()).unwrap();
        assert!(sion_http::cipher::ed25519_verify(
            &public,
            b"1725442341Arq0D5A61EgUu4OxUvOp",
            &signature
        ));
        // What could pass for an event is not signed.
        let oracle = r#"{"d":{"plain_token":"{\"op\":0}","event_ts":"1"},"op":13}"#;
        assert_eq!(
            qq_hook(&hook(Vec::new(), oracle), &seed, &replies, &mut seen)
                .0
                .status,
            400
        );
        // An event, signed over its time and body.
        let event = r#"{"id":"ev1","op":0,"t":"C2C_MESSAGE_CREATE","d":{"id":"m1","author":{"user_openid":"U1"},"content":"How much is left?","attachments":[{"content_type":"image/png","filename":"p.png","url":"//example.org/p.png"}]}}"#;
        let signed = |body: &str| {
            let mut message = b"1725442400".to_vec();
            message.extend_from_slice(body.as_bytes());
            vec![
                (
                    "x-signature-ed25519".to_string(),
                    hex::encode(sion_http::cipher::ed25519_sign(&seed, &message).unwrap()),
                ),
                (
                    "x-signature-timestamp".to_string(),
                    "1725442400".to_string(),
                ),
            ]
        };
        let (answer, messages) = qq_hook(&hook(signed(event), event), &seed, &replies, &mut seen);
        assert_eq!(answer.body, br#"{"op":12}"#);
        let message = &messages[0];
        assert_eq!(
            (message.chat.as_str(), message.text.as_str(), message.direct),
            ("user:U1", "How much is left?", true)
        );
        assert_eq!(message.files[0].id, "https://example.org/p.png");
        assert_eq!(replies.lock().unwrap()["user:U1"].0, "m1");
        // Sent again, or forged: not passed on.
        assert!(
            qq_hook(&hook(signed(event), event), &seed, &replies, &mut seen)
                .1
                .is_empty()
        );
        let forged = event.replace("ev1", "ev2");
        assert_eq!(
            qq_hook(&hook(signed(event), &forged), &seed, &replies, &mut seen)
                .0
                .status,
            401
        );
        assert_eq!(
            qq_hook(&hook(Vec::new(), &forged), &seed, &replies, &mut seen)
                .0
                .status,
            401
        );
    }

    #[test]
    fn groups_and_channels_are_chats_of_their_kind() {
        let group = json!({"t": "GROUP_AT_MESSAGE_CREATE", "d": {"id": "m2", "group_openid": "G1",
            "author": {"member_openid": "M1"}, "content": " Hi"}});
        let message = qq_message(&group).unwrap();
        assert_eq!(
            (message.chat.as_str(), message.text.as_str()),
            ("group:G1", "Hi")
        );
        assert!(!message.direct && message.mentions == ["qq-bot"]);
        let channel = json!({"t": "AT_MESSAGE_CREATE", "d": {"id": "m3", "channel_id": "C1",
            "author": {"id": "A1"}, "content": "<@!123> Hello there"}});
        assert_eq!(qq_message(&channel).unwrap().text, "Hello there");
        let bot = json!({"t": "AT_MESSAGE_CREATE", "d": {"id": "m4", "channel_id": "C1",
            "author": {"id": "B1", "bot": true}, "content": "x"}});
        assert!(qq_message(&bot).is_none());
        assert_eq!(Qq::path("group:G1").unwrap(), "/v2/groups/G1/messages");
        assert!(Qq::path("G1").is_err() && Qq::path("user:../x").is_err());
    }
}
