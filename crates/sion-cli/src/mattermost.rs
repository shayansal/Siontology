//! Mattermost, through its API (v4): events over its WebSocket, posts
//! through `/posts`, with a bot's access token (`MATTERMOST_TOKEN`) on the
//! server at `MATTERMOST_URL`. Direct messages and channels alike arrive;
//! a voice message is an audio file attached to a post, and a spoken reply
//! is posted with the audio attached.

use std::sync::OnceLock;
use std::time::Duration;

use serde_json::{json, Value};
use sion_http::ws::WebSocket;
use sion_http::HttpError;

use crate::live::{again, keep, text_of, Fault, Link, Live, Next, MAX_EVENT};
use crate::messengers::{Ack, Incoming, Messenger, Note, LIVE_ONCE_WAIT};
use crate::showing::{with_status, Draft, Drafting, Typist};
use crate::voice::{Audio, Form, MAX_HEARD};

/// A Mattermost bot.
pub struct Mattermost {
    client: sion_http::Client,
    /// The server, `https://chat.example.org`.
    url: String,
    token: String,
    live: OnceLock<Live>,
    me: OnceLock<Vec<String>>,
}

impl std::fmt::Debug for Mattermost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Mattermost({})", self.url)
    }
}

fn client() -> Result<sion_http::Client, String> {
    sion_http::Client::new(sion_http::Options {
        timeout: Duration::from_secs(60),
        // The operator names the server: one on their own network too.
        allow_private: true,
        ..sion_http::Options::default()
    })
    .map_err(|e| e.to_string())
}

