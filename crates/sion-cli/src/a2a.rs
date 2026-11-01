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
//! runs (the last thousand).
//!
//! A client may have a task's updates pushed to it: each time the task's
//! state changes, the task is POSTed to the webhooks it registered for it
//! (`tasks/pushNotificationConfig/set`, or `CreateTaskPushNotificationConfig`;
//! get, list and delete too, or a message's own config), with its token in
//! `X-A2A-Notification-Token` and, if it gave Bearer credentials, those.
//! A webhook is an HTTPS address that resolves to a public one, unless the
//! operator lets them reach this machine's network (`--a2a-push-private`).
//! `tasks/resubscribe` (`SubscribeToTask`) streams a task's state as it
//! changes, until it is final.

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
    let changed = {
        let mut tasks = TASKS.lock().unwrap_or_else(|e| e.into_inner());
        let before = tasks
            .iter()
            .find(|t| t.id == task.id)
            .map(|t| t.state.clone());
        tasks.retain(|t| t.id != task.id);
        tasks.push_back(task.clone());
        let mut gone = Vec::new();
        while tasks.len() > TASKS_KEPT {
            if let Some(old) = tasks.pop_front() {
                gone.push(old.id);
            }
        }
        if !gone.is_empty() {
            let mut pushes = PUSHES.lock().unwrap_or_else(|e| e.into_inner());
            for id in gone {
                pushes.remove(&id);
            }
        }
        before.as_deref() != Some(task.state.as_str())
    };
    if changed {
        notify(task);
    }
}

/// Where a task's updates are pushed: a webhook its client registered.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Push {
    /// The config's id.
    id: String,
    /// The webhook.
    url: String,
    /// The client's token, sent back in `X-A2A-Notification-Token`.
    token: Option<String>,
    /// Bearer credentials the client asked to be called with.
    bearer: Option<String>,
    /// The wire version the client spoke, which its notifications are in.
    version: Version,
}

/// The most webhooks a task has.
const PUSHES_PER_TASK: usize = 8;

/// The webhooks of the tasks kept, by task.
static PUSHES: Mutex<std::collections::BTreeMap<String, Vec<Push>>> =
    Mutex::new(std::collections::BTreeMap::new());

fn pushes_of(task: &str) -> Vec<Push> {
    PUSHES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(task)
        .cloned()
        .unwrap_or_default()
}

/// Whether the operator lets webhooks reach this machine's network.
fn push_private() -> bool {
    PUBLISHED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .is_some_and(|p| p.push_private)
}

