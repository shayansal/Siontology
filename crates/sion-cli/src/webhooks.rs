//! Messengers that push what is said to the bridge as signed webhooks:
//! WhatsApp (Meta's Cloud API), LINE (the Messaging API) and SMS (Twilio).
//!
//! The bridge listens on an address of its own (`--listen`,
//! [`LISTEN`] by default), which the operator exposes to the service, behind
//! TLS. Every webhook is checked against the service's signature with the
//! secret the operator gives, and an unsigned or wrongly signed one is
//! refused before anything reaches the profile. A service sends again what
//! it thinks was not received: each message is passed on once, by its id.
//!
//! | `--via` | Signature | Sends with | Voice notes |
//! |---|---|---|---|
//! | `whatsapp` | `X-Hub-Signature-256`: HMAC-SHA256 of the body, the app secret's | the Cloud API's `/messages` | heard; spoken as an audio message |
//! | `line` | `X-Line-Signature`: HMAC-SHA256 of the body, base64, the channel secret's | a reply within 50 seconds, a push after | heard; replies in text (LINE plays audio only from a public address) |
//! | `sms` | `X-Twilio-Signature`: HMAC-SHA1 of the URL and the sorted form, base64, the auth token's | Twilio's `Messages` | MMS audio heard; replies in text |

use std::collections::{BTreeMap, VecDeque};
use std::io::Read;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::drafts::Poster;
use crate::live::{Link, Live};
use crate::messengers::{Ack, Incoming, Messenger, Note, LIVE_ONCE_WAIT};
use crate::showing::Typist;
use crate::voice::{Audio, Form, MAX_HEARD};

/// Where a bridge listens for webhooks unless told otherwise.
pub const LISTEN: &str = "127.0.0.1:8797";

/// The longest webhook taken, in bytes.
const MAX_HOOK: u64 = 1 << 20;

/// How many message ids are remembered, so a message sent again is not
/// answered twice.
const REMEMBERED: usize = 1000;

/// A webhook as it came.
#[derive(Clone, Debug)]
pub struct Hook {
    /// `GET` or `POST`.
    pub method: String,
    /// The path and query.
    pub url: String,
    /// Its headers, names lower-cased.
    pub headers: Vec<(String, String)>,
    /// Its body.
    pub body: Vec<u8>,
}

impl Hook {
    /// The header `name` (lower-case).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    /// The query's value for `name`, decoded.
    pub fn query(&self, name: &str) -> Option<String> {
        let query = self.url.split_once('?')?.1;
        decode_form(query.as_bytes())
            .into_iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v)
    }
}

/// What a webhook is answered with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Answer {
    /// The status.
    pub status: u16,
    /// The body's media type.
    pub content_type: &'static str,
    /// The body.
    pub body: Vec<u8>,
}

impl Answer {
    pub(crate) fn ok() -> Answer {
        Answer::text(200, "ok")
    }

    pub(crate) fn text(status: u16, body: &str) -> Answer {
        Answer {
            status,
            content_type: "text/plain; charset=utf-8",
            body: body.as_bytes().to_vec(),
        }
    }

    pub(crate) fn json(status: u16, body: &Value) -> Answer {
        Answer {
            status,
            content_type: "application/json",
            body: body.to_string().into_bytes(),
        }
    }
}

/// The ids recently passed on.
#[derive(Debug, Default)]
pub struct Seen(VecDeque<String>);

impl Seen {
    /// Whether `id` comes for the first time (and keeps it).
    pub fn first(&mut self, id: &str) -> bool {
        if self.0.iter().any(|seen| seen == id) {
            return false;
        }
        self.0.push_back(id.to_string());
        if self.0.len() > REMEMBERED {
            self.0.pop_front();
        }
        true
    }
}

/// `a=b&c=d`, decoded (`+` as a space, `%XX` as its byte).
pub fn decode_form(body: &[u8]) -> Vec<(String, String)> {
    let decode = |text: &str| {
        let bytes = text.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut i = 0;
        while i < bytes.len() {
            let hex = |b: u8| char::from(b).to_digit(16);
            match bytes[i] {
                b'+' => out.push(b' '),
                b'%' => match (
                    bytes.get(i + 1).copied().and_then(hex),
                    bytes.get(i + 2).copied().and_then(hex),
                ) {
                    (Some(h), Some(l)) => {
                        out.push((h * 16 + l) as u8);
                        i += 2;
                    }
                    _ => out.push(b'%'),
                },
                b => out.push(b),
            }
            i += 1;
        }
        String::from_utf8_lossy(&out).into_owned()
    };
    String::from_utf8_lossy(body)
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((name, value)) => (decode(name), decode(value)),
            None => (decode(pair), String::new()),
        })
        .collect()
}