/// Calls the API: `method path`, with `body` as JSON if any.
fn call(
    client: &sion_http::Client,
    url: &str,
    token: &str,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> Result<Value, String> {
    let auth = format!("Bearer {token}");
    let bytes = body.map(|b| b.to_string().into_bytes());
    let mut headers = vec![("Authorization", auth.as_str())];
    if bytes.is_some() {
        headers.push(("Content-Type", "application/json"));
    }
    let fetched = client
        .request(
            method,
            &format!("{url}/api/v4{path}"),
            &headers,
            bytes.as_deref(),
            4 << 20,
        )
        .map_err(|e| e.to_string().replace(token, "<token>"))?;
    serde_json::from_slice(&fetched.body)
        .map_err(|e| format!("Mattermost answered with something other than JSON: {e}"))
}

/// A message from a `posted` event, unless it is the bot's own, a
/// system message, or says nothing; `voice` is the audio file it carries,
/// if one does, and `files` the others.
pub fn mattermost_message(
    data: &Value,
    me: Option<&str>,
    voice: Option<Note>,
    files: Vec<Note>,
) -> Option<Incoming> {
    let post: Value = serde_json::from_str(data["post"].as_str()?).ok()?;
    let from = post["user_id"].as_str()?;
    if Some(from) == me || post["type"].as_str().is_some_and(|t| !t.is_empty()) {
        return None;
    }
    let mut text = post["message"].as_str().unwrap_or_default().to_string();
    if let Some(name) = data["me_name"].as_str() {
        text = text.replace(&format!("@{name}"), "").trim().to_string();
    }
    if text.is_empty() && voice.is_none() && files.is_empty() {
        return None;
    }
    let mentions: Vec<String> = data["mentions"]
        .as_str()
        .and_then(|m| serde_json::from_str::<Vec<String>>(m).ok())
        .unwrap_or_default();
    Some(Incoming {
        id: post["id"].as_str().unwrap_or_default().to_string(),
        thread: post["root_id"].as_str().unwrap_or_default().to_string(),
        chat: post["channel_id"].as_str()?.to_string(),
        from: from.to_string(),
        text,
        voice,
        direct: data["channel_type"] == "D",
        mentions,
        files,
    })
}

/// Mattermost's typing indicator (`POST /users/me/typing`), in the
/// message's thread if it is in one.
struct Typing {
    client: sion_http::Client,
    url: String,
    token: String,
    channel: String,
    thread: String,
}

impl Typist for Typing {
    fn show(&mut self) -> Result<Duration, String> {
        let mut body = json!({"channel_id": self.channel});
        if !self.thread.is_empty() {
            body["parent_id"] = json!(self.thread);
        }
        call(
            &self.client,
            &self.url,
            &self.token,
            "POST",
            "/users/me/typing",
            Some(&body),
        )?;
        // Clients show it for a few seconds.
        Ok(Duration::from_secs(5))
    }
}

/// A Mattermost reply as a post patched as it grows.
struct Edit {
    client: sion_http::Client,
    url: String,
    token: String,
    channel: String,
    thread: String,
    post: Option<String>,
    shown: String,
}

impl Edit {
    fn put(&mut self, text: &str) -> Result<(), String> {
        if self.post.is_some() && self.shown == text {
            return Ok(());
        }
        self.shown = text.to_string();
        let message: String = text.chars().take(16_000).collect();
        match &self.post {
            Some(id) => call(
                &self.client,
                &self.url,
                &self.token,
                "PUT",
                &format!("/posts/{}/patch", crate::messengers::segment(id)),
                Some(&json!({"message": message})),
            )
            .map(|_| ()),
            None => {
                let mut post = json!({"channel_id": self.channel, "message": message});
                if !self.thread.is_empty() {
                    post["root_id"] = json!(self.thread);
                }
                let made = call(
                    &self.client,
                    &self.url,
                    &self.token,
                    "POST",
                    "/posts",
                    Some(&post),
                )?;
                self.post = made["id"].as_str().map(str::to_string);
                Ok(())
            }
        }
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

impl Mattermost {
    /// The bot whose token is in `MATTERMOST_TOKEN`, on the server at
    /// `MATTERMOST_URL`.
    pub fn from_env() -> Result<Mattermost, String> {
        let env = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
        Ok(Mattermost {
            client: client()?,
            url: env("MATTERMOST_URL")
                .ok_or("set MATTERMOST_URL (https://…)")?
                .trim_end_matches('/')
                .to_string(),
            token: env("MATTERMOST_TOKEN").ok_or("set MATTERMOST_TOKEN (a bot's access token)")?,
            live: OnceLock::new(),
            me: OnceLock::new(),
        })
    }

    fn call(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Value, String> {
        call(&self.client, &self.url, &self.token, method, path, body)
    }

    fn live(&self) -> &Live {
        self.live.get_or_init(|| {
            let (client, url, token) = (self.client.clone(), self.url.clone(), self.token.clone());
            Live::start("mattermost", move |link| {
                keep("mattermost", &link, |link, _| {
                    session(&client, &url, &token, link)
                });
            })
        })
    }
}

/// One WebSocket connection: who the bot is, then its events.
fn session(client: &sion_http::Client, url: &str, token: &str, link: &Link) -> Result<Next, Fault> {
    let refused = |e: String| {
        if e.contains("HTTP 401") {
            Fault::Fatal("Mattermost refused the token".into())
        } else {
            again(e)
        }
    };
    let me = call(client, url, token, "GET", "/users/me", None).map_err(refused)?;
    let (id, name) = (
        me["id"].as_str().unwrap_or_default().to_string(),
        me["username"].as_str().unwrap_or_default().to_string(),
    );
    link.set_me(vec![id.clone(), format!("@{name}")]);
    let socket_url = format!(
        "{}/api/v4/websocket",
        url.replacen("https://", "wss://", 1)
            .replacen("http://", "ws://", 1)
    );
    let auth = format!("Bearer {token}");
    let mut socket =
        WebSocket::connect(client, &socket_url, &[("Authorization", &auth)], MAX_EVENT)
            .map_err(again)?;
    loop {
        if link.stopped() {
            socket.close(1000, "stopped");
            return Ok(Next::Stop);
        }
        let event = match text_of(socket.receive(Duration::from_secs(1))) {
            Ok(Some(event)) => event,
            Ok(None) => continue,
            Err(HttpError::Closed { .. }) => return Ok(Next::Resume),
            Err(e) => return Err(again(e)),
        };
        if event["event"] != "posted" {
            continue;
        }
        let mut data = event["data"].clone();
        data["me_name"] = json!(name);
        // The first audio file attached is a voice message; the others are
        // files.
        let post: Value = data["post"]
            .as_str()
            .and_then(|p| serde_json::from_str(p).ok())
            .unwrap_or_default();
        let mut voice = None;
        let mut files = Vec::new();
        for file in post["file_ids"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|file| file.as_str())
            .filter(|file| !file.is_empty() && file.bytes().all(|b| b.is_ascii_alphanumeric()))
        {
            let Ok(info) = call(
                client,
                url,
                token,
                "GET",
                &format!("/files/{file}/info"),
                None,
            ) else {
                continue;
            };
            let media_type = info["mime_type"].as_str().map(str::to_string);
            if voice.is_none()
                && media_type
                    .as_deref()
                    .is_some_and(|t| t.starts_with("audio/"))
            {
                voice = Some(Note {
                    id: file.to_string(),
                    media_type,
                    name: None,
                });
            } else {
                files.push(Note {
                    id: file.to_string(),
                    media_type,
                    name: info["name"].as_str().map(str::to_string),
                });
            }
        }
        if let Some(message) = mattermost_message(&data, Some(&id), voice, files) {
            if !link.pass(message) {
                socket.close(1000, "stopped");
                return Ok(Next::Stop);
            }
        }
    }
}

impl Messenger for Mattermost {
    fn acknowledge(&self, message: &Incoming, ack: Ack) -> Result<(), String> {
        let me = self.me()?.into_iter().next().unwrap_or_default();
        let react = |emoji: &str| {
            self.call(
                "POST",
                "/reactions",
                Some(&json!({"user_id": me, "post_id": message.id, "emoji_name": emoji})),
            )
            .map(|_| ())
        };
        let unreact = |emoji: &str| {
            self.call(
                "DELETE",
                &format!("/users/{me}/posts/{}/reactions/{emoji}", message.id),
                None,
            )
            .map(|_| ())
        };
        match ack {
            Ack::Seen => react("eyes"),
            Ack::Done => unreact("eyes"),
            Ack::Failed => {
                unreact("eyes")?;
                react("warning")
            }
        }
    }

    fn name(&self) -> &'static str {
        "mattermost"
    }

    fn chat_word(&self) -> &'static str {
        "channel"
    }

    fn receive(&self, cursor: &str, wait: u64) -> Result<(Vec<Incoming>, String), String> {
        Ok((self.live().receive(wait)?, cursor.to_string()))
    }

    fn send(&self, chat: &str, text: &str) -> Result<(), String> {
        let message: String = text.chars().take(16_000).collect();
        self.call(
            "POST",
            "/posts",
            Some(&json!({"channel_id": chat, "message": message})),
        )
        .map(|_| ())
    }

    fn reply(&self, message: &Incoming, text: &str) -> Result<(), String> {
        let mut post = json!({"channel_id": message.chat, "message": text.chars().take(16_000).collect::<String>()});
        if !message.thread.is_empty() {
            post["root_id"] = json!(message.thread);
        }
        self.call("POST", "/posts", Some(&post)).map(|_| ())
    }

    fn room(&self) -> usize {
        16_000
    }

    fn typist(&self, message: &Incoming, _aloud: bool) -> Option<Box<dyn Typist>> {
        Some(Box::new(Typing {
            client: self.client.clone(),
            url: self.url.clone(),
            token: self.token.clone(),
            channel: message.chat.clone(),
            thread: message.thread.clone(),
        }))
    }

    fn drafting(&self, message: &Incoming) -> Option<Drafting> {
        let (client, url, token) = (self.client.clone(), self.url.clone(), self.token.clone());
        let (channel, thread) = (message.chat.clone(), message.thread.clone());
        Some(Drafting {
            open: Box::new(move || {
                Box::new(Edit {
                    client: client.clone(),
                    url: url.clone(),
                    token: token.clone(),
                    channel: channel.clone(),
                    thread: thread.clone(),
                    post: None,
                    shown: String::new(),
                })
            }),
            every: Duration::from_secs(1),
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
        if let Some(me) = self.me.get() {
            return Ok(me.clone());
        }
        let me = self.call("GET", "/users/me", None)?;
        let names = vec![
            me["id"].as_str().unwrap_or_default().to_string(),
            format!("@{}", me["username"].as_str().unwrap_or_default()),
        ];
        Ok(self.me.get_or_init(|| names).clone())
    }

    fn fetch_voice(&self, note: &Note) -> Result<Audio, String> {
        if note.id.is_empty() || !note.id.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return Err("not a file id".into());
        }
        let auth = format!("Bearer {}", self.token);
        let fetched = self
            .client
            .request(
                "GET",
                &format!("{}/api/v4/files/{}", self.url, note.id),
                &[("Authorization", &auth)],
                None,
                MAX_HEARD as u64,
            )
            .map_err(|e| e.to_string().replace(&self.token, "<token>"))?;
        let media_type = note
            .media_type
            .clone()
            .unwrap_or_else(|| Audio::sniff(&fetched.body).to_string());
        Ok(Audio::new(&media_type, fetched.body))
    }

    fn send_voice(&self, chat: &str, audio: &Audio, caption: &str) -> Result<bool, String> {
        self.attach(
            chat,
            "",
            caption,
            &audio.name,
            &audio.media_type,
            &audio.bytes,
        )
        .map(|()| true)
    }

    fn send_file(
        &self,
        message: &Incoming,
        file: &crate::files::File,
        caption: &str,
    ) -> Result<bool, String> {
        self.attach(
            &message.chat,
            &message.thread,
            caption,
            &file.name,
            &file.media_type,
            &file.bytes,
        )
        .map(|()| true)
    }
}

impl Mattermost {
    /// Posts `caption` to `chat` (in `thread`, if not empty) with a file
    /// attached: the file uploaded first, then the post that has it.
    fn attach(
        &self,
        chat: &str,
        thread: &str,
        caption: &str,
        name: &str,
        media_type: &str,
        bytes: &[u8],
    ) -> Result<(), String> {
        let (content_type, body) = Form::new()
            .text("channel_id", chat)
            .bytes("files", name, media_type, bytes)
            .finish();
        let auth = format!("Bearer {}", self.token);
        let uploaded = self
            .client
            .request(
                "POST",
                &format!("{}/api/v4/files", self.url),
                &[("Authorization", &auth), ("Content-Type", &content_type)],
                Some(&body),
                1 << 20,
            )
            .map_err(|e| e.to_string().replace(&self.token, "<token>"))?;
        let uploaded: Value = serde_json::from_slice(&uploaded.body)
            .map_err(|e| format!("Mattermost answered with something other than JSON: {e}"))?;
        let id = uploaded["file_infos"][0]["id"]
            .as_str()
            .ok_or("Mattermost gave no file id")?;
        let message: String = caption.chars().take(16_000).collect();
        let mut post = json!({"channel_id": chat, "message": message, "file_ids": [id]});
        if !thread.is_empty() {
            post["root_id"] = json!(thread);
        }
        self.call("POST", "/posts", Some(&post)).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn posts_are_read_from_events() {
        let post = json!({"id": "p1", "user_id": "u9", "channel_id": "c1", "message": "@lead how much is left?", "type": ""});
        let data = json!({"post": post.to_string(), "channel_type": "O",
            "mentions": "[\"b7\"]", "me_name": "lead"});
        let message = mattermost_message(&data, Some("b7"), None, Vec::new()).unwrap();
        assert_eq!(
            (message.chat.as_str(), message.text.as_str(), message.direct),
            ("c1", "how much is left?", false)
        );
        assert_eq!(message.mentions, ["b7"]);
        let own = json!({"post": json!({"user_id": "b7", "channel_id": "c1", "message": "mine"}).to_string()});
        assert!(mattermost_message(&own, Some("b7"), None, Vec::new()).is_none());
        let system = json!({"post": json!({"user_id": "u9", "channel_id": "c1", "message": "joined", "type": "system_join_channel"}).to_string()});
        assert!(mattermost_message(&system, Some("b7"), None, Vec::new()).is_none());
        let direct = json!({"post": json!({"user_id": "u9", "channel_id": "d1", "message": "hi"}).to_string(), "channel_type": "D"});
        assert!(
            mattermost_message(&direct, Some("b7"), None, Vec::new())
                .unwrap()
                .direct
        );
    }
}