/// A webhook a client gave, checked: an absolute HTTPS address (HTTP too,
/// where the operator lets webhooks reach this machine's network), its
/// token and credentials of reasonable size, and Bearer the only scheme.
fn push_of(config: &Value, version: Version) -> Result<Push, (i64, String)> {
    let invalid = |why: &str| (-32602, why.to_string());
    let url = config["url"]
        .as_str()
        .ok_or_else(|| invalid("a push notification config needs a url"))?;
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://").filter(|_| push_private()))
        .ok_or_else(|| invalid("a webhook is an https:// address"))?;
    if rest.is_empty()
        || rest.starts_with('/')
        || url.len() > 2_048
        || url.contains(char::is_whitespace)
    {
        return Err(invalid("a webhook names its host"));
    }
    let token = match config["token"].as_str() {
        Some(token) if token.len() > 512 => return Err(invalid("the token is too long")),
        Some(token) if !token.is_empty() => Some(token.to_string()),
        _ => None,
    };
    let auth = &config["authentication"];
    let bearer = if auth.is_null() {
        None
    } else {
        let schemes: Vec<&str> = match version {
            Version::V03 => auth["schemes"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect(),
            Version::V10 => auth["scheme"].as_str().into_iter().collect(),
        };
        if !schemes.iter().any(|s| s.eq_ignore_ascii_case("bearer")) {
            return Err(invalid(
                "only Bearer authentication is offered for webhooks",
            ));
        }
        auth["credentials"]
            .as_str()
            .filter(|c| !c.is_empty() && c.len() <= 4_096 && !c.contains(['\r', '\n']))
            .map(str::to_string)
    };
    let id = config["id"]
        .as_str()
        .filter(|id| !id.is_empty() && id.len() <= 128 && !id.contains(char::is_control))
        .map(str::to_string)
        .unwrap_or_else(crate::runtime::new_id);
    Ok(Push {
        id,
        url: url.to_string(),
        token,
        bearer,
        version,
    })
}

/// Registers `push` for `task`, replacing a webhook of the same id.
fn add_push(task: &str, push: Push) -> Result<Push, (i64, String)> {
    let mut pushes = PUSHES.lock().unwrap_or_else(|e| e.into_inner());
    let list = pushes.entry(task.to_string()).or_default();
    list.retain(|p| p.id != push.id);
    if list.len() >= PUSHES_PER_TASK {
        return Err((
            -32602,
            format!("a task has at most {PUSHES_PER_TASK} webhooks"),
        ));
    }
    list.push(push.clone());
    Ok(push)
}

/// A webhook as the client is shown it (its credentials not).
fn config_json(task: &str, push: &Push) -> Value {
    let mut config = json!({"id": push.id, "url": push.url});
    if let Some(token) = &push.token {
        config["token"] = json!(token);
    }
    match push.version {
        Version::V03 => json!({"taskId": task, "pushNotificationConfig": config}),
        Version::V10 => {
            config["taskId"] = json!(task);
            config
        }
    }
}

/// Whether `method` is one of the webhook methods, in either version.
fn is_push_method(method: &str) -> bool {
    method.starts_with("tasks/pushNotificationConfig/")
        || method.ends_with("TaskPushNotificationConfig")
        || method.ends_with("TaskPushNotificationConfigs")
}

/// Answers a webhook method for profile `profile`'s tasks: set (create),
/// get, list and delete, as `version` writes them.
fn push_config(
    method: &str,
    params: &Value,
    profile: &str,
    version: Version,
) -> Result<Value, (i64, String)> {
    let v03 = method.starts_with("tasks/");
    let verb = if v03 {
        method
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase()
    } else {
        method
            .trim_end_matches('s')
            .strip_suffix("TaskPushNotificationConfig")
            .unwrap_or_default()
            .to_ascii_lowercase()
    };
    let task_id = if v03 && verb != "set" {
        &params["id"]
    } else {
        &params["taskId"]
    };
    let task = task_id
        .as_str()
        .and_then(find)
        .filter(|t| t.profile == profile)
        .ok_or((-32001, "no such task".to_string()))?;
    let named = if v03 {
        &params["pushNotificationConfigId"]
    } else {
        &params["id"]
    };
    match verb.as_str() {
        "set" | "create" => {
            let config = if v03 {
                &params["pushNotificationConfig"]
            } else {
                params
            };
            let push = add_push(&task.id, push_of(config, version)?)?;
            Ok(config_json(&task.id, &push))
        }
        "get" => {
            let pushes = pushes_of(&task.id);
            let push = match named.as_str() {
                Some(named) => pushes.iter().find(|p| p.id == named),
                None => pushes.first(),
            }
            .ok_or((-32001, "the task has no such webhook".to_string()))?;
            Ok(config_json(&task.id, push))
        }
        "list" => {
            let configs: Vec<Value> = pushes_of(&task.id)
                .iter()
                .map(|p| config_json(&task.id, p))
                .collect();
            Ok(if v03 {
                json!(configs)
            } else {
                json!({"configs": configs, "nextPageToken": ""})
            })
        }
        "delete" => {
            let named = named
                .as_str()
                .ok_or((-32602, "say which webhook to delete".to_string()))?;
            let mut pushes = PUSHES.lock().unwrap_or_else(|e| e.into_inner());
            let list = pushes.entry(task.id.clone()).or_default();
            let before = list.len();
            list.retain(|p| p.id != named);
            if list.len() == before {
                return Err((-32001, "the task has no such webhook".to_string()));
            }
            Ok(if v03 { Value::Null } else { json!({}) })
        }
        _ => Err((-32601, format!("no method {method}"))),
    }
}

/// Pushes `task`, as it now stands, to each webhook registered for it, each
/// from a thread of its own: a webhook that does not answer holds up no
/// other, nor the task.
fn notify(task: &Task) {
    let pushes = pushes_of(&task.id);
    if pushes.is_empty() {
        return;
    }
    let private = push_private();
    for push in pushes {
        let task = task.clone();
        std::thread::spawn(move || {
            let _ = deliver(&push, &task, private);
        });
    }
}

/// POSTs `task` to `push`'s webhook, in its client's version: the task in
/// 0.3, a stream response holding it in 1.0.
fn deliver(push: &Push, task: &Task, private: bool) -> Result<(), String> {
    let client = sion_http::Client::new(sion_http::Options {
        timeout: std::time::Duration::from_secs(10),
        allow_private: private,
        // A webhook on this machine's network is not reached through the
        // proxy.
        proxy_from_env: !private,
        ..sion_http::Options::default()
    })
    .map_err(|e| e.to_string())?;
    let body = match push.version {
        Version::V03 => task.json(Version::V03),
        Version::V10 => json!({"task": task.json(Version::V10)}),
    };
    let bearer = push.bearer.as_ref().map(|b| format!("Bearer {b}"));
    let mut headers: Vec<(&str, &str)> = Vec::new();
    if let Some(token) = &push.token {
        headers.push(("X-A2A-Notification-Token", token));
    }
    if let Some(bearer) = &bearer {
        headers.push(("Authorization", bearer));
    }
    client
        .post_json(&push.url, &headers, body.to_string().as_bytes(), 64 * 1024)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Whether a task in `state` changes no more: its stream ends there.
fn is_final(state: &str) -> bool {
    matches!(
        state,
        "completed" | "failed" | "canceled" | "rejected" | "input-required"
    )
}

/// A task's updates as Server-Sent Events, for a client that subscribes to
/// it again: the task as it stands, then its state each time it changes,
/// until it is final (at most an hour).
fn resubscribed(task: Task, id: Value, version: Version) -> Pieces {
    let (tell, pieces) = std::sync::mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let send = |result: Value| tell.send(event(None, &rpc_result(&id, result))).is_ok();
        let status = |task: &Task| {
            let mut status = json!({"taskId": task.id, "contextId": task.context,
                "status": task.json(version)["status"].clone(), "final": is_final(&task.state)});
            match version {
                Version::V03 => {
                    status["kind"] = json!("status-update");
                    status
                }
                Version::V10 => json!({"statusUpdate": status}),
            }
        };
        let first = match version {
            Version::V03 => task.json(version),
            Version::V10 => json!({"task": task.json(version)}),
        };
        if !send(first) {
            return;
        }
        let mut last = task;
        let until = std::time::Instant::now() + std::time::Duration::from_secs(3_600);
        while !is_final(&last.state) && std::time::Instant::now() < until {
            std::thread::sleep(std::time::Duration::from_millis(200));
            let Some(now) = find(&last.id) else {
                return;
            };
            if now.state != last.state && !send(status(&now)) {
                return;
            }
            last = now;
        }
    });
    pieces
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
        "capabilities": {"streaming": true, "pushNotifications": true},
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
            // The message's own webhook, if it gives one, before the task
            // is first kept.
            let configuration = &params["configuration"];
            let given = match version {
                Version::V03 => &configuration["pushNotificationConfig"],
                Version::V10 => &configuration["taskPushNotificationConfig"],
            };
            if !given.is_null() {
                if let Err((code, why)) =
                    push_of(given, version).and_then(|push| add_push(&task.id, push))
                {
                    return whole(200, rpc_error(&id, code, &why));
                }
            }
            keep(&task);
            let key = format!("a2a:{name}:{context}");
            // Another agent's message is content from outside: it taints the
            // session. A decision it carries is its token's holder's.
            let who = crate::channels::holder(name, presented, admin)
                .map(|holder| format!("a2a:{holder}"));
            if method.contains("stream") || method == "SendStreamingMessage" {
                return Served::Streamed(streamed(
                    ledger,
                    profile,
                    (key, who),
                    text,
                    task,
                    id,
                    version,
                ));
            }
            match converse_with(
                ledger,
                &profile,
                &key,
                &text,
                &[],
                crate::runtime::Hooks {
                    who: who.as_deref(),
                    outside: true,
                    ..crate::runtime::Hooks::default()
                },
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
        _ if is_push_method(&method) => match push_config(&method, params, name, version) {
            Ok(result) => whole(200, rpc_result(&id, result)),
            Err((code, why)) => whole(200, rpc_error(&id, code, &why)),
        },
        "tasks/resubscribe" | "SubscribeToTask" => {
            let task_id = params["id"].as_str().unwrap_or_default();
            let Some(task) = find(task_id).filter(|t| t.profile == name) else {
                return whole(200, rpc_error(&id, -32001, "no such task"));
            };
            Served::Streamed(resubscribed(task, id, version))
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
    (key, who): (String, Option<String>),
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
            crate::runtime::Hooks {
                who: who.as_deref(),
                outside: true,
                ..crate::runtime::Hooks::watched(&watch)
            },
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
    /// Whether other agents' webhooks may be on this machine's network
    /// (loopback and private addresses, plain HTTP): for testing.
    pub push_private: bool,
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
    fn a_tasks_updates_are_pushed_to_the_webhooks_its_client_gave() {
        use std::io::{BufRead, BufReader, Read, Write};
        // A webhook on this machine, which the operator allowed.
        publish(Published {
            profiles: Vec::new(),
            base: None,
            push_private: true,
        });
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/hook", listener.local_addr().unwrap());
        let heard = std::thread::spawn(move || {
            let mut heard = Vec::new();
            for _ in 0..2 {
                let (stream, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut headers = Vec::new();
                let mut length = 0;
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if line.trim().is_empty() {
                        break;
                    }
                    if let Some(n) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = n.trim().parse().unwrap();
                    }
                    headers.push(line.trim().to_string());
                }
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                let mut stream = stream;
                stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                    .unwrap();
                heard.push((headers, String::from_utf8(body).unwrap()));
            }
            heard
        });
        let mut task = Task {
            id: format!("push-{}", std::process::id()),
            context: "c1".into(),
            profile: "lead".into(),
            state: "working".into(),
            reply: String::new(),
        };
        keep(&task);
        let config = json!({"url": url, "token": "t0k3n", "id": "hook-1"});
        let set = json!({"taskId": task.id, "pushNotificationConfig": config});
        let shown = push_config(
            "tasks/pushNotificationConfig/set",
            &set,
            "lead",
            Version::V03,
        )
        .unwrap();
        assert_eq!(shown["pushNotificationConfig"]["id"], "hook-1");
        // Another profile's token does not reach the task.
        assert!(push_config(
            "tasks/pushNotificationConfig/set",
            &set,
            "other",
            Version::V03
        )
        .is_err());
        let listed = push_config(
            "tasks/pushNotificationConfig/list",
            &json!({"id": task.id}),
            "lead",
            Version::V03,
        )
        .unwrap();
        assert_eq!(listed.as_array().map(Vec::len), Some(1));
        // Each change of state is pushed, the task as it stands.
        task.state = "input-required".into();
        task.reply = "Approve paying bob 3 USD?".into();
        keep(&task);
        std::thread::sleep(std::time::Duration::from_millis(300));
        task.state = "completed".into();
        task.reply = "Paid.".into();
        keep(&task);
        let heard = heard.join().unwrap();
        let states: Vec<String> = heard
            .iter()
            .map(|(_, body)| {
                serde_json::from_str::<Value>(body).unwrap()["status"]["state"].to_string()
            })
            .collect();
        assert!(
            states.contains(&"\"input-required\"".to_string())
                && states.contains(&"\"completed\"".to_string()),
            "{states:?}"
        );
        for (headers, _) in &heard {
            assert!(
                headers
                    .iter()
                    .any(|h| h.eq_ignore_ascii_case("x-a2a-notification-token: t0k3n")),
                "{headers:?}"
            );
        }
        // 1.0's shapes; a webhook that is not HTTPS is refused unless allowed.
        let v10 = json!({"taskId": task.id, "url": "https://agents.example.org/hook", "id": "hook-2",
            "authentication": {"scheme": "Bearer", "credentials": "abc"}});
        let shown = push_config(
            "CreateTaskPushNotificationConfig",
            &v10,
            "lead",
            Version::V10,
        )
        .unwrap();
        assert_eq!(
            (shown["id"].as_str(), shown.get("authentication")),
            (Some("hook-2"), None)
        );
        let listed = push_config(
            "ListTaskPushNotificationConfigs",
            &json!({"taskId": task.id}),
            "lead",
            Version::V10,
        )
        .unwrap();
        assert_eq!(listed["configs"].as_array().map(Vec::len), Some(2));
        push_config(
            "DeleteTaskPushNotificationConfig",
            &json!({"taskId": task.id, "id": "hook-2"}),
            "lead",
            Version::V10,
        )
        .unwrap();
        let basic = json!({"taskId": task.id, "url": "https://x.example/h", "authentication": {"scheme": "Basic"}});
        assert!(push_config(
            "CreateTaskPushNotificationConfig",
            &basic,
            "lead",
            Version::V10
        )
        .is_err());
        publish(Published::default());
        let plain = json!({"taskId": task.id, "pushNotificationConfig": {"url": "http://agents.example.org/h"}});
        let refused = push_config(
            "tasks/pushNotificationConfig/set",
            &plain,
            "lead",
            Version::V03,
        )
        .unwrap_err();
        assert!(refused.1.contains("https://"), "{refused:?}");
        assert!(
            is_push_method("tasks/pushNotificationConfig/get")
                && is_push_method("GetTaskPushNotificationConfig")
        );
        assert!(!is_push_method("tasks/get"));
    }

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
