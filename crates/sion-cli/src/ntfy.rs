//! ntfy, the publish-and-subscribe notification service: each topic in
//! `--allow` is a chat, polled for what was published to it since the
//! bridge's place (`GET /<topic>/json?poll=1`), and answered by publishing
//! to it (`POST /<topic>`), tagged so that the bridge does not take its own
//! answers for messages. `NTFY_URL` names the server (`https://ntfy.sh` by
//! default); `NTFY_TOKEN`, or `NTFY_USER` and `NTFY_PASSWORD`, sign in to
//! one that asks. A file published with a message (an attachment) is taken;
//! a file the agent shares is published as one, where the server takes
//! attachments.
//!
//! ntfy names no writer: whoever may publish to a topic speaks for you there
//! (`agent:ntfy-topics`). Use a topic of your own, on a server that asks
//! who publishes, or a name nobody can guess.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::Value;

use crate::messengers::{Incoming, Messenger, Note};
use crate::voice::Audio;

/// The server unless `NTFY_URL` names another.
pub const NTFY: &str = "https://ntfy.sh";

/// The tag the bridge's answers carry.
pub const OWN_TAG: &str = "sion";

/// The most bytes ntfy takes as a message's text (more becomes a file).
const NTFY_TEXT_BYTES: usize = 4_096;

/// An ntfy server and its topics.
pub struct Ntfy {
    client: sion_http::Client,
    base: String,
    auth: Option<String>,
    topics: Vec<String>,
}

impl std::fmt::Debug for Ntfy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Ntfy({}, {:?})", self.base, self.topics)
    }
}

/// Whether `topic` is a topic's name as ntfy takes them.
fn plain_topic(topic: &str) -> bool {
    !topic.is_empty()
        && topic.len() <= 64
        && topic
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
}

/// The messages of a poll's answer (one JSON object a line) for `topic`,
/// those the bridge published left out, and the id of the last.
pub fn ntfy_messages(body: &str, topic: &str) -> (Vec<Incoming>, Option<String>) {
    let mut out = Vec::new();
    let mut last = None;
    for line in body.lines().filter(|l| !l.trim().is_empty()) {
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if event["event"] != "message" {
            continue;
        }
        let Some(id) = event["id"].as_str() else {
            continue;
        };
        last = Some(id.to_string());
        let own = event["tags"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|tag| tag == OWN_TAG);
        if own {
            continue;
        }
        let text = event["message"]
            .as_str()
            .unwrap_or_default()
            .trim()
            .to_string();
        let files: Vec<Note> = event["attachment"]["url"]
            .as_str()
            .map(|url| Note {
                id: url.to_string(),
                media_type: event["attachment"]["type"].as_str().map(str::to_string),
                name: event["attachment"]["name"].as_str().map(str::to_string),
            })
            .into_iter()
            .collect();
        if text.is_empty() && files.is_empty() {
            continue;
        }
        out.push(Incoming {
            id: id.to_string(),
            thread: String::new(),
            chat: topic.to_string(),
            from: String::new(),
            text,
            voice: None,
            direct: true,
            mentions: Vec::new(),
            files,
        });
    }
    (out, last)
}

impl Ntfy {
    /// The server `NTFY_URL` names (default [`NTFY`]), for `topics`.
    pub fn from_env(topics: &[String]) -> Result<Ntfy, String> {
        let env = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
        if topics.is_empty() {
            return Err("name the topics to answer with --allow".into());
        }
        if let Some(bad) = topics.iter().find(|t| !plain_topic(t)) {
            return Err(format!(
                "{bad} is not an ntfy topic (letters, digits, - and _)"
            ));
        }
        let auth = match (env("NTFY_TOKEN"), env("NTFY_USER"), env("NTFY_PASSWORD")) {
            (Some(token), _, _) => Some(format!("Bearer {token}")),
            (None, Some(user), Some(password)) => Some(format!(
                "Basic {}",
                sion_http::base64(format!("{user}:{password}").as_bytes())
            )),
            _ => None,
        };
        Ok(Ntfy {
            client: sion_http::Client::new(sion_http::Options {
                timeout: Duration::from_secs(60),
                // The person names the server.
                allow_private: true,
                ..sion_http::Options::default()
            })
            .map_err(|e| e.to_string())?,
            base: env("NTFY_URL")
                .unwrap_or_else(|| NTFY.into())
                .trim_end_matches('/')
                .to_string(),
            auth,
            topics: topics.to_vec(),
        })
    }

    fn headers<'a>(&'a self, more: &[(&'a str, &'a str)]) -> Vec<(&'a str, &'a str)> {
        let mut headers: Vec<(&str, &str)> = self
            .auth
            .as_deref()
            .map(|auth| ("Authorization", auth))
            .into_iter()
            .collect();
        headers.extend_from_slice(more);
        headers
    }

    fn hide(&self, error: String) -> String {
        match &self.auth {
            Some(auth) => error.replace(auth, "<credentials>"),
            None => error,
        }
    }
}

/// `text` cut to what ntfy takes as a message, on a character's edge.
fn fit(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if out.len() + c.len_utf8() > NTFY_TEXT_BYTES {
            break;
        }
        out.push(c);
    }
    out
}