/// `fields` form-encoded.
pub(crate) fn encode_form(fields: &[(&str, &str)]) -> String {
    let encode = |text: &str| {
        text.bytes()
            .map(|b| {
                if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                    char::from(b).to_string()
                } else {
                    format!("%{b:02X}")
                }
            })
            .collect::<String>()
    };
    fields
        .iter()
        .map(|(n, v)| format!("{}={}", encode(n), encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// Listens on `listen`, answering each webhook as `handle` says and
/// passing on the messages it finds; the address is taken at once, so a
/// bridge that cannot listen says so before it starts.
pub fn inbox(
    name: &str,
    listen: &str,
    mut handle: impl FnMut(&Hook) -> (Answer, Vec<Incoming>) + Send + 'static,
) -> Result<Live, String> {
    inbox_holding(name, listen, None, move |hook| {
        let (answer, messages) = handle(hook);
        Handled {
            answer,
            messages,
            hold: None,
        }
    })
}

/// A webhook held open for the reply to its chat ([`inbox_holding`]).
pub(crate) struct HeldHook {
    pub(crate) request: tiny_http::Request,
    /// When to stop waiting, and answer with `fallback`.
    pub(crate) until: Instant,
    pub(crate) fallback: Answer,
    /// What kind of webhook it is, for the shape of the reply.
    pub(crate) kind: &'static str,
}

/// The webhooks held open, by chat.
pub(crate) type Held = Arc<Mutex<BTreeMap<String, HeldHook>>>;

/// What a webhook is answered with, and what it brought.
pub(crate) struct Handled {
    pub(crate) answer: Answer,
    pub(crate) messages: Vec<Incoming>,
    /// To hold the webhook open for the reply to a chat: the chat, until
    /// when (then answered with `answer`), and its kind.
    pub(crate) hold: Option<(String, Instant, &'static str)>,
}

/// [`inbox`], where a webhook can be held open for its reply, in `held`,
/// for a service that takes the reply as the webhook's answer.
pub(crate) fn inbox_holding(
    name: &str,
    listen: &str,
    held: Option<Held>,
    handle: impl FnMut(&Hook) -> Handled + Send + 'static,
) -> Result<Live, String> {
    let server = tiny_http::Server::http(listen).map_err(|e| format!("{listen}: {e}"))?;
    println!("{name}: webhooks on http://{listen}/");
    Ok(Live::start(name, move |link| {
        serve(server, &link, held.as_ref(), handle)
    }))
}

/// Answers a webhook.
pub(crate) fn respond(request: tiny_http::Request, answer: Answer) {
    let header =
        tiny_http::Header::from_bytes(&b"Content-Type"[..], answer.content_type).expect("a header");
    let _ = request.respond(
        tiny_http::Response::from_data(answer.body)
            .with_status_code(answer.status)
            .with_header(header),
    );
}

/// Answers the webhooks held past their time, or all of them.
fn release(held: &Held, all: bool) {
    let mut held = held.lock().unwrap_or_else(|e| e.into_inner());
    let now = Instant::now();
    let done: Vec<String> = held
        .iter()
        .filter(|(_, hook)| all || hook.until <= now)
        .map(|(chat, _)| chat.clone())
        .collect();
    for chat in done {
        if let Some(hook) = held.remove(&chat) {
            respond(hook.request, hook.fallback);
        }
    }
}

fn serve(
    server: tiny_http::Server,
    link: &Link,
    held: Option<&Held>,
    mut handle: impl FnMut(&Hook) -> Handled,
) {
    'serving: while !link.stopped() {
        if let Some(held) = held {
            release(held, false);
        }
        let mut request = match server.recv_timeout(Duration::from_millis(200)) {
            Ok(Some(request)) => request,
            Ok(None) => continue,
            Err(e) => {
                link.fail(format!("the webhook listener: {e}"));
                return;
            }
        };
        let mut body = Vec::new();
        let read = request
            .as_reader()
            .take(MAX_HOOK + 1)
            .read_to_end(&mut body);
        if read.is_err() || body.len() as u64 > MAX_HOOK {
            respond(request, Answer::text(413, "too long"));
            continue;
        }
        let hook = Hook {
            method: request.method().as_str().to_string(),
            url: request.url().to_string(),
            headers: request
                .headers()
                .iter()
                .map(|h| {
                    (
                        h.field.as_str().as_str().to_ascii_lowercase(),
                        h.value.as_str().to_string(),
                    )
                })
                .collect(),
            body,
        };
        let handled = handle(&hook);
        match (handled.hold, held) {
            (Some((chat, until, kind)), Some(held)) => {
                // Held before the message is passed on, for its reply.
                let earlier = held.lock().unwrap_or_else(|e| e.into_inner()).insert(
                    chat,
                    HeldHook {
                        request,
                        until,
                        fallback: handled.answer,
                        kind,
                    },
                );
                if let Some(earlier) = earlier {
                    respond(earlier.request, earlier.fallback);
                }
            }
            _ => respond(request, handled.answer),
        }
        for message in handled.messages {
            if !link.pass(message) {
                break 'serving;
            }
        }
    }
    if let Some(held) = held {
        release(held, true);
    }
}

pub(crate) fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

pub(crate) fn needed(name: &str, what: &str) -> Result<String, String> {
    env(name).ok_or_else(|| format!("set {name} ({what})"))
}

pub(crate) fn client() -> Result<sion_http::Client, String> {
    sion_http::Client::new(sion_http::Options {
        timeout: Duration::from_secs(60),
        // The operator names the service's address.
        allow_private: true,
        ..sion_http::Options::default()
    })
    .map_err(|e| e.to_string())
}

pub(crate) fn json_of(body: &[u8], service: &str) -> Result<Value, String> {
    serde_json::from_slice(body)
        .map_err(|e| format!("{service} answered with something other than JSON: {e}"))
}

pub(crate) fn cut(text: &str, chars: usize) -> String {
    let text: String = text.chars().take(chars).collect();
    if text.is_empty() {
        "(nothing)".into()
    } else {
        text
    }
}

/// The scheme and authority of `url`.
fn origin(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    (!authority.is_empty()).then(|| format!("{scheme}://{authority}"))
}

/// The host of `url`, lower-cased.
fn host(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.rsplit('@').next()?.split(':').next()?;
    Some(host.to_ascii_lowercase())
}

/// `audio` of the media type a service gave, or its bytes show.
fn audio_of(media_type: Option<&str>, bytes: Vec<u8>) -> Audio {
    let media_type = media_type
        .map(str::to_string)
        .unwrap_or_else(|| Audio::sniff(&bytes).to_string());
    Audio::new(&media_type, bytes)
}

/// Whether HMAC-SHA256 of `body` under `secret` is `signature`.
fn signed_sha256(secret: &str, body: &[u8], signature: &[u8]) -> bool {
    sion_http::same_bytes(&sion_http::hmac_sha256(secret.as_bytes(), body), signature)
}

/// WhatsApp, through Meta's Cloud API: webhooks in, `/messages` out.
pub struct WhatsApp {
    client: sion_http::Client,
    base: String,
    token: String,
    phone: String,
    live: Live,
}

impl std::fmt::Debug for WhatsApp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "WhatsApp({}, {})", self.base, self.phone)
    }
}

