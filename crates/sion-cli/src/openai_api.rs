//! An OpenAI-compatible endpoint on the HTTP channel (`sion agent serve`),
//! so that apps made for OpenAI's chat API (Open WebUI, LibreChat,
//! editors, scripts) talk to profiles: `GET /v1/models` lists the
//! profiles a token reaches, each as a model, and `POST
//! /v1/chat/completions` is answered by the profile the request names as
//! its model, as Server-Sent Events when it asks to stream.
//!
//! Such apps send the whole conversation with every message. The endpoint
//! finds the session a conversation is in by what was said before its
//! last message (a digest of it), so that the conversation goes on in its
//! session, approvals included (`/approve`, `/decline <why>`, as on every
//! channel). A conversation it has not seen (edited, or begun elsewhere)
//! starts a new session, told what was said before. The tokens are the
//! channel's own: the machine's, or one of a profile's.
//!
//! A request that asks to stream (`"stream": true`) is answered as the
//! model writes: the text of each chunk is what the agent wrote since the
//! last, the run's answers a paragraph each, and the chunks end with the
//! reply the app keeps (`finish_reason` `stop`, then `data: [DONE]`).

use std::sync::Mutex;

use serde_json::{json, Value};

use crate::channels::{
    access, converse_with, event, refused_for_talk, set_state, state, Access, Answer, Growing,
    Pieces,
};
use crate::runtime::SharedLedger;

/// The longest request taken, in bytes: apps send the whole conversation,
/// its pictures inline.
pub const MAX_REQUEST: u64 = 32 << 20;

/// How much of an earlier conversation a new session is told, in bytes.
const MAX_CONTEXT: usize = 12 << 10;

/// What the endpoint answers: a status, a media type and a body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Answered {
    /// The status.
    pub status: u16,
    /// The media type.
    pub content_type: &'static str,
    /// The body.
    pub body: String,
}

fn error(status: u16, kind: &str, message: &str) -> Answered {
    Answered {
        status,
        content_type: "application/json",
        body: json!({"error": {"message": message, "type": kind, "code": Value::Null}}).to_string(),
    }
}

fn ok(body: Value) -> Answered {
    Answered {
        status: 200,
        content_type: "application/json",
        body: body.to_string(),
    }
}

/// What the endpoint answers: whole, or as events while the agent works.
#[derive(Debug)]
pub enum Served {
    /// Answered whole.
    Whole(Answered),
    /// Server-Sent Events, as they come.
    Streamed(Pieces),
}

/// Whether `path` is the endpoint's.
pub fn is_openai(path: &str) -> bool {
    matches!(path, "/v1/models" | "/v1/chat/completions")
}

/// Answers `method path` with `body`, for the bearer token `presented`
/// (the machine's being `admin`).
pub fn answer(
    ledger: SharedLedger,
    method: &str,
    path: &str,
    body: &str,
    presented: Option<&str>,
    admin: Option<&str>,
) -> Served {
    if presented.is_none() {
        return Served::Whole(error(
            401,
            "authentication_error",
            "a bearer token is needed",
        ));
    }
    match (method, path) {
        ("GET", "/v1/models") => Served::Whole(models(presented, admin)),
        ("POST", "/v1/chat/completions") => complete(ledger, body, presented, admin),
        _ => Served::Whole(error(
            405,
            "invalid_request_error",
            "not a method of this path",
        )),
    }
}

/// The profiles `presented` reaches, as models.
fn models(presented: Option<&str>, admin: Option<&str>) -> Answered {
    let profiles = match crate::profiles::all() {
        Ok(profiles) => profiles,
        Err(e) => return error(500, "server_error", &e.to_string()),
    };
    let data: Vec<Value> = profiles
        .keys()
        .filter(|name| access(name, presented, admin) != Access::None)
        .map(|name| json!({"id": name, "object": "model", "created": 0, "owned_by": "sion"}))
        .collect();
    if data.is_empty() {
        return error(401, "authentication_error", "this token reaches no profile");
    }
    ok(json!({"object": "list", "data": data}))
}