impl Messenger for Ntfy {
    fn name(&self) -> &'static str {
        "ntfy"
    }

    fn chat_word(&self) -> &'static str {
        "topic"
    }

    fn receive(&self, cursor: &str, _wait: u64) -> Result<(Vec<Incoming>, String), String> {
        let mut places: BTreeMap<String, String> = serde_json::from_str(cursor).unwrap_or_default();
        let mut out = Vec::new();
        for topic in &self.topics {
            let Some(since) = places.get(topic).cloned() else {
                // The first time, from now on.
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                places.insert(topic.clone(), now.to_string());
                continue;
            };
            let url = format!("{}/{topic}/json?poll=1&since={since}", self.base);
            let fetched = self
                .client
                .request("GET", &url, &self.headers(&[]), None, 4 << 20)
                .map_err(|e| self.hide(e.to_string()))?;
            let (messages, last) = ntfy_messages(&String::from_utf8_lossy(&fetched.body), topic);
            out.extend(messages);
            if let Some(last) = last {
                places.insert(topic.clone(), last);
            }
        }
        Ok((out, serde_json::to_string(&places).unwrap_or_default()))
    }

    fn send(&self, chat: &str, text: &str) -> Result<(), String> {
        if !plain_topic(chat) {
            return Err(format!("{chat} is not an ntfy topic"));
        }
        let text = fit(text);
        self.client
            .request(
                "POST",
                &format!("{}/{chat}", self.base),
                &self.headers(&[("Tags", "robot,sion"), ("Markdown", "yes")]),
                Some(if text.is_empty() {
                    b"(nothing)"
                } else {
                    text.as_bytes()
                }),
                1 << 16,
            )
            .map(|_| ())
            .map_err(|e| self.hide(e.to_string()))
    }

    /// ntfy counts bytes: a part of 1,365 characters fits whatever script
    /// it is in.
    fn room(&self) -> usize {
        NTFY_TEXT_BYTES / 3
    }

    fn long_polls(&self) -> bool {
        false
    }

    fn sees_strangers(&self) -> bool {
        false
    }

    /// An attachment, from where ntfy keeps it (the server's credentials
    /// go only to the server itself).
    fn fetch_voice(&self, note: &Note) -> Result<Audio, String> {
        let own = note.id.starts_with(&format!("{}/", self.base));
        let headers = if own { self.headers(&[]) } else { Vec::new() };
        let fetched = self
            .client
            .request("GET", &note.id, &headers, None, crate::files::MAX_RECEIVED)
            .map_err(|e| self.hide(e.to_string()))?;
        let media_type = note
            .media_type
            .clone()
            .or(fetched.content_type.clone())
            .unwrap_or_else(|| Audio::sniff(&fetched.body).to_string());
        Ok(Audio::new(&media_type, fetched.body))
    }

    /// Published as an attachment (`PUT /<topic>`), its name given.
    fn send_file(
        &self,
        message: &Incoming,
        file: &crate::files::File,
        caption: &str,
    ) -> Result<bool, String> {
        if !plain_topic(&message.chat) {
            return Err(format!("{} is not an ntfy topic", message.chat));
        }
        let name: String = file
            .name
            .chars()
            .map(|c| {
                if c.is_ascii_graphic() || c == ' ' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let caption: String = caption
            .chars()
            .filter(|c| !c.is_control())
            .take(500)
            .collect();
        let mut more = vec![("Tags", "robot,sion"), ("Filename", name.as_str())];
        if !caption.is_empty() {
            more.push(("Message", caption.as_str()));
        }
        self.client
            .request(
                "PUT",
                &format!("{}/{}", self.base, message.chat),
                &self.headers(&more),
                Some(&file.bytes),
                1 << 16,
            )
            .map(|_| true)
            .map_err(|e| self.hide(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_poll_gives_what_others_published_and_where_it_ends() {
        let body = [
            r#"{"id":"a1","time":1,"event":"open","topic":"ops"}"#,
            r#"{"id":"a2","time":2,"event":"message","topic":"ops","message":"How much is left?"}"#,
            r#"{"id":"a3","time":3,"event":"message","topic":"ops","message":"85 USD.","tags":["robot","sion"]}"#,
            r#"{"id":"a4","time":4,"event":"message","topic":"ops","message":"","attachment":{"name":"r.pdf","type":"application/pdf","size":9,"url":"https://ntfy.sh/file/a4.pdf"}}"#,
            "not json",
        ]
        .join("\n");
        let (messages, last) = ntfy_messages(&body, "ops");
        assert_eq!(last.as_deref(), Some("a4"));
        assert_eq!(messages.len(), 2, "its own answer left out");
        assert_eq!(
            (messages[0].chat.as_str(), messages[0].text.as_str()),
            ("ops", "How much is left?")
        );
        assert_eq!(messages[1].files[0].name.as_deref(), Some("r.pdf"));
        assert!(plain_topic("ops_2-a") && !plain_topic("a/b") && !plain_topic(""));
        assert_eq!(fit(&"é".repeat(3_000)).len(), 4_096);
    }
}
