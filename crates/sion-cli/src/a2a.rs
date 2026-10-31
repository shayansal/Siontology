//! Agent2Agent (A2A) on the HTTP channel, so that other agents reach a
//! profile as an A2A agent: its card (`GET /v1/profiles/<name>/agent-card.json`,
//! and at `/.well-known/agent-card.json` for the profile `sion agent serve
//! --a2a` names), and JSON-RPC at `POST /v1/profiles/<name>/a2a` with one
//! of the channel's tokens.
//!
//! Both of A2A's wire versions are spoken, as the request names them (the
//! `A2A-Version` header, or the method's name): 0.3 (`message/send`,
//! `message/stream`, `tasks/get`, `tasks/cancel`; roles `user` and `agent`,
//! states `completed`, parts with a `kind`), and 1.0 (`SendMessage`,
//! `SendStreamingMessage`, `GetTask`, `CancelTask`; `ROLE_USER`,
//! `TASK_STATE_COMPLETED`, parts without one, a task wrapped as `task`).
//! The card gives both: 0.3's `url` and `protocolVersion`, and 1.0's
//! `supportedInterfaces`.
//!
//! A message's `contextId` is a conversation, each a session as on every
//! channel; a message without one starts a conversation, whose id the task
//! gives back. Each message is a task, answered when the agent is done:
//! `completed`, or `input-required` while the session waits for an
//! approval, which a token made to decide gives in the same context
//! (`/approve`, `/decline <why>`). Tasks are remembered while the server
//! runs (the last thousand). Push notifications are not offered.

use std::collections::VecDeque;
use std::sync::Mutex;

use serde_json::{json, Value};

use crate::channels::{
    access, converse_with, event, refused_for_talk, Access, Answer, Growing, Pieces,
};
use crate::openai_api::Served;
use crate::profiles::Profile;
use crate::runtime::SharedLedger;

/// The most tasks remembered.
const TASKS_KEPT: usize = 1_000;

/// The wire version of a request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Version {
    /// 0.3: `message/send`, lower-case enums, parts with a `kind`.
    V03,
    /// 1.0: `SendMessage`, `ROLE_…` and `TASK_STATE_…`, parts without.
    V10,
}

impl Version {
    /// The version a request speaks: its `A2A-Version` header, else its
    /// method's name.
    pub fn of(header: Option<&str>, method: &str) -> Version {
        match header.map(str::trim) {
            Some(v) if v.starts_with('1') => Version::V10,
            Some(v) if v.starts_with("0.") => Version::V03,
            _ if method.contains('/') => Version::V03,
            _ => Version::V10,
        }
    }

    fn state(self, state: &str) -> Value {
        match self {
            Version::V03 => json!(state),
            Version::V10 => json!(format!(
                "TASK_STATE_{}",
                state.replace('-', "_").to_ascii_uppercase()
            )),
        }
    }

    fn role(self, agent: bool) -> Value {
        match (self, agent) {
            (Version::V03, true) => json!("agent"),
            (Version::V03, false) => json!("user"),
            (Version::V10, true) => json!("ROLE_AGENT"),
            (Version::V10, false) => json!("ROLE_USER"),
        }
    }

    fn text_part(self, text: &str) -> Value {
        match self {
            Version::V03 => json!({"kind": "text", "text": text}),
            Version::V10 => json!({"text": text, "mediaType": "text/plain"}),
        }
    }
}

/// A task: the message it answers, and what became of it.
#[derive(Clone, Debug)]
struct Task {
    id: String,
    context: String,
    profile: String,
    /// `submitted`, `working`, `completed`, `input-required`, `failed` or
    /// `canceled`.
    state: String,
    reply: String,
}

impl Task {
    fn json(&self, version: Version) -> Value {
        let mut status = json!({"state": version.state(&self.state)});
        let mut task = json!({"id": self.id, "contextId": self.context, "status": status.clone()});
        if !self.reply.is_empty() {
            let parts = json!([version.text_part(&self.reply)]);
            if self.state == "input-required" {
                // What the session waits for is asked of the client.
                status["message"] = json!({"role": version.role(true), "parts": parts,
                    "messageId": format!("{}-asks", self.id)});
                if version == Version::V03 {
                    status["message"]["kind"] = json!("message");
                }
                task["status"] = status;
            } else {
                task["artifacts"] = json!([{"artifactId": format!("{}-reply", self.id), "name": "reply", "parts": parts}]);
            }
        }
        if version == Version::V03 {
            task["kind"] = json!("task");
        }
        task
    }
}