/// Answers one of WhatsApp's webhooks: Meta's check of the address (its
/// challenge, given the verify token), or messages signed with the app
/// secret, for the business number `phone`.
pub fn whatsapp_hook(
    hook: &Hook,
    secret: &str,
    verify: &str,
    phone: &str,
    seen: &mut Seen,
) -> (Answer, Vec<Incoming>) {
    if hook.method == "GET" {
        let token = hook.query("hub.verify_token").unwrap_or_default();
        return match (
            hook.query("hub.mode").as_deref(),
            hook.query("hub.challenge"),
        ) {
            (Some("subscribe"), Some(challenge))
                if sion_http::same_bytes(token.as_bytes(), verify.as_bytes()) =>
            {
                (Answer::text(200, &challenge), Vec::new())
            }
            _ => (Answer::text(403, "not the verify token"), Vec::new()),
        };
    }
    let signature = hook
        .header("x-hub-signature-256")
        .and_then(|s| s.strip_prefix("sha256="))
        .and_then(|h| hex::decode(h).ok())
        .unwrap_or_default();
    if !signed_sha256(secret, &hook.body, &signature) {
        return (
            Answer::text(401, "not signed with the app secret"),
            Vec::new(),
        );
    }
    let Ok(value) = serde_json::from_slice::<Value>(&hook.body) else {
        return (Answer::text(400, "not JSON"), Vec::new());
    };
    let mut out = Vec::new();
    for change in value["entry"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|entry| entry["changes"].as_array().into_iter().flatten())
    {
        let value = &change["value"];
        if value["metadata"]["phone_number_id"]
            .as_str()
            .is_some_and(|p| p != phone)
        {
            continue;
        }
        for message in value["messages"].as_array().into_iter().flatten() {
            let (Some(id), Some(from)) = (message["id"].as_str(), message["from"].as_str()) else {
                continue;
            };
            if !seen.first(id) {
                continue;
            }
            let voice = (message["type"] == "audio")
                .then(|| message["audio"]["id"].as_str())
                .flatten()
                .map(|media| Note {
                    id: media.to_string(),
                    media_type: message["audio"]["mime_type"].as_str().map(str::to_string),
                    name: None,
                });
            // A picture, a video, a document or a sticker, with its caption.
            let kind = message["type"].as_str().unwrap_or_default();
            let media = &message[kind];
            let files: Vec<Note> = matches!(kind, "image" | "video" | "document" | "sticker")
                .then(|| media["id"].as_str())
                .flatten()
                .map(|media_id| Note {
                    id: media_id.to_string(),
                    media_type: media["mime_type"].as_str().map(str::to_string),
                    name: media["filename"].as_str().map(str::to_string),
                })
                .into_iter()
                .collect();
            let text = message["text"]["body"]
                .as_str()
                .or_else(|| {
                    (!files.is_empty())
                        .then(|| media["caption"].as_str())
                        .flatten()
                })
                .unwrap_or_default();
            if text.is_empty() && voice.is_none() && files.is_empty() {
                continue;
            }
            out.push(Incoming {
                id: id.to_string(),
                thread: String::new(),
                chat: from.to_string(),
                from: from.to_string(),
                text: text.to_string(),
                voice,
                direct: true,
                mentions: Vec::new(),
                files,
            });
        }
    }
    (Answer::ok(), out)
}

impl WhatsApp {
    /// The business number `WHATSAPP_PHONE_NUMBER_ID`, with the access
    /// token in `WHATSAPP_TOKEN`, its webhooks checked against
    /// `WHATSAPP_APP_SECRET` and `WHATSAPP_VERIFY_TOKEN`, at
    /// `WHATSAPP_API_BASE` (default `https://graph.facebook.com/v23.0`),
    /// listening on `listen`.
    pub fn from_env(listen: &str) -> Result<WhatsApp, String> {
        let token = needed("WHATSAPP_TOKEN", "a Cloud API access token")?;
        let phone = needed("WHATSAPP_PHONE_NUMBER_ID", "the business number's id")?;
        let secret = needed(
            "WHATSAPP_APP_SECRET",
            "the app secret webhooks are signed with",
        )?;
        let verify = needed(
            "WHATSAPP_VERIFY_TOKEN",
            "what Meta's check of the webhook gives",
        )?;
        let base = env("WHATSAPP_API_BASE")
            .unwrap_or_else(|| "https://graph.facebook.com/v23.0".into())
            .trim_end_matches('/')
            .to_string();
        let ours = phone.clone();
        let mut seen = Seen::default();
        let live = inbox("whatsapp", listen, move |hook| {
            whatsapp_hook(hook, &secret, &verify, &ours, &mut seen)
        })?;
        Ok(WhatsApp {
            client: client()?,
            base,
            token,
            phone,
            live,
        })
    }

    fn post(&self, path: &str, content_type: &str, body: &[u8]) -> Result<Value, String> {
        let auth = format!("Bearer {}", self.token);
        let fetched = self
            .client
            .request(
                "POST",
                &format!("{}/{path}", self.base),
                &[("Authorization", &auth), ("Content-Type", content_type)],
                Some(body),
                1 << 20,
            )
            .map_err(|e| e.to_string().replace(&self.token, "<token>"))?;
        json_of(&fetched.body, "WhatsApp")
    }

    fn message(&self, message: Value) -> Result<(), String> {
        self.post(
            &format!("{}/messages", self.phone),
            "application/json",
            message.to_string().as_bytes(),
        )
        .map(|_| ())
    }

    /// Whether the token may go to `url`: Meta's media hosts, or the API's.
    fn is_meta(&self, url: &str) -> bool {
        host(url).is_some_and(|h| {
            h.ends_with(".fbsbx.com")
                || h.ends_with(".facebook.com")
                || h.ends_with(".whatsapp.net")
        }) || origin(&self.base).is_some_and(|own| url.starts_with(&format!("{own}/")))
    }
}

impl Messenger for WhatsApp {
    /// Read, as a person's app shows it; a reaction on what failed.
    fn acknowledge(&self, message: &Incoming, ack: Ack) -> Result<(), String> {
        match ack {
            Ack::Seen => self.message(json!({"messaging_product": "whatsapp", "status": "read",
                "message_id": message.id})),
            Ack::Done => Ok(()),
            Ack::Failed => self.message(json!({"messaging_product": "whatsapp",
                "recipient_type": "individual", "to": message.chat, "type": "reaction",
                "reaction": {"message_id": message.id, "emoji": crate::messengers::FAILED}})),
        }
    }

