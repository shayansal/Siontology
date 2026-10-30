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

use serde_json::{json, Value};

use crate::channels::{access, converse, is_decision, set_state, state, Access};
use crate::runtime::SharedLedger;

/// The longest request taken, in bytes: apps send the whole conversation.
pub const MAX_REQUEST: u64 = 1 << 20;

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
) -> Answered {
    if presented.is_none() {
        return error(401, "authentication_error", "a bearer token is needed");
    }
    match (method, path) {
        ("GET", "/v1/models") => models(presented, admin),
        ("POST", "/v1/chat/completions") => complete(ledger, body, presented, admin),
        _ => error(405, "invalid_request_error", "not a method of this path"),
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
) -> Answered {
    let Ok(request) = serde_json::from_str::<Value>(body) else {
        return error(400, "invalid_request_error", "send the request as JSON");
    };
    let Some(model) = request["model"].as_str() else {
        return error(400, "invalid_request_error", "name a profile as the model");
    };
    let allowed = access(model, presented, admin);
    if allowed == Access::None {
        return error(
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
        return error(400, "invalid_request_error", "send at least one message");
    };
    if role.as_str() != "user" {
        return error(
            400,
            "invalid_request_error",
            "the last message is the user's",
        );
    }
    if is_decision(last) && allowed != Access::Approve {
        return error(
            403,
            "permission_error",
            "this token may talk to the profile, not approve or decline",
        );
    }
    let profile = match crate::profiles::get(model) {
        Ok(profile) => profile,
        Err(e) => return error(404, "invalid_request_error", &e.to_string()),
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
    let answer = match converse(ledger, &profile, &conversation, &text) {
        Ok(answer) => answer,
        Err(e) => return error(400, "invalid_request_error", &e.to_string()),
    };
    // The next message's conversation is this one with this answer.
    if let Some(session) = &answer.session {
        let mut next = said.clone();
        next.push(("assistant".into(), answer.reply.clone()));
        let _ = set_state(&format!("openai:{model}:{}", digest(&next)), Some(session));
        let _ = set_state(&conversation, None);
    }
    let id = format!(
        "chatcmpl-{}",
        answer
            .session
            .clone()
            .unwrap_or_else(crate::runtime::new_id)
    );
    let created = crate::schedules::now();
    if request["stream"] != true {
        return ok(json!({
            "id": id,
            "object": "chat.completion",
            "created": created,
            "model": model,
            "choices": [{"index": 0, "message": {"role": "assistant", "content": answer.reply},
                "finish_reason": "stop"}],
        }));
    }
    // The answer comes whole from the session: streamed as one piece.
    let chunk = |delta: Value, finish: Value| {
        let value = json!({"id": id, "object": "chat.completion.chunk", "created": created,
            "model": model, "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]});
        format!("data: {value}\n\n")
    };
    let body = [
        chunk(json!({"role": "assistant", "content": ""}), Value::Null),
        chunk(json!({"content": answer.reply}), Value::Null),
        chunk(json!({}), json!("stop")),
        "data: [DONE]\n\n".to_string(),
    ]
    .concat();
    Answered {
        status: 200,
        content_type: "text/event-stream",
        body,
    }
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
}
