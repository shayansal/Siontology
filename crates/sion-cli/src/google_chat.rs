//! Google Chat, as a Chat app with an HTTP endpoint: Chat sends what is
//! said to the bridge as webhooks (`--listen`), each with a token Google
//! signs (RS256) for the app, and the bridge answers in the webhook's own
//! answer, within Chat's thirty seconds, or, with a service account's key,
//! through the Chat API, whenever the answer comes.
//!
//! | Variable | What it is |
//! |---|---|
//! | `GOOGLE_CHAT_AUDIENCE` | the app's project number, or its endpoint URL, as its Authentication Audience is set |
//! | `GOOGLE_CHAT_CREDENTIALS` | a service account's key file: answers are sent through the Chat API, and voice notes fetched |
//! | `GOOGLE_CHAT_SENDER` | with the endpoint URL as audience, whose tokens are taken: `chat@system.gserviceaccount.com`, or a Workspace add-on's service account |
//!
//! With the project number, a token is taken if it is signed by a key of
//! `chat@system.gserviceaccount.com`, which issued it, for that number;
//! with the URL, if it is a Google ID token (`accounts.google.com`) for
//! the URL, of the sender's verified address. Events come in either of
//! Chat's shapes: an interaction event, or a Workspace add-on's.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::jwt::{Bearer, KeySet};
use crate::live::Live;
use crate::messengers::{Incoming, Messenger, Note, LIVE_ONCE_WAIT};
use crate::showing::{with_status, Draft, Drafting};
use crate::voice::{Audio, MAX_HEARD};
use crate::webhooks::{
    client, cut, encode_form, env, inbox_holding, json_of, needed, respond, Answer, Handled, Held,
    Hook, Seen,
};

/// Who signs Chat's own tokens (the project number as audience).
pub const CHAT: &str = "chat@system.gserviceaccount.com";

/// The keys of [`CHAT`].
pub const CHAT_KEYS: &str =
    "https://www.googleapis.com/service_accounts/v1/jwk/chat@system.gserviceaccount.com";

/// Google's keys for its ID tokens (the endpoint URL as audience).
pub const GOOGLE_KEYS: &str = "https://www.googleapis.com/oauth2/v3/certs";

/// The scope of a Chat app's own calls.
const SCOPE: &str = "https://www.googleapis.com/auth/chat.bot";

/// How long a webhook is held open for its answer: Chat waits thirty
/// seconds.
const HOLD: Duration = Duration::from_secs(25);

/// What a webhook is checked with.
#[derive(Debug)]
pub struct Guard {
    /// The project number, or the endpoint URL.
    pub audience: String,
    /// With the URL as audience, whose ID tokens are taken.
    pub sender: String,
    /// The keys.
    pub keys: KeySet,
    /// The client the keys are fetched with.
    pub client: sion_http::Client,
}

impl Guard {
    /// Whether the audience is the project number (rather than the URL).
    fn project(&self) -> bool {
        !self.audience.is_empty() && self.audience.bytes().all(|b| b.is_ascii_digit())
    }

    /// Checks a webhook's token (see the module's documentation).
    pub fn check(&mut self, hook: &Hook, now: u64) -> Result<(), String> {
        let token = hook
            .header("authorization")
            .and_then(|a| {
                a.strip_prefix("Bearer ")
                    .or_else(|| a.strip_prefix("bearer "))
            })
            .unwrap_or_default()
            .trim();
        if token.is_empty() {
            return Err("no token".into());
        }
        let (claims, _) = self.keys.verify(&self.client, token)?;
        if self.project() {
            return crate::jwt::check(&claims, &[CHAT], &self.audience, now);
        }
        crate::jwt::check(
            &claims,
            &["https://accounts.google.com", "accounts.google.com"],
            &self.audience,
            now,
        )?;
        if claims["email"].as_str() != Some(self.sender.as_str())
            || claims["email_verified"] != true
        {
            return Err(format!("a token not of {}", self.sender));
        }
        Ok(())
    }
}

/// What a Chat event says, in either of Chat's shapes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatEvent {
    /// The message, as the bridge takes it.
    pub incoming: Incoming,
    /// The message's name (`spaces/…/messages/…`).
    pub id: String,
    /// Its thread's name, for the answer.
    pub thread: Option<String>,
    /// Whether it came in a Workspace add-on's shape.
    pub add_on: bool,
    /// The names of the apps it mentions.
    pub apps: Vec<String>,
}