    fn name(&self) -> &'static str {
        "whatsapp"
    }

    fn chat_word(&self) -> &'static str {
        "number"
    }

    fn receive(&self, cursor: &str, wait: u64) -> Result<(Vec<Incoming>, String), String> {
        Ok((self.live.receive(wait)?, cursor.to_string()))
    }

    fn send(&self, chat: &str, text: &str) -> Result<(), String> {
        self.message(json!({"messaging_product": "whatsapp", "to": chat,
            "type": "text", "text": {"body": cut(text, 4_096)}}))
    }

    fn room(&self) -> usize {
        4_096
    }

    /// The message marked read, with WhatsApp's typing indicator: 25
    /// seconds at a time, or until the reply.
    fn typist(&self, message: &Incoming, _aloud: bool) -> Option<Box<dyn Typist>> {
        (!message.id.is_empty()).then(|| -> Box<dyn Typist> {
            Box::new(Poster {
                client: self.client.clone(),
                url: format!("{}/{}/messages", self.base, self.phone),
                auth: format!("Bearer {}", self.token),
                secret: self.token.clone(),
                body: json!({"messaging_product": "whatsapp", "status": "read",
                    "message_id": message.id, "typing_indicator": {"type": "text"}}),
                lasts: Duration::from_secs(20),
            })
        })
    }

    fn long_polls(&self) -> bool {
        true
    }

    fn once_wait(&self) -> u64 {
        LIVE_ONCE_WAIT
    }

    fn fetch_voice(&self, note: &Note) -> Result<Audio, String> {
        if note.id.is_empty() || note.id.contains(['/', '?', '#']) {
            return Err("not a media id".into());
        }
        let auth = format!("Bearer {}", self.token);
        let get = |url: &str, most: u64| {
            self.client
                .request("GET", url, &[("Authorization", &auth)], None, most)
                .map_err(|e| e.to_string().replace(&self.token, "<token>"))
        };
        let media = json_of(
            &get(&format!("{}/{}", self.base, note.id), 1 << 20)?.body,
            "WhatsApp",
        )?;
        let url = media["url"]
            .as_str()
            .ok_or("WhatsApp gave no address for the media")?;
        if !self.is_meta(url) {
            return Err("media not on Meta's hosts is not fetched with the token".into());
        }
        let fetched = get(url, MAX_HEARD as u64)?;
        Ok(audio_of(
            media["mime_type"].as_str().or(note.media_type.as_deref()),
            fetched.body,
        ))
    }

    fn send_voice(&self, chat: &str, audio: &Audio, caption: &str) -> Result<bool, String> {
        let id = self.upload(&audio.name, &audio.media_type, &audio.bytes)?;
        self.message(json!({"messaging_product": "whatsapp", "to": chat,
            "type": "audio", "audio": {"id": id}}))?;
        // An audio message has no caption: the reply follows in text.
        self.send(chat, caption)?;
        Ok(true)
    }

    /// A JPEG or PNG as a picture, anything else as a document.
    fn send_file(
        &self,
        message: &Incoming,
        file: &crate::files::File,
        caption: &str,
    ) -> Result<bool, String> {
        let id = self.upload(&file.name, &file.media_type, &file.bytes)?;
        let picture = matches!(
            crate::files::sniff_picture(&file.bytes),
            Some("image/jpeg" | "image/png")
        );
        let mut media = json!({"id": id});
        if !caption.is_empty() {
            media["caption"] = json!(cut(caption, 1_024));
        }
        let kind = if picture {
            "image"
        } else {
            media["filename"] = json!(file.name);
            "document"
        };
        self.message(json!({"messaging_product": "whatsapp", "to": message.chat,
            "type": kind, kind: media}))?;
        Ok(true)
    }
}

impl WhatsApp {
    /// Uploads a file to the business number's media: its id.
    fn upload(&self, name: &str, media_type: &str, bytes: &[u8]) -> Result<String, String> {
        let (content_type, body) = Form::new()
            .text("messaging_product", "whatsapp")
            .text("type", media_type)
            .bytes("file", name, media_type, bytes)
            .finish();
        let uploaded = self.post(&format!("{}/media", self.phone), &content_type, &body)?;
        Ok(uploaded["id"]
            .as_str()
            .ok_or("WhatsApp gave no media id")?
            .to_string())
    }
}

/// How long a LINE reply token is used, from the message it came with: it
/// lasts about a minute, and a push is sent after.
const LINE_REPLY_SECS: u64 = 50;

/// The reply tokens of the chats that wrote last, and when.
type Replies = Arc<Mutex<BTreeMap<String, (String, Instant)>>>;

/// LINE, through the Messaging API: webhooks in; a reply while the
/// message's token lasts, a push after.
pub struct Line {
    client: sion_http::Client,
    base: String,
    data: String,
    token: String,
    replies: Replies,
    live: Live,
    me: OnceLock<Vec<String>>,
}

impl std::fmt::Debug for Line {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Line({})", self.base)
    }
}