/// A message's text: a string, or the text parts of a list of parts.
fn text_of(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter(|part| part["type"] == "text")
            .filter_map(|part| part["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// The pictures of a message's parts, sent inline (`image_url` parts whose
/// URL is a `data:` one): those a model takes. A picture's own address is
/// not fetched.
fn pictures_of(content: &Value) -> Vec<sion_agent::Picture> {
    content
        .as_array()
        .into_iter()
        .flatten()
        .filter(|part| part["type"] == "image_url")
        .filter_map(|part| {
            part["image_url"]["url"]
                .as_str()
                .or_else(|| part["image_url"].as_str())
        })
        .filter_map(crate::files::picture_of_data_url)
        .collect()
}

/// What says which conversation `said` is: a digest of who said what.
fn digest(said: &[(String, String)]) -> String {
    let canonical = serde_json::to_string(said).unwrap_or_default();
    hex::encode(&sion_http::sha256(canonical.as_bytes())[..16])
}

/// What a new session is told: what was said before, then the message.
fn told(before: &[(String, String)], last: &str) -> String {
    let mut earlier = String::new();
    for (role, text) in before.iter().rev() {
        let line = format!("{role}: {text}\n");
        if earlier.len() + line.len() > MAX_CONTEXT {
            break;
        }
        earlier.insert_str(0, &line);
    }
    if earlier.is_empty() {
        last.to_string()
    } else {
        format!("Said before, in the app this comes from:\n{earlier}\nNow: {last}")
    }
}

fn complete(
    ledger: SharedLedger,
    body: &str,
    presented: Option<&str>,
    admin: Option<&str>,
) -> Served {
    let whole =
        |status: u16, kind: &str, message: &str| Served::Whole(error(status, kind, message));
    let Ok(request) = serde_json::from_str::<Value>(body) else {
        return whole(400, "invalid_request_error", "send the request as JSON");
    };
    let Some(model) = request["model"].as_str().map(str::to_string) else {
        return whole(400, "invalid_request_error", "name a profile as the model");
    };
    let allowed = access(&model, presented, admin);
    if allowed == Access::None {
        return whole(
            401,
            "authentication_error",
            "this token does not reach this profile",
        );
    }
    let said: Vec<(String, String)> = request["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|m| {
            (
                m["role"].as_str().unwrap_or("user").to_string(),
                text_of(&m["content"]),
            )
        })
        .collect();
    let Some(((role, last), before)) = said.split_last() else {
        return whole(400, "invalid_request_error", "send at least one message");
    };
    let pictures = request["messages"]
        .as_array()
        .and_then(|messages| messages.last())
        .map(|message| pictures_of(&message["content"]))
        .unwrap_or_default();
    if role.as_str() != "user" {
        return whole(
            400,
            "invalid_request_error",
            "the last message is the user's",
        );
    }
    if let Some(refused) = refused_for_talk(last).filter(|_| allowed != Access::Approve) {
        return whole(403, "permission_error", refused);
    }
    let profile = match crate::profiles::get(&model) {
        Ok(profile) => profile,
        Err(e) => return whole(404, "invalid_request_error", &e.to_string()),
    };
    // A conversation seen before goes on in its session; another starts
    // one, told what was said before.
    let key = format!("openai:{model}:{}", digest(before));
    let known = !before.is_empty() && matches!(state(&key), Ok(Some(_)));
    let (conversation, text) = if known {
        (key, last.clone())
    } else {
        (
            format!("openai:{model}:new:{}", crate::runtime::new_id()),
            told(before, last),
        )
    };
    // The next message's conversation is this one with this answer.
    let record = {
        let (said, model, conversation) = (said.clone(), model.clone(), conversation.clone());
        move |answer: &Answer| {
            if let Some(session) = &answer.session {
                let mut next = said.clone();
                next.push(("assistant".into(), answer.reply.clone()));
                let _ = set_state(&format!("openai:{model}:{}", digest(&next)), Some(session));
                let _ = set_state(&conversation, None);
            }
        }
    };
    let created = crate::schedules::now();
    if request["stream"] != true {
        let answer = match converse_with(
            ledger,
            &profile,
            &conversation,
            &text,
            &pictures,
            crate::runtime::Hooks::default(),
        ) {
            Ok(answer) => answer,
            Err(e) => return whole(400, "invalid_request_error", &e.to_string()),
        };
        record(&answer);
        let id = format!(
            "chatcmpl-{}",
            answer
                .session
                .clone()
                .unwrap_or_else(crate::runtime::new_id)
        );
        return Served::Whole(ok(json!({
            "id": id,
            "object": "chat.completion",
            "created": created,
            "model": model,
            "choices": [{"index": 0, "message": {"role": "assistant", "content": answer.reply},
                "finish_reason": "stop"}],
        })));
    }
    // The run's text as the model writes it, then the rest of the reply.
    // The chunks' id names the session, as a whole answer's does: the first
    // goes once the run says which session it is.
    let (tell, pieces) = std::sync::mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let id: Mutex<Option<String>> = Mutex::new(None);
        let chunk = |delta: Value, finish: Value| {
            let id = id.lock().ok().and_then(|id| id.clone()).unwrap_or_default();
            event(
                None,
                &json!({"id": id, "object": "chat.completion.chunk", "created": created,
                    "model": model, "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]}),
            )
        };
        let begin = |session: &str| {
            let first = id
                .lock()
                .map(|mut id| {
                    id.get_or_insert_with(|| format!("chatcmpl-{session}"))
                        .clone()
                })
                .is_ok();
            if first {
                let _ = tell.send(chunk(
                    json!({"role": "assistant", "content": ""}),
                    Value::Null,
                ));
            }
        };
        let begun = || id.lock().map(|id| id.is_some()).unwrap_or(true);
        let growing = Mutex::new(Growing::default());
        let watch = |seen: &sion_agent::Seen| {
            if let sion_agent::Seen::Began(session) = seen {
                if !begun() {
                    begin(session);
                }
                return;
            }
            let more = growing.lock().map(|mut g| g.take(seen)).ok().flatten();
            if let Some(more) = more {
                let _ = tell.send(chunk(json!({"content": more}), Value::Null));
            }
        };
        let answered = converse_with(
            ledger,
            &profile,
            &conversation,
            &text,
            &pictures,
            crate::runtime::Hooks::watched(&watch),
        );
        // A command is answered without a run.
        if !begun() {
            let session = answered
                .as_ref()
                .ok()
                .and_then(|answer| answer.session.clone())
                .unwrap_or_else(crate::runtime::new_id);
            begin(&session);
        }
        match answered {
            Ok(answer) => {
                let rest = growing
                    .lock()
                    .map(|g| g.rest(&answer.reply))
                    .unwrap_or_default();
                if !rest.is_empty() {
                    let _ = tell.send(chunk(json!({"content": rest}), Value::Null));
                }
                record(&answer);
                let _ = tell.send(chunk(json!({}), json!("stop")));
            }
            Err(e) => {
                let _ = tell.send(event(
                    None,
                    &json!({"error": {"message": e.to_string(), "type": "invalid_request_error", "code": Value::Null}}),
                ));
            }
        }
        let _ = tell.send(b"data: [DONE]\n\n".to_vec());
    });
    Served::Streamed(pieces)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversations_are_told_apart_by_what_was_said() {
        let a = vec![("user".to_string(), "Hi".to_string())];
        let mut b = a.clone();
        b.push(("assistant".into(), "Hello.".into()));
        assert_ne!(digest(&a), digest(&b));
        assert_eq!(digest(&a), digest(&a.clone()));
        assert_eq!(text_of(&json!("plain")), "plain");
        assert_eq!(
            text_of(
                &json!([{"type": "text", "text": "a"}, {"type": "image_url"}, {"type": "text", "text": "b"}])
            ),
            "a\nb"
        );
        assert_eq!(told(&[], "Now this"), "Now this");
        let before = told(&b, "And then?");
        assert!(
            before.starts_with("Said before") && before.ends_with("Now: And then?"),
            "{before}"
        );
        assert!(is_openai("/v1/chat/completions") && !is_openai("/v1/profiles/x/messages"));
    }

    #[test]
    fn pictures_sent_inline_are_shown_and_addresses_are_not_fetched() {
        let content = json!([
            {"type": "text", "text": "What is this?"},
            {"type": "image_url", "image_url": {"url": "data:image/jpeg;base64,iVBORw0KGgoAAA=="}},
            {"type": "image_url", "image_url": {"url": "https://example.org/a.png"}},
            {"type": "image_url", "image_url": {"url": "data:text/plain;base64,aGk="}}
        ]);
        let pictures = pictures_of(&content);
        assert_eq!(pictures.len(), 1);
        // What the bytes are, whatever the URL says.
        assert_eq!(pictures[0].media_type, "image/png");
        assert!(pictures_of(&json!("plain")).is_empty());
        assert_eq!(text_of(&content), "What is this?");
    }
}