/// The message of a Chat event, an interaction event's (`type: MESSAGE`)
/// or a Workspace add-on's (`chat.messagePayload`); `None` for other
/// events, and apps' messages.
pub fn chat_event(event: &Value) -> Option<ChatEvent> {
    let (message, space, add_on) = if event["chat"]["messagePayload"].is_object() {
        let payload = &event["chat"]["messagePayload"];
        (&payload["message"], &payload["space"], true)
    } else if event["type"] == "MESSAGE" {
        let space = if event["space"].is_object() {
            &event["space"]
        } else {
            &event["message"]["space"]
        };
        (&event["message"], space, false)
    } else {
        return None;
    };
    if message["sender"]["type"] == "BOT" {
        return None;
    }
    let chat = space["name"]
        .as_str()
        .or(message["space"]["name"].as_str())?;
    let id = message["name"].as_str()?;
    // What was said to the app: without its mention, where Chat cut it.
    let text = message["argumentText"]
        .as_str()
        .filter(|t| !t.trim().is_empty())
        .or(message["text"].as_str())
        .unwrap_or_default()
        .trim()
        .to_string();
    let voice = message["attachment"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|a| {
            a["contentType"]
                .as_str()
                .is_some_and(|t| t.starts_with("audio/"))
        })
        .and_then(|a| {
            Some(Note {
                id: a["attachmentDataRef"]["resourceName"].as_str()?.to_string(),
                media_type: a["contentType"].as_str().map(str::to_string),
                name: None,
            })
        });
    if text.is_empty() && voice.is_none() {
        return None;
    }
    let mut mentions = Vec::new();
    let mut apps = Vec::new();
    for annotation in message["annotations"].as_array().into_iter().flatten() {
        if annotation["type"] != "USER_MENTION" {
            continue;
        }
        let user = &annotation["userMention"]["user"];
        if let Some(name) = user["name"].as_str() {
            mentions.push(name.to_string());
            if user["type"] == "BOT" {
                apps.push(name.to_string());
                mentions.push("app".to_string());
            }
        }
    }
    let direct = space["type"] == "DM"
        || space["spaceType"] == "DIRECT_MESSAGE"
        || space["singleUserBotDm"] == true;
    Some(ChatEvent {
        incoming: Incoming {
            id: String::new(),
            thread: String::new(),
            chat: chat.to_string(),
            from: message["sender"]["name"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            text,
            voice,
            direct,
            mentions,
            files: Vec::new(),
        },
        id: id.to_string(),
        thread: message["thread"]["name"].as_str().map(str::to_string),
        add_on,
        apps,
    })
}

/// The answer to a webhook that carries the reply, in its event's shape.
pub fn reply_answer(kind: &str, text: &str) -> Answer {
    let message = json!({"text": cut(text, 4_000)});
    Answer::json(
        200,
        &if kind == "add-on" {
            json!({"hostAppDataAction": {"chatDataAction": {"createMessageAction": {"message": message}}}})
        } else {
            message
        },
    )
}

/// Answers one of Chat's webhooks: refused unless its token is right
/// ([`Guard::check`]); a message, passed on, its webhook held open for the
/// answer unless answers go through the Chat API (`asynchronous`).
pub(crate) fn chat_hook(
    hook: &Hook,
    guard: &mut Guard,
    threads: &Mutex<BTreeMap<String, String>>,
    apps: &Mutex<BTreeSet<String>>,
    seen: &mut Seen,
    asynchronous: bool,
) -> Handled {
    let answer = |status: u16, why: &str| Handled {
        answer: Answer::text(status, why),
        messages: Vec::new(),
        hold: None,
    };
    if hook.method != "POST" {
        return answer(405, "only POST");
    }
    if let Err(why) = guard.check(hook, now()) {
        return answer(401, &why);
    }
    let Ok(event) = serde_json::from_slice::<Value>(&hook.body) else {
        return answer(400, "not JSON");
    };
    let nothing = Handled {
        answer: Answer::json(200, &json!({})),
        messages: Vec::new(),
        hold: None,
    };
    let Some(event) = chat_event(&event) else {
        return nothing;
    };
    if !seen.first(&event.id) {
        return nothing;
    }
    if let Some(thread) = &event.thread {
        threads
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(event.incoming.chat.clone(), thread.clone());
    }
    apps.lock()
        .unwrap_or_else(|e| e.into_inner())
        .extend(event.apps);
    let hold = (!asynchronous).then(|| {
        (
            event.incoming.chat.clone(),
            Instant::now() + HOLD,
            if event.add_on { "add-on" } else { "chat" },
        )
    });
    Handled {
        answer: Answer::json(200, &json!({})),
        messages: vec![event.incoming],
        hold,
    }
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// A service account's key, for the Chat API.
struct Account {
    email: String,
    key: String,
    key_id: Option<String>,
    token_uri: String,
}

impl Account {
    fn read(path: &str) -> Result<Account, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("GOOGLE_CHAT_CREDENTIALS ({path}): {e}"))?;
        let value: Value = serde_json::from_str(&text)
            .map_err(|e| format!("GOOGLE_CHAT_CREDENTIALS ({path}): not JSON: {e}"))?;
        let field = |name: &str| {
            value[name].as_str().map(str::to_string).ok_or_else(|| {
                format!("GOOGLE_CHAT_CREDENTIALS ({path}): no {name}: not a service account's key")
            })
        };
        if value["type"] != "service_account" {
            return Err(format!(
                "GOOGLE_CHAT_CREDENTIALS ({path}): not a service account's key"
            ));
        }
        Ok(Account {
            email: field("client_email")?,
            key: field("private_key")?,
            key_id: value["private_key_id"].as_str().map(str::to_string),
            token_uri: value["token_uri"]
                .as_str()
                .unwrap_or("https://oauth2.googleapis.com/token")
                .to_string(),
        })
    }
}