/// The tasks of this server's run.
static TASKS: Mutex<VecDeque<Task>> = Mutex::new(VecDeque::new());

fn keep(task: &Task) {
    let mut tasks = TASKS.lock().unwrap_or_else(|e| e.into_inner());
    tasks.retain(|t| t.id != task.id);
    tasks.push_back(task.clone());
    while tasks.len() > TASKS_KEPT {
        tasks.pop_front();
    }
}

fn find(id: &str) -> Option<Task> {
    TASKS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .find(|t| t.id == id)
        .cloned()
}

/// A profile's agent card, its JSON-RPC endpoint at `base` (the channel's
/// public address).
pub fn card(profile: &Profile, base: &str) -> Value {
    let url = format!(
        "{}/v1/profiles/{}/a2a",
        base.trim_end_matches('/'),
        profile.name
    );
    // What the operator tells the agent stays theirs: the card says only
    // what the agent is.
    let description = format!(
        "{}, a Siontology agent: it acts within a charter on its ledger",
        profile.name
    );
    json!({
        "name": profile.name,
        "description": description,
        "version": env!("CARGO_PKG_VERSION"),
        // 0.3's fields.
        "protocolVersion": "0.3.0",
        "url": url,
        "preferredTransport": "JSONRPC",
        // 1.0's.
        "supportedInterfaces": [{"url": url, "protocolBinding": "JSONRPC", "protocolVersion": "1.0"}],
        "capabilities": {"streaming": true, "pushNotifications": false},
        "defaultInputModes": ["text/plain"],
        "defaultOutputModes": ["text/plain"],
        "skills": [{"id": "converse", "name": format!("Talk with {}", profile.name),
            "description": "A task in words: the agent answers with its tools, within its charter; what it waits to be approved is asked back.",
            "tags": ["siontology"]}],
        "securitySchemes": {"bearer": {"type": "http", "scheme": "bearer",
            "httpAuthSecurityScheme": {"scheme": "Bearer"}}},
        "security": [{"bearer": []}],
    })
}