/// Answers one of LINE's webhooks, signed with the channel secret: its
/// messages, each chat's reply token kept in `replies`.
pub fn line_hook(
    hook: &Hook,
    secret: &str,
    replies: &Replies,
    seen: &mut Seen,
) -> (Answer, Vec<Incoming>) {
    use base64::Engine;
    let signature = hook
        .header("x-line-signature")
        .and_then(|s| base64::engine::general_purpose::STANDARD.decode(s).ok())
        .unwrap_or_default();
    if !signed_sha256(secret, &hook.body, &signature) {
        return (
            Answer::text(401, "not signed with the channel secret"),
            Vec::new(),
        );
    }
    let Ok(value) = serde_json::from_slice::<Value>(&hook.body) else {
        return (Answer::text(400, "not JSON"), Vec::new());
    };
    let mut out = Vec::new();
    for event in value["events"].as_array().into_iter().flatten() {
        if event["type"] != "message" {
            continue;
        }
        let id = event["webhookEventId"]
            .as_str()
            .or(event["message"]["id"].as_str())
            .unwrap_or_default();
        if id.is_empty() || !seen.first(id) {
            continue;
        }
        let source = &event["source"];
        let direct = source["type"] == "user";
        let chat = match source["type"].as_str() {
            Some("group") => source["groupId"].as_str(),
            Some("room") => source["roomId"].as_str(),
            _ => source["userId"].as_str(),
        };
        let Some(chat) = chat else { continue };
        let message = &event["message"];
        let voice = (message["type"] == "audio")
            .then(|| message["id"].as_str())
            .flatten()
            .map(|id| Note {
                id: id.to_string(),
                media_type: Some("audio/m4a".into()),
                name: None,
            });
        // A picture, a video or a file, fetched by the message's id as a
        // voice note is; what it is, its bytes say.
        let files: Vec<Note> = matches!(message["type"].as_str(), Some("image" | "video" | "file"))
            .then(|| message["id"].as_str())
            .flatten()
            .map(|id| Note {
                id: id.to_string(),
                media_type: None,
                name: message["fileName"].as_str().map(str::to_string),
            })
            .into_iter()
            .collect();
        let text = message["text"].as_str().unwrap_or_default();
        if text.is_empty() && voice.is_none() && files.is_empty() {
            continue;
        }
        let mut mentions = Vec::new();
        for mentioned in message["mention"]["mentionees"]
            .as_array()
            .into_iter()
            .flatten()
        {
            if mentioned["isSelf"] == true {
                mentions.push("self".to_string());
            }
            if let Some(user) = mentioned["userId"].as_str() {
                mentions.push(user.to_string());
            }
        }
        if let (Some(token), Ok(mut replies)) = (event["replyToken"].as_str(), replies.lock()) {
            replies.insert(chat.to_string(), (token.to_string(), Instant::now()));
        }
        out.push(Incoming {
            id: message["id"].as_str().unwrap_or_default().to_string(),
            thread: String::new(),
            chat: chat.to_string(),
            from: source["userId"].as_str().unwrap_or_default().to_string(),
            text: text.to_string(),
            voice,
            direct,
            mentions,
            files,
        });
    }
    (Answer::ok(), out)
}

impl Line {
    /// The channel whose access token is in `LINE_CHANNEL_ACCESS_TOKEN`,
    /// its webhooks checked against `LINE_CHANNEL_SECRET`, at
    /// `LINE_API_BASE` (default `https://api.line.me`) and
    /// `LINE_DATA_API_BASE` (`https://api-data.line.me`), listening on
    /// `listen`.
    pub fn from_env(listen: &str) -> Result<Line, String> {
        let token = needed("LINE_CHANNEL_ACCESS_TOKEN", "the channel's access token")?;
        let secret = needed("LINE_CHANNEL_SECRET", "the secret webhooks are signed with")?;
        let base = env("LINE_API_BASE")
            .unwrap_or_else(|| "https://api.line.me".into())
            .trim_end_matches('/')
            .to_string();
        let data = env("LINE_DATA_API_BASE")
            .unwrap_or_else(|| "https://api-data.line.me".into())
            .trim_end_matches('/')
            .to_string();
        let replies: Replies = Arc::new(Mutex::new(BTreeMap::new()));
        let kept = replies.clone();
        let mut seen = Seen::default();
        let live = inbox("line", listen, move |hook| {
            line_hook(hook, &secret, &kept, &mut seen)
        })?;
        Ok(Line {
            client: client()?,
            base,
            data,
            token,
            replies,
            live,
            me: OnceLock::new(),
        })
    }

    fn call(&self, method: &str, url: &str, body: Option<&Value>) -> Result<Value, String> {
        let auth = format!("Bearer {}", self.token);
        let bytes = body.map(|b| b.to_string().into_bytes());
        let mut headers = vec![("Authorization", auth.as_str())];
        if bytes.is_some() {
            headers.push(("Content-Type", "application/json"));
        }
        let fetched = self
            .client
            .request(method, url, &headers, bytes.as_deref(), 1 << 20)
            .map_err(|e| e.to_string().replace(&self.token, "<token>"))?;
        if fetched.body.is_empty() {
            return Ok(Value::Null);
        }
        json_of(&fetched.body, "LINE")
    }
}

impl Messenger for Line {
    fn name(&self) -> &'static str {
        "line"
    }

    fn receive(&self, cursor: &str, wait: u64) -> Result<(Vec<Incoming>, String), String> {
        Ok((self.live.receive(wait)?, cursor.to_string()))
    }

    fn send(&self, chat: &str, text: &str) -> Result<(), String> {
        let messages = json!([{"type": "text", "text": cut(text, 5_000)}]);
        // A reply token is used once, while it lasts; then a push.
        let token = self.replies.lock().ok().and_then(|mut r| r.remove(chat));
        match token {
            Some((token, at)) if at.elapsed() < Duration::from_secs(LINE_REPLY_SECS) => self
                .call(
                    "POST",
                    &format!("{}/v2/bot/message/reply", self.base),
                    Some(&json!({"replyToken": token, "messages": messages})),
                )
                .map(|_| ()),
            _ => self
                .call(
                    "POST",
                    &format!("{}/v2/bot/message/push", self.base),
                    Some(&json!({"to": chat, "messages": messages})),
                )
                .map(|_| ()),
        }
    }

    fn room(&self) -> usize {
        5_000
    }

    /// LINE's loading animation, in a chat with one person (it shows
    /// nowhere else): 60 seconds at a time, or until the reply.
    fn typist(&self, message: &Incoming, _aloud: bool) -> Option<Box<dyn Typist>> {
        message.direct.then(|| -> Box<dyn Typist> {
            Box::new(Poster {
                client: self.client.clone(),
                url: format!("{}/v2/bot/chat/loading/start", self.base),
                auth: format!("Bearer {}", self.token),
                secret: self.token.clone(),
                body: json!({"chatId": message.chat, "loadingSeconds": 60}),
                lasts: Duration::from_secs(55),
            })
        })
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
        let info = self.call("GET", &format!("{}/v2/bot/info", self.base), None)?;
        let mut names = vec!["self".to_string()];
        if let Some(user) = info["userId"].as_str() {
            names.push(user.to_string());
        }
        Ok(self.me.get_or_init(|| names).clone())
    }

    fn fetch_voice(&self, note: &Note) -> Result<Audio, String> {
        if note.id.is_empty() || !note.id.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return Err("not a message id".into());
        }
        let auth = format!("Bearer {}", self.token);
        let fetched = self
            .client
            .request(
                "GET",
                &format!("{}/v2/bot/message/{}/content", self.data, note.id),
                &[("Authorization", &auth)],
                None,
                MAX_HEARD as u64,
            )
            .map_err(|e| e.to_string().replace(&self.token, "<token>"))?;
        Ok(audio_of(note.media_type.as_deref(), fetched.body))
    }
}