/// A reply as a Chat message patched as it grows (`updateMask=text`).
struct Edit {
    client: sion_http::Client,
    api: String,
    token: String,
    space: String,
    thread: Option<String>,
    /// The message's name, once sent.
    name: Option<String>,
    shown: String,
}

impl Edit {
    fn put(&mut self, text: &str) -> Result<(), String> {
        if self.name.is_some() && self.shown == text {
            return Ok(());
        }
        self.shown = text.to_string();
        let auth = format!("Bearer {}", self.token);
        let headers = [
            ("Authorization", auth.as_str()),
            ("Content-Type", "application/json"),
        ];
        let mut message = json!({"text": cut(text, 4_000)});
        let (method, url) = match &self.name {
            Some(name) => ("PATCH", format!("{}/v1/{name}?updateMask=text", self.api)),
            None => {
                if let Some(thread) = &self.thread {
                    message["thread"] = json!({"name": thread});
                }
                (
                    "POST",
                    format!(
                        "{}/v1/{}/messages?messageReplyOption=REPLY_MESSAGE_FALLBACK_TO_NEW_THREAD",
                        self.api, self.space
                    ),
                )
            }
        };
        let fetched = self
            .client
            .request(
                method,
                &url,
                &headers,
                Some(message.to_string().as_bytes()),
                1 << 20,
            )
            .map_err(|e| e.to_string().replace(&self.token, "<token>"))?;
        if self.name.is_none() {
            let sent = json_of(&fetched.body, "Google Chat")?;
            self.name = sent["name"]
                .as_str()
                .filter(|name| name.starts_with(&format!("{}/messages/", self.space)))
                .map(str::to_string);
        }
        Ok(())
    }
}

impl Draft for Edit {
    fn show(&mut self, text: &str, status: Option<&str>) -> Result<(), String> {
        self.put(&with_status(text, status))
    }

    fn finish(&mut self, text: &str) -> Result<(), String> {
        self.put(text)
    }
}

/// A Chat app.
pub struct GoogleChat {
    client: sion_http::Client,
    api: String,
    account: Option<Account>,
    token: Mutex<Bearer>,
    held: Held,
    threads: Arc<Mutex<BTreeMap<String, String>>>,
    apps: Arc<Mutex<BTreeSet<String>>>,
    live: Live,
}

impl std::fmt::Debug for GoogleChat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "GoogleChat({})",
            self.account
                .as_ref()
                .map_or("answering webhooks", |a| a.email.as_str())
        )
    }
}

/// Whether `name` is a space's (`spaces/AAAA`), safe in a path.
fn space_name(name: &str) -> bool {
    name.strip_prefix("spaces/").is_some_and(|id| {
        !id.is_empty()
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    })
}