fn rpc_error(id: &Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

fn rpc_result(id: &Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

/// The text of a message's parts (text parts of either version).
fn text_of(message: &Value) -> String {
    message["parts"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|part| part["kind"] == "text" || part["text"].is_string())
        .filter_map(|part| part["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// What became of a run, as a task's state.
fn state_of(answer: &Answer, waits: bool) -> &'static str {
    if waits {
        "input-required"
    } else if answer.stopped {
        "canceled"
    } else {
        "completed"
    }
}

/// Answers a JSON-RPC request to profile `name`'s A2A endpoint.
pub fn answer(
    ledger: SharedLedger,
    name: &str,
    body: &str,
    version_header: Option<&str>,
    presented: Option<&str>,
    admin: Option<&str>,
) -> Served {
    let whole = |status: u16, value: Value| {
        Served::Whole(crate::openai_api::Answered {
            status,
            content_type: "application/json",
            body: value.to_string(),
        })
    };
    let Ok(request) = serde_json::from_str::<Value>(body) else {
        return whole(400, rpc_error(&Value::Null, -32700, "not JSON"));
    };
    let id = request["id"].clone();
    let method = request["method"].as_str().unwrap_or_default().to_string();
    let version = Version::of(version_header, &method);
    let allowed = access(name, presented, admin);
    if allowed == Access::None {
        return whole(
            401,
            rpc_error(&id, -32600, "this token does not reach this profile"),
        );
    }
    let profile = match crate::profiles::get(name) {
        Ok(profile) => profile,
        Err(e) => return whole(404, rpc_error(&id, -32600, &e.to_string())),
    };
    let params = &request["params"];
    match method.as_str() {
        "message/send" | "SendMessage" | "message/stream" | "SendStreamingMessage" => {
            let message = &params["message"];
            let text = text_of(message);
            if text.trim().is_empty() {
                return whole(200, rpc_error(&id, -32602, "the message has no text part"));
            }
            if let Some(refused) = refused_for_talk(&text).filter(|_| allowed != Access::Approve) {
                return whole(200, rpc_error(&id, -32600, refused));
            }
            let context = message["contextId"]
                .as_str()
                .filter(|c| !c.is_empty() && c.len() <= 128 && !c.contains(char::is_control))
                .map(str::to_string)
                .unwrap_or_else(crate::runtime::new_id);
            let mut task = Task {
                id: crate::runtime::new_id(),
                context: context.clone(),
                profile: name.to_string(),
                state: "working".into(),
                reply: String::new(),
            };
            keep(&task);
            let key = format!("a2a:{name}:{context}");
            if method.contains("stream") || method == "SendStreamingMessage" {
                return Served::Streamed(streamed(ledger, profile, key, text, task, id, version));
            }
            match converse_with(
                ledger,
                &profile,
                &key,
                &text,
                &[],
                crate::runtime::Hooks::default(),
            ) {
                Ok(answer) => {
                    let waits = waits(&answer);
                    task.state = state_of(&answer, waits).into();
                    task.reply = answer.reply;
                }
                Err(e) => {
                    task.state = "failed".into();
                    task.reply = e.to_string();
                }
            }
            keep(&task);
            let result = match version {
                Version::V03 => task.json(version),
                Version::V10 => json!({"task": task.json(version)}),
            };
            whole(200, rpc_result(&id, result))
        }
        "tasks/get" | "GetTask" | "tasks/cancel" | "CancelTask" => {
            let task_id = params["id"].as_str().unwrap_or_default();
            let Some(task) = find(task_id).filter(|t| t.profile == name) else {
                return whole(200, rpc_error(&id, -32001, "no such task"));
            };
            if method.contains("cancel") || method == "CancelTask" {
                if task.state != "working" {
                    return whole(
                        200,
                        rpc_error(&id, -32002, "the task is done: it cannot be canceled"),
                    );
                }
                crate::channels::stop(&format!("a2a:{name}:{}", task.context));
                let canceled = Task {
                    state: "canceled".into(),
                    ..task
                };
                keep(&canceled);
                return whole(200, rpc_result(&id, canceled.json(version)));
            }
            whole(200, rpc_result(&id, task.json(version)))
        }
        _ if method.starts_with("tasks/pushNotificationConfig")
            || method.contains("PushNotification") =>
        {
            whole(
                200,
                rpc_error(&id, -32003, "push notifications are not offered"),
            )
        }
        _ => whole(200, rpc_error(&id, -32601, &format!("no method {method}"))),
    }
}

/// Whether the session the answer came from waits for a decision.
fn waits(answer: &Answer) -> bool {
    answer
        .session
        .as_deref()
        .and_then(|id| crate::runtime::Saved::load(id).ok())
        .is_some_and(|saved| saved.state.pending.is_some())
}

/// A task answered as Server-Sent Events: the task as it begins, its reply
/// as the agent writes it (an artifact that grows), and its state at the
/// end, marked final.
fn streamed(
    ledger: SharedLedger,
    profile: Profile,
    key: String,
    text: String,
    mut task: Task,
    id: Value,
    version: Version,
) -> Pieces {
    let (tell, pieces) = std::sync::mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let send = |result: Value| {
            let _ = tell.send(event(None, &rpc_result(&id, result)));
        };
        let artifact = format!("{}-reply", task.id);
        send(match version {
            Version::V03 => task.json(version),
            Version::V10 => json!({"task": task.json(version)}),
        });
        let growing = Mutex::new(Growing::default());
        let first = Mutex::new(true);
        let update = |more: &str| {
            let mut first = first.lock().unwrap_or_else(|e| e.into_inner());
            let mut update = json!({"taskId": task.id, "contextId": task.context,
                "artifact": {"artifactId": artifact, "name": "reply", "parts": [version.text_part(more)]},
                "append": !*first, "lastChunk": false});
            *first = false;
            match version {
                Version::V03 => {
                    update["kind"] = json!("artifact-update");
                    send(update)
                }
                Version::V10 => send(json!({"artifactUpdate": update})),
            }
        };
        let watch = |seen: &sion_agent::Seen| {
            let more = growing.lock().ok().and_then(|mut g| g.take(seen));
            if let Some(more) = more {
                update(&more);
            }
        };
        let answered = converse_with(
            ledger,
            &profile,
            &key,
            &text,
            &[],
            crate::runtime::Hooks::watched(&watch),
        );
        match answered {
            Ok(answer) => {
                let rest = growing
                    .lock()
                    .map(|g| g.rest(&answer.reply))
                    .unwrap_or_default();
                if !rest.is_empty() {
                    update(&rest);
                }
                let waits = waits(&answer);
                task.state = state_of(&answer, waits).into();
                task.reply = answer.reply;
            }
            Err(e) => {
                task.state = "failed".into();
                task.reply = e.to_string();
            }
        }
        keep(&task);
        let mut status = json!({"taskId": task.id, "contextId": task.context,
            "status": task.json(version)["status"].clone(), "final": true});
        match version {
            Version::V03 => {
                status["kind"] = json!("status-update");
                send(status)
            }
            Version::V10 => send(json!({"statusUpdate": status})),
        }
    });
    pieces
}

/// The profiles whose cards the channel gives (`sion agent serve --a2a`),
/// the first at `/.well-known/agent-card.json`, and the channel's public
/// address, if it was given.
#[derive(Clone, Debug, Default)]
pub struct Published {
    /// The profiles, in the order named.
    pub profiles: Vec<String>,
    /// The address other agents reach the channel at.
    pub base: Option<String>,
}

static PUBLISHED: Mutex<Option<Published>> = Mutex::new(None);

/// Publishes the cards of `published.profiles` from now on.
pub fn publish(published: Published) {
    *PUBLISHED.lock().unwrap_or_else(|e| e.into_inner()) = Some(published);
}

/// The card at `path` (`/.well-known/agent-card.json`, or
/// `/v1/profiles/<name>/agent-card.json`), for a published profile, with
/// the channel at `host` unless its public address was given.
pub fn card_at(path: &str, host: Option<&str>) -> Option<Value> {
    let published = PUBLISHED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()?;
    let name = match path {
        "/.well-known/agent-card.json" | "/.well-known/agent.json" => {
            published.profiles.first()?.clone()
        }
        _ => path
            .strip_prefix("/v1/profiles/")?
            .strip_suffix("/agent-card.json")?
            .to_string(),
    };
    if !published.profiles.contains(&name) {
        return None;
    }
    let profile = crate::profiles::get(&name).ok()?;
    let base = published
        .base
        .or_else(|| host.map(|host| format!("http://{host}")))?;
    Some(card(&profile, &base))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_version_is_written_as_it_is_spoken() {
        assert_eq!(Version::of(None, "message/send"), Version::V03);
        assert_eq!(Version::of(None, "SendMessage"), Version::V10);
        assert_eq!(Version::of(Some("1.0"), "message/send"), Version::V10);
        assert_eq!(Version::of(Some("0.3"), "SendMessage"), Version::V03);
        let task = Task {
            id: "t1".into(),
            context: "c1".into(),
            profile: "lead".into(),
            state: "completed".into(),
            reply: "85 USD left.".into(),
        };
        assert_eq!(
            task.json(Version::V03),
            json!({"id": "t1", "contextId": "c1", "kind": "task", "status": {"state": "completed"},
                "artifacts": [{"artifactId": "t1-reply", "name": "reply",
                    "parts": [{"kind": "text", "text": "85 USD left."}]}]})
        );
        assert_eq!(
            task.json(Version::V10),
            json!({"id": "t1", "contextId": "c1", "status": {"state": "TASK_STATE_COMPLETED"},
                "artifacts": [{"artifactId": "t1-reply", "name": "reply",
                    "parts": [{"text": "85 USD left.", "mediaType": "text/plain"}]}]})
        );
        let waiting = Task {
            state: "input-required".into(),
            ..task
        };
        let written = waiting.json(Version::V10);
        assert_eq!(written["status"]["state"], "TASK_STATE_INPUT_REQUIRED");
        assert_eq!(written["status"]["message"]["role"], "ROLE_AGENT");
        assert!(written.get("artifacts").is_none());
        assert_eq!(
            text_of(
                &json!({"parts": [{"kind": "text", "text": "a"}, {"kind": "file"}, {"text": "b"}]})
            ),
            "a\nb"
        );
    }
}