/// SMS (and MMS) through Twilio: webhooks in, `Messages` out.
pub struct Sms {
    client: sion_http::Client,
    base: String,
    /// Twilio's Messaging API (`TWILIO_MESSAGING_API_BASE`, by default
    /// `https://messaging.twilio.com`), for typing indicators.
    messaging: String,
    account: String,
    token: String,
    number: String,
    live: Live,
}

impl std::fmt::Debug for Sms {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Sms({}, {})", self.base, self.number)
    }
}

/// Twilio's signature of a webhook to `url` with `form`: HMAC-SHA1 of the
/// URL followed by each field's name and value in name order, base64.
pub fn twilio_signature(token: &str, url: &str, form: &[(String, String)]) -> String {
    let mut fields: Vec<&(String, String)> = form.iter().collect();
    fields.sort();
    let mut signed = url.to_string();
    for (name, value) in fields {
        signed.push_str(name);
        signed.push_str(value);
    }
    sion_http::base64(&sion_http::hmac_sha1(token.as_bytes(), signed.as_bytes()))
}

/// Answers one of Twilio's webhooks, signed with the auth token for the
/// public `url` Twilio was given: its message, with an empty TwiML answer
/// (the reply goes through the API, when the profile has one).
pub fn sms_hook(hook: &Hook, token: &str, url: &str, seen: &mut Seen) -> (Answer, Vec<Incoming>) {
    let form = decode_form(&hook.body);
    let expected = twilio_signature(token, url, &form);
    let given = hook.header("x-twilio-signature").unwrap_or_default();
    if !sion_http::same_bytes(given.as_bytes(), expected.as_bytes()) {
        return (
            Answer::text(401, "not signed with the auth token"),
            Vec::new(),
        );
    }
    let field = |name: &str| {
        form.iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    };
    let twiml = Answer {
        status: 200,
        content_type: "text/xml",
        body: b"<?xml version=\"1.0\" encoding=\"UTF-8\"?><Response></Response>".to_vec(),
    };
    let (Some(id), Some(from)) = (field("MessageSid"), field("From")) else {
        return (twiml, Vec::new());
    };
    if !seen.first(id) {
        return (twiml, Vec::new());
    }
    let count: usize = field("NumMedia").and_then(|n| n.parse().ok()).unwrap_or(0);
    let voice = (0..count.min(10)).find_map(|i| {
        let media_type = field(&format!("MediaContentType{i}"))?;
        media_type.starts_with("audio/").then(|| Note {
            id: field(&format!("MediaUrl{i}"))
                .unwrap_or_default()
                .to_string(),
            media_type: Some(media_type.to_string()),
            name: None,
        })
    });
    // The rest of what came with it (an MMS's pictures) are files.
    let files: Vec<Note> = (0..count.min(10))
        .filter_map(|i| {
            let media_type = field(&format!("MediaContentType{i}"))?;
            let url = field(&format!("MediaUrl{i}"))?;
            (!media_type.starts_with("audio/")).then(|| Note {
                id: url.to_string(),
                media_type: Some(media_type.to_string()),
                name: None,
            })
        })
        .collect();
    let text = field("Body").unwrap_or_default();
    if text.is_empty() && voice.is_none() && files.is_empty() {
        return (twiml, Vec::new());
    }
    (
        twiml,
        vec![Incoming {
            id: id.to_string(),
            thread: String::new(),
            chat: from.to_string(),
            from: from.to_string(),
            text: text.to_string(),
            voice,
            direct: true,
            mentions: Vec::new(),
            files,
        }],
    )
}

impl Sms {
    /// The number `TWILIO_NUMBER` of the account `TWILIO_ACCOUNT_SID`, with
    /// the auth token in `TWILIO_AUTH_TOKEN`; webhooks are checked as sent
    /// to `TWILIO_WEBHOOK_URL`, the public address Twilio is given; at
    /// `TWILIO_API_BASE` (default `https://api.twilio.com`), listening on
    /// `listen`.
    pub fn from_env(listen: &str) -> Result<Sms, String> {
        let account = needed("TWILIO_ACCOUNT_SID", "the account's id")?;
        let token = needed(
            "TWILIO_AUTH_TOKEN",
            "the auth token webhooks are signed with",
        )?;
        let number = needed("TWILIO_NUMBER", "the number messages come from")?;
        let url = needed(
            "TWILIO_WEBHOOK_URL",
            "the public address Twilio sends messages to, as Twilio has it",
        )?;
        let base = env("TWILIO_API_BASE")
            .unwrap_or_else(|| "https://api.twilio.com".into())
            .trim_end_matches('/')
            .to_string();
        let signing = token.clone();
        let mut seen = Seen::default();
        let live = inbox("sms", listen, move |hook| {
            sms_hook(hook, &signing, &url, &mut seen)
        })?;
        Ok(Sms {
            client: client()?,
            base,
            messaging: env("TWILIO_MESSAGING_API_BASE")
                .unwrap_or_else(|| "https://messaging.twilio.com".into())
                .trim_end_matches('/')
                .to_string(),
            account,
            token,
            number,
            live,
        })
    }