impl GoogleChat {
    /// The app `GOOGLE_CHAT_AUDIENCE` names, answering through the Chat API
    /// with `GOOGLE_CHAT_CREDENTIALS` if set, listening on `listen`;
    /// `GOOGLE_CHAT_KEYS_URL` and `GOOGLE_CHAT_API_BASE` point it at other
    /// servers (stand-ins), as the key file's `token_uri` does its tokens.
    pub fn from_env(listen: &str) -> Result<GoogleChat, String> {
        let audience = needed(
            "GOOGLE_CHAT_AUDIENCE",
            "the app's project number, or its endpoint URL",
        )?;
        let account = env("GOOGLE_CHAT_CREDENTIALS")
            .map(|path| Account::read(&path))
            .transpose()?;
        let client = client()?;
        let project = audience.bytes().all(|b| b.is_ascii_digit());
        let keys = env("GOOGLE_CHAT_KEYS_URL")
            .unwrap_or_else(|| if project { CHAT_KEYS } else { GOOGLE_KEYS }.into());
        let mut guard = Guard {
            audience,
            sender: env("GOOGLE_CHAT_SENDER").unwrap_or_else(|| CHAT.into()),
            keys: KeySet::at(&keys),
            client: client.clone(),
        };
        let held: Held = Arc::new(Mutex::new(BTreeMap::new()));
        let threads = Arc::new(Mutex::new(BTreeMap::new()));
        let apps = Arc::new(Mutex::new(BTreeSet::new()));
        let (kept, known) = (threads.clone(), apps.clone());
        let asynchronous = account.is_some();
        let mut seen = Seen::default();
        let live = inbox_holding("google-chat", listen, Some(held.clone()), move |hook| {
            chat_hook(hook, &mut guard, &kept, &known, &mut seen, asynchronous)
        })?;
        Ok(GoogleChat {
            client,
            api: env("GOOGLE_CHAT_API_BASE")
                .unwrap_or_else(|| "https://chat.googleapis.com".into())
                .trim_end_matches('/')
                .to_string(),
            account,
            token: Mutex::new(Bearer::default()),
            held,
            threads,
            apps,
            live,
        })
    }

    /// A token for the Chat API, the service account's (a JWT bearer
    /// grant, RFC 7523).
    fn bearer(&self) -> Result<String, String> {
        let account = self.account.as_ref().ok_or(
            "no service account: set GOOGLE_CHAT_CREDENTIALS to a service account's key file",
        )?;
        self.token
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(|| {
                let now = now();
                let claims = json!({
                    "iss": account.email, "scope": SCOPE, "aud": account.token_uri,
                    "iat": now, "exp": now + 3600,
                });
                let assertion =
                    crate::jwt::signed(&claims, &account.key, account.key_id.as_deref())?;
                let body = encode_form(&[
                    ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
                    ("assertion", &assertion),
                ]);
                let fetched = self
                    .client
                    .request(
                        "POST",
                        &account.token_uri,
                        &[("Content-Type", "application/x-www-form-urlencoded")],
                        Some(body.as_bytes()),
                        1 << 16,
                    )
                    .map_err(|e| format!("the service account's token: {e}"))?;
                let value = json_of(&fetched.body, "Google")?;
                let token = value["access_token"]
                    .as_str()
                    .ok_or("Google gave no access token")?;
                Ok((
                    token.to_string(),
                    value["expires_in"].as_u64().unwrap_or(3600),
                ))
            })
    }
}

impl Messenger for GoogleChat {
    fn name(&self) -> &'static str {
        "google-chat"
    }

    fn chat_word(&self) -> &'static str {
        "space"
    }

    fn receive(&self, cursor: &str, wait: u64) -> Result<(Vec<Incoming>, String), String> {
        Ok((self.live.receive(wait)?, cursor.to_string()))
    }

    fn send(&self, chat: &str, text: &str) -> Result<(), String> {
        // In the webhook's own answer, while Chat waits for it.
        let held = self
            .held
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(chat);
        if let Some(hook) = held {
            respond(hook.request, reply_answer(hook.kind, text));
            return Ok(());
        }
        if self.account.is_none() {
            println!(
                "google-chat: the answer came after Chat stopped waiting for it, and was not sent: GOOGLE_CHAT_CREDENTIALS lets the bridge answer later"
            );
            return Ok(());
        }
        if !space_name(chat) {
            return Err(format!("{chat} is not a space's name"));
        }
        let token = self.bearer()?;
        let mut message = json!({"text": cut(text, 4_000)});
        let thread = self
            .threads
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(chat)
            .cloned();
        if let Some(thread) = thread {
            message["thread"] = json!({"name": thread});
        }
        let auth = format!("Bearer {token}");
        self.client
            .request(
                "POST",
                &format!(
                    "{}/v1/{chat}/messages?messageReplyOption=REPLY_MESSAGE_FALLBACK_TO_NEW_THREAD",
                    self.api
                ),
                &[
                    ("Authorization", &auth),
                    ("Content-Type", "application/json"),
                ],
                Some(message.to_string().as_bytes()),
                1 << 20,
            )
            .map_err(|e| e.to_string().replace(&token, "<token>"))?;
        Ok(())
    }

    fn room(&self) -> usize {
        4_000
    }

    /// Through the Chat API only (a webhook's answer is the reply whole): a
    /// message patched as it grows, at most one write a second and a half,
    /// for Chat takes one a second in a space.
    fn drafting(&self, message: &Incoming) -> Option<Drafting> {
        self.account.as_ref()?;
        if !space_name(&message.chat) {
            return None;
        }
        let token = self.bearer().ok()?;
        let thread = self
            .threads
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&message.chat)
            .cloned();
        let (client, api, space) = (self.client.clone(), self.api.clone(), message.chat.clone());
        Some(Drafting {
            open: Box::new(move || {
                Box::new(Edit {
                    client: client.clone(),
                    api: api.clone(),
                    token: token.clone(),
                    space: space.clone(),
                    thread: thread.clone(),
                    name: None,
                    shown: String::new(),
                })
            }),
            every: Duration::from_millis(1_500),
            most: None,
            keep: None,
            first: crate::drafts::FIRST,
        })
    }

    fn long_polls(&self) -> bool {
        true
    }

    fn once_wait(&self) -> u64 {
        LIVE_ONCE_WAIT
    }

    fn me(&self) -> Result<Vec<String>, String> {
        let mut names = vec!["app".to_string()];
        names.extend(
            self.apps
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .cloned(),
        );
        Ok(names)
    }

    fn fetch_voice(&self, note: &Note) -> Result<Audio, String> {
        if note.id.is_empty()
            || note.id.contains("..")
            || !note
                .id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_./=".contains(&b))
        {
            return Err("not an attachment's name".into());
        }
        let token = self.bearer()?;
        let auth = format!("Bearer {token}");
        let fetched = self
            .client
            .request(
                "GET",
                &format!("{}/v1/media/{}?alt=media", self.api, note.id),
                &[("Authorization", &auth)],
                None,
                MAX_HEARD as u64,
            )
            .map_err(|e| e.to_string().replace(&token, "<token>"))?;
        let media_type = note
            .media_type
            .clone()
            .unwrap_or_else(|| Audio::sniff(&fetched.body).to_string());
        Ok(Audio::new(&media_type, fetched.body))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_shapes_of_event_are_read() {
        let interaction = json!({
            "type": "MESSAGE",
            "space": {"name": "spaces/AAA", "type": "ROOM", "spaceType": "SPACE"},
            "message": {
                "name": "spaces/AAA/messages/m1",
                "sender": {"name": "users/111", "type": "HUMAN"},
                "text": "@Lead How much budget is left?",
                "argumentText": " How much budget is left?",
                "thread": {"name": "spaces/AAA/threads/t1"},
                "annotations": [{"type": "USER_MENTION", "userMention": {"user": {"name": "users/999", "type": "BOT"}}}]
            }
        });
        let event = chat_event(&interaction).unwrap();
        assert_eq!(event.incoming.chat, "spaces/AAA");
        assert_eq!(event.incoming.from, "users/111");
        assert_eq!(event.incoming.text, "How much budget is left?");
        assert!(!event.incoming.direct);
        assert_eq!(event.incoming.mentions, ["users/999", "app"]);
        assert_eq!(event.thread.as_deref(), Some("spaces/AAA/threads/t1"));
        assert!(!event.add_on);
        let add_on = json!({"chat": {"user": {"name": "users/111"}, "messagePayload": {
            "space": {"name": "spaces/DM1", "spaceType": "DIRECT_MESSAGE"},
            "message": {"name": "spaces/DM1/messages/m2", "sender": {"name": "users/111", "type": "HUMAN"},
                "text": "Listen", "attachment": [{"contentType": "audio/ogg",
                    "attachmentDataRef": {"resourceName": "spaces/DM1/attachments/a1"}}]}}}});
        let event = chat_event(&add_on).unwrap();
        assert!(event.add_on && event.incoming.direct);
        assert_eq!(
            event.incoming.voice,
            Some(Note {
                id: "spaces/DM1/attachments/a1".into(),
                media_type: Some("audio/ogg".into()),
                name: None,
            })
        );
        // Other events, and apps' own messages, are not talk.
        assert!(
            chat_event(&json!({"type": "ADDED_TO_SPACE", "space": {"name": "spaces/AAA"}}))
                .is_none()
        );
        let mut from_app = interaction.clone();
        from_app["message"]["sender"]["type"] = json!("BOT");
        assert!(chat_event(&from_app).is_none());
        assert_eq!(
            String::from_utf8(reply_answer("add-on", "Hi").body).unwrap(),
            r#"{"hostAppDataAction":{"chatDataAction":{"createMessageAction":{"message":{"text":"Hi"}}}}}"#
        );
        assert!(
            space_name("spaces/AAA_b-1") && !space_name("spaces/../x") && !space_name("users/1")
        );
    }
}