    fn basic(&self) -> String {
        format!(
            "Basic {}",
            sion_http::base64(format!("{}:{}", self.account, self.token).as_bytes())
        )
    }
}

impl Messenger for Sms {
    fn name(&self) -> &'static str {
        "sms"
    }

    fn chat_word(&self) -> &'static str {
        "number"
    }

    fn receive(&self, cursor: &str, wait: u64) -> Result<(Vec<Incoming>, String), String> {
        Ok((self.live.receive(wait)?, cursor.to_string()))
    }

    fn send(&self, chat: &str, text: &str) -> Result<(), String> {
        let body = encode_form(&[
            ("To", chat),
            ("From", &self.number),
            ("Body", &cut(text, 1_600)),
        ]);
        self.client
            .request(
                "POST",
                &format!(
                    "{}/2010-04-01/Accounts/{}/Messages.json",
                    self.base, self.account
                ),
                &[
                    ("Authorization", &self.basic()),
                    ("Content-Type", "application/x-www-form-urlencoded"),
                ],
                Some(body.as_bytes()),
                1 << 20,
            )
            .map(|_| ())
            .map_err(|e| e.to_string().replace(&self.token, "<token>"))
    }

    fn room(&self) -> usize {
        1_600
    }

    /// For a WhatsApp sender on Twilio, Twilio's typing indicator (which
    /// marks the message read): 25 seconds at a time, or until the reply.
    /// An SMS shows none.
    fn typist(&self, message: &Incoming, _aloud: bool) -> Option<Box<dyn Typist>> {
        (message.chat.starts_with("whatsapp:") && !message.id.is_empty()).then(
            || -> Box<dyn Typist> {
                Box::new(Poster {
                    client: self.client.clone(),
                    url: format!("{}/v3/Indicators/Typing.json", self.messaging),
                    auth: self.basic(),
                    secret: self.token.clone(),
                    body: json!({"channel": "whatsapp", "messageId": message.id}),
                    lasts: Duration::from_secs(20),
                })
            },
        )
    }

    fn long_polls(&self) -> bool {
        true
    }

    fn once_wait(&self) -> u64 {
        LIVE_ONCE_WAIT
    }

    fn fetch_voice(&self, note: &Note) -> Result<Audio, String> {
        // Twilio's media address takes the account's credentials and sends
        // on to where the file is kept, which takes none.
        if !origin(&self.base).is_some_and(|own| note.id.starts_with(&format!("{own}/"))) {
            return Err("media not on Twilio's API is not fetched with its credentials".into());
        }
        let auth = self.basic();
        let first = self
            .client
            .send("GET", &note.id, &[("Authorization", &auth)], None)
            .map_err(|e| e.to_string())?;
        let response = if (300..400).contains(&first.status) {
            let to = first
                .header("location")
                .ok_or("Twilio sent the media on, but not where")?
                .to_string();
            self.client
                .send("GET", &to, &[], None)
                .map_err(|e| e.to_string())?
        } else {
            first
        };
        if response.status != 200 {
            return Err(format!("Twilio's media answered HTTP {}", response.status));
        }
        let bytes = response
            .read_all(MAX_HEARD as u64)
            .map_err(|e| e.to_string())?;
        Ok(audio_of(note.media_type.as_deref(), bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hook(method: &str, url: &str, headers: &[(&str, &str)], body: &[u8]) -> Hook {
        Hook {
            method: method.into(),
            url: url.into(),
            headers: headers
                .iter()
                .map(|(n, v)| (n.to_string(), v.to_string()))
                .collect(),
            body: body.to_vec(),
        }
    }

    #[test]
    fn whatsapp_hooks_are_checked_and_read() {
        let mut seen = Seen::default();
        let check = hook(
            "GET",
            "/?hub.mode=subscribe&hub.verify_token=v3rify&hub.challenge=1158201444",
            &[],
            b"",
        );
        let (answer, _) = whatsapp_hook(&check, "s3cret", "v3rify", "106", &mut seen);
        assert_eq!((answer.status, answer.body), (200, b"1158201444".to_vec()));
        let wrong = hook(
            "GET",
            "/?hub.mode=subscribe&hub.verify_token=no&hub.challenge=1",
            &[],
            b"",
        );
        assert_eq!(
            whatsapp_hook(&wrong, "s3cret", "v3rify", "106", &mut seen)
                .0
                .status,
            403
        );
        let body = json!({"entry": [{"changes": [{"value": {"metadata": {"phone_number_id": "106"},
            "messages": [{"id": "wamid.1", "from": "15550001111", "type": "text", "text": {"body": "Hi"}},
                         {"id": "wamid.2", "from": "15550001111", "type": "audio",
                          "audio": {"id": "media-9", "mime_type": "audio/ogg; codecs=opus", "voice": true}}]}}]}]})
        .to_string();
        let signature = format!(
            "sha256={}",
            hex::encode(sion_http::hmac_sha256(b"s3cret", body.as_bytes()))
        );
        let signed = hook(
            "POST",
            "/",
            &[("x-hub-signature-256", &signature)],
            body.as_bytes(),
        );
        let (answer, messages) = whatsapp_hook(&signed, "s3cret", "v3rify", "106", &mut seen);
        assert_eq!(answer.status, 200);
        assert_eq!(messages.len(), 2);
        assert_eq!(
            (messages[0].chat.as_str(), messages[0].text.as_str()),
            ("15550001111", "Hi")
        );
        assert_eq!(messages[1].voice.as_ref().unwrap().id, "media-9");
        // Sent again: passed on once.
        assert!(whatsapp_hook(&signed, "s3cret", "v3rify", "106", &mut seen)
            .1
            .is_empty());
        let forged = hook(
            "POST",
            "/",
            &[("x-hub-signature-256", "sha256=00")],
            body.as_bytes(),
        );
        assert_eq!(
            whatsapp_hook(&forged, "s3cret", "v3rify", "106", &mut seen)
                .0
                .status,
            401
        );
        let unsigned = hook("POST", "/", &[], body.as_bytes());
        assert_eq!(
            whatsapp_hook(&unsigned, "s3cret", "v3rify", "106", &mut seen)
                .0
                .status,
            401
        );
    }

    #[test]
    fn line_hooks_are_checked_and_read() {
        use base64::Engine;
        let body = json!({"events": [
            {"type": "message", "webhookEventId": "e1", "replyToken": "r1",
             "source": {"type": "user", "userId": "U1"}, "message": {"id": "1", "type": "text", "text": "Hi"}},
            {"type": "message", "webhookEventId": "e2", "replyToken": "r2",
             "source": {"type": "group", "groupId": "C9", "userId": "U2"},
             "message": {"id": "2", "type": "text", "text": "@bot hi",
                 "mention": {"mentionees": [{"isSelf": true, "userId": "Ubot"}]}}},
            {"type": "follow", "webhookEventId": "e3", "source": {"type": "user", "userId": "U3"}},
            {"type": "message", "webhookEventId": "e4", "replyToken": "r4",
             "source": {"type": "user", "userId": "U1"}, "message": {"id": "4", "type": "image"}},
            {"type": "message", "webhookEventId": "e5", "replyToken": "r5",
             "source": {"type": "user", "userId": "U1"},
             "message": {"id": "5", "type": "file", "fileName": "report.pdf", "fileSize": 15}}
        ]})
        .to_string();
        let signature = base64::engine::general_purpose::STANDARD
            .encode(sion_http::hmac_sha256(b"chan", body.as_bytes()));
        let replies: Replies = Arc::new(Mutex::new(BTreeMap::new()));
        let mut seen = Seen::default();
        let signed = hook(
            "POST",
            "/",
            &[("x-line-signature", &signature)],
            body.as_bytes(),
        );
        let (answer, messages) = line_hook(&signed, "chan", &replies, &mut seen);
        assert_eq!(answer.status, 200);
        assert_eq!(messages.len(), 4);
        // A picture and a file, fetched by their messages' ids.
        assert_eq!(
            (messages[2].files.clone(), messages[3].files.clone()),
            (
                vec![Note {
                    id: "4".into(),
                    media_type: None,
                    name: None
                }],
                vec![Note {
                    id: "5".into(),
                    media_type: None,
                    name: Some("report.pdf".into())
                }]
            )
        );
        assert!(messages[0].direct);
        assert_eq!(
            (messages[1].chat.as_str(), messages[1].direct),
            ("C9", false)
        );
        assert_eq!(messages[1].mentions, ["self", "Ubot"]);
        assert_eq!(replies.lock().unwrap()["C9"].0, "r2");
        let forged = hook(
            "POST",
            "/",
            &[("x-line-signature", "AAAA")],
            body.as_bytes(),
        );
        assert_eq!(
            line_hook(&forged, "chan", &replies, &mut seen).0.status,
            401
        );
    }

    #[test]
    fn twilio_signatures_are_twilios() {
        // Twilio's documented example; Python's hmac gives the same.
        let form: Vec<(String, String)> = [
            ("CallSid", "CA1234567890ABCDE"),
            ("Caller", "+12349013030"),
            ("Digits", "1234"),
            ("From", "+12349013030"),
            ("To", "+18005551212"),
        ]
        .iter()
        .map(|(n, v)| (n.to_string(), v.to_string()))
        .collect();
        assert_eq!(
            twilio_signature(
                "12345",
                "https://mycompany.com/myapp.php?foo=1&bar=2",
                &form
            ),
            "0/KCTR6DLpKmkAf8muzZqo1nDgQ="
        );
        let body = encode_form(&[
            ("MessageSid", "SM1"),
            ("From", "+15550001111"),
            ("Body", "How much is left?"),
            ("NumMedia", "0"),
        ]);
        let url = "https://bot.example.org/sms";
        let signature = twilio_signature("tok", url, &decode_form(body.as_bytes()));
        let mut seen = Seen::default();
        let signed = hook(
            "POST",
            "/sms",
            &[("x-twilio-signature", &signature)],
            body.as_bytes(),
        );
        let (answer, messages) = sms_hook(&signed, "tok", url, &mut seen);
        assert_eq!((answer.status, answer.content_type), (200, "text/xml"));
        assert_eq!(messages[0].text, "How much is left?");
        assert_eq!(sms_hook(&signed, "other", url, &mut seen).0.status, 401);
        // An MMS: its audio a voice note, its picture a file.
        let body = encode_form(&[
            ("MessageSid", "MM1"),
            ("From", "+15550001111"),
            ("Body", ""),
            ("NumMedia", "2"),
            ("MediaContentType0", "image/jpeg"),
            (
                "MediaUrl0",
                "https://api.twilio.com/2010-04-01/Accounts/AC1/Messages/MM1/Media/ME0",
            ),
            ("MediaContentType1", "audio/amr"),
            (
                "MediaUrl1",
                "https://api.twilio.com/2010-04-01/Accounts/AC1/Messages/MM1/Media/ME1",
            ),
        ]);
        let signature = twilio_signature("tok", url, &decode_form(body.as_bytes()));
        let signed = hook(
            "POST",
            "/sms",
            &[("x-twilio-signature", &signature)],
            body.as_bytes(),
        );
        let (_, messages) = sms_hook(&signed, "tok", url, &mut seen);
        assert!(messages[0].voice.as_ref().unwrap().id.ends_with("/ME1"));
        assert_eq!(
            messages[0].files,
            [Note {
                id: "https://api.twilio.com/2010-04-01/Accounts/AC1/Messages/MM1/Media/ME0".into(),
                media_type: Some("image/jpeg".into()),
                name: None
            }]
        );
    }

    #[test]
    fn forms_are_read_and_written() {
        assert_eq!(
            decode_form(b"a=1+2&b=%2B15%20x&c"),
            [
                ("a".to_string(), "1 2".to_string()),
                ("b".to_string(), "+15 x".to_string()),
                ("c".to_string(), String::new())
            ]
        );
        assert_eq!(encode_form(&[("To", "+1 5")]), "To=%2B1%205");
        assert_eq!(
            host("https://Lookaside.FBSBX.com:443/x"),
            Some("lookaside.fbsbx.com".into())
        );
    }
}
