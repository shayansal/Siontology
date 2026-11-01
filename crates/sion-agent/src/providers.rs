//! Model providers over HTTPS, through `sion-http` (bounded, guarded, with
//! the hybrid post-quantum key exchange first): the Anthropic Messages API,
//! any OpenAI-compatible chat completions API, and Google's Gemini API.
//!
//! All stream ([`Model::stream`]): the request asks for Server-Sent Events,
//! and the text is told as each event brings it, while the answer is built
//! from the same events into what a whole answer would have been. An
//! answer that stops before its end is a transport failure, and an error
//! the stream reports partway is mapped as the status it stands for. A
//! server that answers whole although asked to stream is read whole.
//!
//! Each takes one API key or several ([`Keys`]): a key the provider limits
//! or refuses rests, and the next is tried.

use serde_json::{json, Value};

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::mcp_http::Events;
use crate::model::{
    cut_off, images_shown, tell_whole, Block, Delta, Image, Message, Model, ModelError, Request,
    Response, Role, Stop, Usage,
};

/// What stands in a conversation for a picture a model is no longer shown.
pub const IMAGE_NOT_SHOWN: &str = "[a picture, no longer shown: take a new one to see]";

/// The pictures of a request, as the providers walk them: which are shown,
/// and the place of the next one.
struct Pictures<'a> {
    kept: &'a BTreeMap<String, String>,
    shown: BTreeSet<usize>,
    next: usize,
}

impl<'a> Pictures<'a> {
    fn of(messages: &[Message], kept: &'a BTreeMap<String, String>) -> Pictures<'a> {
        Pictures {
            kept,
            shown: images_shown(messages, kept),
            next: 0,
        }
    }

    /// The bytes of the next picture, `image`, if it is shown.
    fn next(&mut self, image: &Image) -> Option<&'a str> {
        let at = self.next;
        self.next += 1;
        if self.shown.contains(&at) {
            self.kept.get(&image.digest).map(String::as_str)
        } else {
            None
        }
    }
}

/// The most bytes of a model's answer read.
const MAX_ANSWER: u64 = 8 << 20;

/// The most bytes of one event of a streamed answer.
const MAX_EVENT: usize = 1 << 20;

/// `POST url` with `body`, asking for the answer as Server-Sent Events; an
/// error status is mapped as [`http_error`] maps it.
fn post_stream(
    client: &sion_http::Client,
    url: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> Result<sion_http::Response, ModelError> {
    let mut all = vec![
        ("Content-Type", "application/json"),
        ("Accept", "text/event-stream"),
    ];
    all.extend_from_slice(headers);
    let mut response = client
        .send("POST", url, &all, Some(body))
        .map_err(http_error)?;
    if !(200..300).contains(&response.status) {
        let mut start = Vec::new();
        let _ = response.body().take(2_048).read_to_end(&mut start);
        return Err(http_error(sion_http::HttpError::Status {
            status: response.status,
            body: String::from_utf8_lossy(&start).into_owned(),
        }));
    }
    Ok(response)
}

/// Whether `response` streams events, rather than an answer whole.
fn streams(response: &sion_http::Response) -> bool {
    response.media_type().as_deref() == Some("text/event-stream")
}

/// An answer that came whole, although it was asked to stream: read in
/// full, with `read`, and told at once.
fn whole(
    response: sion_http::Response,
    read: fn(&Value) -> Result<Response, ModelError>,
    seen: &mut dyn FnMut(Delta) -> bool,
) -> Result<Response, ModelError> {
    let body = response.read_all(MAX_ANSWER).map_err(http_error)?;
    let value: Value = serde_json::from_slice(&body)
        .map_err(|e| ModelError::Protocol(format!("answer is not JSON: {e}")))?;
    let answer = read(&value)?;
    tell_whole(&answer, seen);
    Ok(answer)
}

/// A call's input from the JSON its arguments were streamed as: an empty
/// object for none, and what could not be parsed kept as it is.
fn input_of(arguments: &str) -> Value {
    if arguments.trim().is_empty() {
        return json!({});
    }
    serde_json::from_str(arguments).unwrap_or_else(|_| json!({ "_unparsed": arguments }))
}

/// The error a stream reports partway, as the status it stands for: the
/// provider overloaded or failing is transient, the rest is not.
fn stream_error(error: &Value) -> ModelError {
    let kind = error["type"]
        .as_str()
        .or_else(|| error["code"].as_str())
        .unwrap_or("error");
    let message = error["message"].as_str().unwrap_or("no message");
    let said = format!("{kind} while streaming: {message}");
    match kind {
        "overloaded_error"
        | "rate_limit_error"
        | "api_error"
        | "timeout_error"
        | "server_error"
        | "rate_limit_exceeded"
        | "insufficient_capacity" => ModelError::Busy(said),
        _ => ModelError::Refused(said),
    }
}

fn http_error(error: sion_http::HttpError) -> ModelError {
    use sion_http::HttpError::*;
    match error {
        Status { status, body } => match status {
            408 | 409 | 425 | 429 | 500 | 502 | 503 | 504 | 529 => {
                ModelError::Busy(format!("HTTP {status}: {body}"))
            }
            _ => ModelError::Refused(format!("HTTP {status}: {body}")),
        },
        Transport(e) => ModelError::Transport(e),
        Closed { code, reason } => {
            ModelError::Transport(format!("the connection was closed ({code}): {reason}"))
        }
        Refused(e) | Config(e) => ModelError::Refused(e),
        TooLarge(n) => ModelError::Protocol(format!("answer longer than {n} bytes")),
    }
}

/// How long a key rests that the provider limited (too many requests).
pub const LIMITED_REST: Duration = Duration::from_secs(60);

/// How long a key rests that the provider refused (not authorized, or out
/// of credit).
pub const REFUSED_REST: Duration = Duration::from_secs(3_600);

/// A provider's API keys, used in turn, one call after another: a key the
/// provider limits rests [`LIMITED_REST`], one it refuses [`REFUSED_REST`],
/// and the next key is tried at once, each once per call. Keys at rest are
/// tried last, the soonest awake first. Shown, it says only how many.
pub struct Keys {
    keys: Vec<String>,
    next: AtomicUsize,
    resting: Mutex<Vec<Option<Instant>>>,
}

impl std::fmt::Debug for Keys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Keys({})", self.keys.len())
    }
}

impl Keys {
    /// The keys of `list`: one, or several separated by commas.
    pub fn parse(list: &str) -> Keys {
        let keys: Vec<String> = list
            .split(',')
            .map(str::trim)
            .filter(|key| !key.is_empty())
            .map(str::to_string)
            .collect();
        let resting = Mutex::new(vec![None; keys.len()]);
        Keys {
            keys,
            next: AtomicUsize::new(0),
            resting,
        }
    }

    /// How many keys there are.
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// Calls `send` with a key, and with the next while the provider
    /// limits or refuses the one it was given and another may do better:
    /// what the last call returned. With no keys, `send` is given none.
    pub fn send<T>(
        &self,
        mut send: impl FnMut(Option<&str>) -> Result<T, ModelError>,
    ) -> Result<T, ModelError> {
        if self.keys.is_empty() {
            return send(None);
        }
        let now = Instant::now();
        let count = self.keys.len();
        let start = self.next.fetch_add(1, Ordering::Relaxed) % count;
        let resting = self
            .resting
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let mut order: Vec<usize> = (0..count).map(|i| (start + i) % count).collect();
        order.sort_by_key(|&i| resting[i].filter(|until| *until > now));
        let mut last = None;
        for at in order {
            match send(Some(&self.keys[at])) {
                Err(error) if count > 1 => match rest_for(&error) {
                    Some(rest) => {
                        if let Some(until) = self
                            .resting
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .get_mut(at)
                        {
                            *until = Some(Instant::now() + rest);
                        }
                        last = Some(error);
                    }
                    None => return Err(error),
                },
                answered => return answered,
            }
        }
        Err(last.unwrap_or_else(|| ModelError::Refused("no key was tried".into())))
    }
}

/// How long a key rests after `error`, if another key may do better: the
/// provider limited it, or refused it.
fn rest_for(error: &ModelError) -> Option<Duration> {
    match error {
        ModelError::Busy(why) if why.starts_with("HTTP 429") => Some(LIMITED_REST),
        ModelError::Refused(why)
            if ["HTTP 401", "HTTP 402", "HTTP 403"]
                .iter()
                .any(|status| why.starts_with(status)) =>
        {
            Some(REFUSED_REST)
        }
        _ => None,
    }
}

/// The Anthropic Messages API (`POST /v1/messages`, API version
/// `2026-10-21`: no sampling parameters are sent, and the top-level
/// `cache_control` asks for automatic prompt caching).
pub struct Anthropic {
    client: sion_http::Client,
    /// The API's base URL, `https://api.anthropic.com` by default.
    pub base: String,
    /// The model, for example `claude-opus-5-5`.
    pub model: String,
    keys: Keys,
    /// The `anthropic-version` header.
    pub version: String,
    /// Ask the API to cache the conversation's prefix.
    pub cache: bool,
}

impl std::fmt::Debug for Anthropic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Anthropic({} at {})", self.model, self.base)
    }
}

impl Anthropic {
    /// A provider for `model` with the API key `key` (several, separated
    /// by commas, are used in turn: [`Keys`]).
    pub fn new(client: sion_http::Client, model: &str, key: &str) -> Anthropic {
        Anthropic {
            client,
            base: "https://api.anthropic.com".into(),
            model: model.into(),
            keys: Keys::parse(key),
            version: "2026-10-21".into(),
            cache: true,
        }
    }

    /// The request body for `request`.
    pub fn body(&self, request: &Request) -> Value {
        let mut pictures = Pictures::of(&request.messages, &request.images);
        let messages: Vec<Value> = request
            .messages
            .iter()
            .map(|m| anthropic_message(m, &mut pictures))
            .collect();
        let tools: Vec<Value> = request
            .tools
            .iter()
            .map(|t| json!({"name": t.name, "description": t.description, "input_schema": t.input_schema}))
            .collect();
        let mut body = json!({
            "model": self.model,
            "max_tokens": request.max_tokens,
            "system": request.system,
            "messages": messages,
        });
        if !tools.is_empty() {
            body["tools"] = json!(tools);
        }
        if self.cache {
            body["cache_control"] = json!({"type": "ephemeral"});
        }
        body
    }
}

fn anthropic_message(message: &Message, pictures: &mut Pictures<'_>) -> Value {
    let content: Vec<Value> = message
        .content
        .iter()
        .map(|b| match b {
            Block::Text { text } => json!({"type": "text", "text": text}),
            Block::ToolUse {
                id, name, input, ..
            } => {
                json!({"type": "tool_use", "id": id, "name": name, "input": input})
            }
            Block::ToolResult {
                id,
                content,
                is_error,
                images,
            } if images.is_empty() => {
                json!({"type": "tool_result", "tool_use_id": id, "content": content, "is_error": is_error})
            }
            // A result with pictures is a list of blocks: its text, then
            // each picture, or a line for one no longer shown.
            Block::ToolResult {
                id,
                content,
                is_error,
                images,
            } => {
                let mut parts = Vec::new();
                if !content.is_empty() {
                    parts.push(json!({"type": "text", "text": content}));
                }
                for image in images {
                    parts.push(match pictures.next(image) {
                        Some(data) => json!({"type": "image", "source": {
                            "type": "base64", "media_type": image.media_type, "data": data}}),
                        None => json!({"type": "text", "text": IMAGE_NOT_SHOWN}),
                    });
                }
                json!({"type": "tool_result", "tool_use_id": id, "content": parts, "is_error": is_error})
            }
            Block::Image { image } => match pictures.next(image) {
                Some(data) => json!({"type": "image", "source": {
                    "type": "base64", "media_type": image.media_type, "data": data}}),
                None => json!({"type": "text", "text": IMAGE_NOT_SHOWN}),
            },
        })
        .collect();
    json!({"role": match message.role { Role::User => "user", Role::Assistant => "assistant" }, "content": content})
}

/// Reads an Anthropic Messages API answer.
pub fn anthropic_response(answer: &Value) -> Result<Response, ModelError> {
    let blocks = answer["content"]
        .as_array()
        .ok_or_else(|| ModelError::Protocol("no content in the answer".into()))?;
    let mut content = Vec::new();
    for block in blocks {
        match block["type"].as_str() {
            Some("text") => content.push(Block::text(block["text"].as_str().unwrap_or_default())),
            Some("tool_use") => content.push(Block::ToolUse {
                id: block["id"].as_str().unwrap_or_default().to_string(),
                name: block["name"].as_str().unwrap_or_default().to_string(),
                input: block["input"].clone(),
                signature: None,
            }),
            // Thinking and other blocks are not part of the conversation
            // this runtime keeps.
            _ => {}
        }
    }
    Ok(Response {
        content,
        stop: anthropic_stop(answer["stop_reason"].as_str()),
        usage: anthropic_usage(&answer["usage"]),
        model: answer["model"].as_str().unwrap_or_default().to_string(),
    })
}

fn anthropic_stop(reason: Option<&str>) -> Stop {
    match reason {
        Some("end_turn") | Some("stop_sequence") => Stop::EndTurn,
        Some("tool_use") => Stop::ToolUse,
        Some("max_tokens") => Stop::MaxTokens,
        Some("refusal") => Stop::Refusal,
        Some("pause_turn") => Stop::PauseTurn,
        other => Stop::Other(other.unwrap_or("none").to_string()),
    }
}

/// A block of a streamed Anthropic answer, as it is built.
enum Building {
    Text(String),
    Tool {
        id: String,
        name: String,
        json: String,
    },
    /// Thinking and the rest, not kept (as [`anthropic_response`] does not).
    Other,
}

/// Reads a streamed Anthropic Messages API answer (`"stream": true`) from
/// its events, telling `seen` its text and its calls as they come: the
/// answer is what [`anthropic_response`] reads from the same answer whole.
/// Thinking is not told, nor kept.
pub fn anthropic_stream<R: Read>(
    events: &mut Events<R>,
    seen: &mut dyn FnMut(Delta) -> bool,
) -> Result<Response, ModelError> {
    let mut blocks: BTreeMap<u64, Building> = BTreeMap::new();
    let mut texts = 0usize;
    let mut model = String::new();
    let mut usage = Usage::default();
    let mut stop: Option<String> = None;
    let mut ended = false;
    // Whether the watcher cut the answer off.
    let mut cut = false;
    while let Some(event) = events.next_event() {
        let event = event.map_err(ModelError::Transport)?;
        let Ok(data) = serde_json::from_str::<Value>(&event.data) else {
            continue;
        };
        let index = data["index"].as_u64().unwrap_or(0);
        match data["type"].as_str().unwrap_or(event.name.as_str()) {
            "message_start" => {
                let message = &data["message"];
                model = message["model"].as_str().unwrap_or_default().to_string();
                usage = anthropic_usage(&message["usage"]);
            }
            "content_block_start" => {
                let block = &data["content_block"];
                let building = match block["type"].as_str() {
                    Some("text") => {
                        let text = block["text"].as_str().unwrap_or_default().to_string();
                        // Told as the answer's text reads: blocks joined by
                        // line feeds.
                        let told = if texts == 0 {
                            text.clone()
                        } else {
                            format!("\n{text}")
                        };
                        texts += 1;
                        if !told.is_empty() {
                            cut = !seen(Delta::Text(told));
                        }
                        Building::Text(text)
                    }
                    Some("tool_use") => {
                        let name = block["name"].as_str().unwrap_or_default().to_string();
                        cut = !seen(Delta::Call(name.clone()));
                        Building::Tool {
                            id: block["id"].as_str().unwrap_or_default().to_string(),
                            name,
                            json: String::new(),
                        }
                    }
                    _ => Building::Other,
                };
                blocks.insert(index, building);
            }
            "content_block_delta" => {
                let delta = &data["delta"];
                match (blocks.get_mut(&index), delta["type"].as_str()) {
                    (Some(Building::Text(text)), Some("text_delta")) => {
                        let more = delta["text"].as_str().unwrap_or_default();
                        text.push_str(more);
                        if !more.is_empty() {
                            cut = !seen(Delta::Text(more.to_string()));
                        }
                    }
                    (Some(Building::Tool { json, .. }), Some("input_json_delta")) => {
                        json.push_str(delta["partial_json"].as_str().unwrap_or_default());
                    }
                    _ => {}
                }
            }
            "message_delta" => {
                if let Some(reason) = data["delta"]["stop_reason"].as_str() {
                    stop = Some(reason.to_string());
                }
                let more = &data["usage"];
                if let Some(output) = more["output_tokens"].as_u64() {
                    usage.output = output;
                }
                if let Some(input) = more["input_tokens"].as_u64() {
                    let read = more["cache_read_input_tokens"]
                        .as_u64()
                        .unwrap_or(usage.cache_read);
                    let write = more["cache_creation_input_tokens"]
                        .as_u64()
                        .unwrap_or(usage.cache_write);
                    usage.input = input + read + write;
                    usage.cache_read = read;
                    usage.cache_write = write;
                }
            }
            "message_stop" => {
                ended = true;
                break;
            }
            "error" => return Err(stream_error(&data["error"])),
            // `ping`, `content_block_stop`, and what a later version adds.
            _ => {}
        }
        // Cut off: nothing more is read, and the connection closes.
        if cut {
            break;
        }
    }
    if cut {
        let texts = blocks.into_values().filter_map(|block| match block {
            Building::Text(text) => Some(text),
            _ => None,
        });
        return Ok(cut_off(texts, usage, model));
    }
    if !ended {
        return Err(ModelError::Transport(
            "the answer stopped before its end".into(),
        ));
    }
    let content = blocks
        .into_values()
        .filter_map(|block| match block {
            Building::Text(text) => Some(Block::text(text)),
            Building::Tool { id, name, json } => Some(Block::ToolUse {
                id,
                name,
                input: input_of(&json),
                signature: None,
            }),
            Building::Other => None,
        })
        .collect();
    Ok(Response {
        content,
        stop: anthropic_stop(stop.as_deref()),
        usage,
        model,
    })
}

impl Anthropic {
    fn url(&self) -> String {
        format!("{}/v1/messages", self.base.trim_end_matches('/'))
    }
}

impl Model for Anthropic {
    fn id(&self) -> &str {
        &self.model
    }

    fn complete(&self, request: &Request) -> Result<Response, ModelError> {
        let body = serde_json::to_vec(&self.body(request))
            .map_err(|e| ModelError::Protocol(e.to_string()))?;
        let answer = self.keys.send(|key| {
            self.client
                .post_json(
                    &self.url(),
                    &[
                        ("x-api-key", key.unwrap_or_default()),
                        ("anthropic-version", &self.version),
                    ],
                    &body,
                    MAX_ANSWER,
                )
                .map_err(http_error)
        })?;
        let value: Value = serde_json::from_slice(&answer.body)
            .map_err(|e| ModelError::Protocol(format!("answer is not JSON: {e}")))?;
        anthropic_response(&value)
    }

    fn stream(
        &self,
        request: &Request,
        seen: &mut dyn FnMut(Delta) -> bool,
    ) -> Result<Response, ModelError> {
        let mut body = self.body(request);
        body["stream"] = json!(true);
        let body = serde_json::to_vec(&body).map_err(|e| ModelError::Protocol(e.to_string()))?;
        let mut response = self.keys.send(|key| {
            post_stream(
                &self.client,
                &self.url(),
                &[
                    ("x-api-key", key.unwrap_or_default()),
                    ("anthropic-version", &self.version),
                ],
                &body,
            )
        })?;
        if !streams(&response) {
            return whole(response, anthropic_response, seen);
        }
        anthropic_stream(&mut Events::new(response.body(), MAX_EVENT), seen)
    }
}

/// Any API that speaks OpenAI's chat completions with tools (`POST
/// {base}/chat/completions`), local servers included.
pub struct OpenAiCompatible {
    client: sion_http::Client,
    /// The API's base URL, for example `https://api.openai.com/v1`.
    pub base: String,
    /// The model.
    pub model: String,
    keys: Keys,
}

impl std::fmt::Debug for OpenAiCompatible {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "OpenAiCompatible({} at {})", self.model, self.base)
    }
}

impl OpenAiCompatible {
    /// A provider at `base` for `model`, with an optional bearer key
    /// (several, separated by commas, are used in turn: [`Keys`]).
    pub fn new(client: sion_http::Client, base: &str, model: &str, key: Option<&str>) -> Self {
        OpenAiCompatible {
            client,
            base: base.into(),
            model: model.into(),
            keys: Keys::parse(key.unwrap_or_default()),
        }
    }

    /// The request body for `request`.
    pub fn body(&self, request: &Request) -> Value {
        let messages = openai_messages(&request.system, &request.messages, &request.images);
        let tools: Vec<Value> = request
            .tools
            .iter()
            .map(|t| json!({"type": "function", "function": {"name": t.name, "description": t.description, "parameters": t.input_schema}}))
            .collect();
        let mut body =
            json!({"model": self.model, "messages": messages, "max_tokens": request.max_tokens});
        if !tools.is_empty() {
            body["tools"] = json!(tools);
        }
        body
    }
}

/// A conversation in OpenAI's chat format: the system prompt, then each
/// message, with tool calls on assistant messages and one `tool` message per
/// result. A `tool` message holds text only: the pictures results returned
/// follow them in a user message, those `images` still keeps and among the
/// last shown.
pub fn openai_messages(
    system: &str,
    conversation: &[Message],
    images: &BTreeMap<String, String>,
) -> Vec<Value> {
    let mut pictures = Pictures::of(conversation, images);
    let mut messages = vec![json!({"role": "system", "content": system})];
    for message in conversation {
        match message.role {
            Role::Assistant => {
                let calls: Vec<Value> = message
                    .tool_uses()
                    .map(|(id, name, input)| {
                        json!({"id": id, "type": "function", "function": {"name": name, "arguments": input.to_string()}})
                    })
                    .collect();
                let mut m = json!({"role": "assistant", "content": message.text()});
                if !calls.is_empty() {
                    m["tool_calls"] = json!(calls);
                }
                messages.push(m);
            }
            Role::User => {
                let mut shown: Vec<Value> = Vec::new();
                // The person's own pictures, with their text.
                let mut own: Vec<Value> = Vec::new();
                for block in &message.content {
                    if let Block::Image { image } = block {
                        own.push(match pictures.next(image) {
                            Some(data) => json!({"type": "image_url", "image_url": {
                                "url": format!("data:{};base64,{data}", image.media_type)}}),
                            None => json!({"type": "text", "text": IMAGE_NOT_SHOWN}),
                        });
                    }
                    if let Block::ToolResult {
                        id,
                        content,
                        is_error,
                        images,
                    } = block
                    {
                        let mut content = if *is_error {
                            format!("error: {content}")
                        } else {
                            content.clone()
                        };
                        for image in images {
                            match pictures.next(image) {
                                Some(data) => {
                                    content.push_str("\n[the picture it returned follows]");
                                    shown.push(json!({"type": "image_url", "image_url": {
                                        "url": format!("data:{};base64,{data}", image.media_type)}}));
                                }
                                None => {
                                    content.push('\n');
                                    content.push_str(IMAGE_NOT_SHOWN);
                                }
                            }
                        }
                        messages
                            .push(json!({"role": "tool", "tool_call_id": id, "content": content}));
                    }
                }
                if !shown.is_empty() {
                    let mut parts = vec![json!({"type": "text",
                        "text": "The pictures the tool calls above returned:"})];
                    parts.extend(shown);
                    messages.push(json!({"role": "user", "content": parts}));
                }
                let text = message.text();
                if !own.is_empty() {
                    let mut parts = Vec::new();
                    if !text.is_empty() {
                        parts.push(json!({"type": "text", "text": text}));
                    }
                    parts.extend(own);
                    messages.push(json!({"role": "user", "content": parts}));
                } else if !text.is_empty() {
                    messages.push(json!({"role": "user", "content": text}));
                }
            }
        }
    }
    messages
}

/// Reads an OpenAI-compatible chat completion.
pub fn openai_response(answer: &Value) -> Result<Response, ModelError> {
    let choice = &answer["choices"][0];
    let message = &choice["message"];
    if message.is_null() {
        return Err(ModelError::Protocol("no choices in the answer".into()));
    }
    let mut content = Vec::new();
    if let Some(text) = message["content"].as_str() {
        if !text.is_empty() {
            content.push(Block::text(text));
        }
    }
    for call in message["tool_calls"].as_array().into_iter().flatten() {
        let arguments = call["function"]["arguments"].as_str().unwrap_or("{}");
        let input =
            serde_json::from_str(arguments).unwrap_or_else(|_| json!({"_unparsed": arguments}));
        content.push(Block::ToolUse {
            id: call["id"].as_str().unwrap_or_default().to_string(),
            name: call["function"]["name"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            input,
            signature: None,
        });
    }
    Ok(Response {
        content,
        stop: openai_stop(choice["finish_reason"].as_str()),
        usage: openai_usage(&answer["usage"]),
        model: answer["model"].as_str().unwrap_or_default().to_string(),
    })
}

fn openai_stop(reason: Option<&str>) -> Stop {
    match reason {
        Some("stop") => Stop::EndTurn,
        Some("tool_calls") | Some("function_call") => Stop::ToolUse,
        Some("length") => Stop::MaxTokens,
        Some("content_filter") => Stop::Refusal,
        other => Stop::Other(other.unwrap_or("none").to_string()),
    }
}

/// A call of a streamed chat completion, as it is built.
#[derive(Default)]
struct Calling {
    id: String,
    name: String,
    arguments: String,
}

/// Reads a streamed chat completion (`"stream": true`) from its events,
/// telling `seen` its text and its calls as they come: the answer is what
/// [`openai_response`] reads from the same completion whole. The usage is
/// the last chunk's, when `stream_options.include_usage` asked for it. The
/// stream ends at `data: [DONE]`; one that ends without it is whole only if
/// it said why the model stopped.
pub fn openai_stream<R: Read>(
    events: &mut Events<R>,
    seen: &mut dyn FnMut(Delta) -> bool,
) -> Result<Response, ModelError> {
    let mut text = String::new();
    let mut calls: BTreeMap<u64, Calling> = BTreeMap::new();
    let mut finish: Option<String> = None;
    let mut usage = Usage::default();
    let mut model = String::new();
    let mut done = false;
    // Whether the watcher cut the answer off.
    let mut cut = false;
    while let Some(event) = events.next_event() {
        let event = event.map_err(ModelError::Transport)?;
        if event.data.trim() == "[DONE]" {
            done = true;
            break;
        }
        let chunk: Value = serde_json::from_str(&event.data)
            .map_err(|e| ModelError::Protocol(format!("a streamed chunk is not JSON: {e}")))?;
        if chunk["error"].is_object() {
            return Err(stream_error(&chunk["error"]));
        }
        if model.is_empty() {
            model = chunk["model"].as_str().unwrap_or_default().to_string();
        }
        if chunk["usage"].is_object() {
            usage = openai_usage(&chunk["usage"]);
        }
        for choice in chunk["choices"].as_array().into_iter().flatten() {
            if choice["index"].as_u64().unwrap_or(0) != 0 {
                continue;
            }
            let delta = &choice["delta"];
            if let Some(more) = delta["content"].as_str() {
                if !more.is_empty() {
                    text.push_str(more);
                    cut = cut || !seen(Delta::Text(more.to_string()));
                }
            }
            for call in delta["tool_calls"].as_array().into_iter().flatten() {
                let at = call["index"].as_u64().unwrap_or(0);
                let building = calls.entry(at).or_default();
                if let Some(id) = call["id"].as_str().filter(|id| !id.is_empty()) {
                    building.id = id.to_string();
                }
                if let Some(name) = call["function"]["name"].as_str().filter(|n| !n.is_empty()) {
                    if building.name.is_empty() {
                        cut = cut || !seen(Delta::Call(name.to_string()));
                    }
                    building.name.push_str(name);
                }
                if let Some(arguments) = call["function"]["arguments"].as_str() {
                    building.arguments.push_str(arguments);
                }
            }
            if let Some(reason) = choice["finish_reason"].as_str() {
                finish = Some(reason.to_string());
            }
        }
        // Cut off: nothing more is read, and the connection closes.
        if cut {
            break;
        }
    }
    if cut {
        return Ok(cut_off([text], usage, model));
    }
    if !done && finish.is_none() {
        return Err(ModelError::Transport(
            "the answer stopped before its end".into(),
        ));
    }
    let mut content = Vec::new();
    if !text.is_empty() {
        content.push(Block::text(text));
    }
    for call in calls.into_values() {
        content.push(Block::ToolUse {
            id: call.id,
            name: call.name,
            input: input_of(&call.arguments),
            signature: None,
        });
    }
    Ok(Response {
        content,
        stop: openai_stop(finish.as_deref()),
        usage,
        model,
    })
}

impl OpenAiCompatible {
    fn url(&self) -> String {
        format!("{}/chat/completions", self.base.trim_end_matches('/'))
    }

    fn bearer(key: Option<&str>) -> Option<String> {
        key.map(|k| format!("Bearer {k}"))
    }
}

impl Model for OpenAiCompatible {
    fn id(&self) -> &str {
        &self.model
    }

    fn complete(&self, request: &Request) -> Result<Response, ModelError> {
        let body = serde_json::to_vec(&self.body(request))
            .map_err(|e| ModelError::Protocol(e.to_string()))?;
        let answer = self.keys.send(|key| {
            let bearer = Self::bearer(key);
            let headers: Vec<(&str, &str)> = bearer
                .as_deref()
                .map(|b| vec![("Authorization", b)])
                .unwrap_or_default();
            self.client
                .post_json(&self.url(), &headers, &body, MAX_ANSWER)
                .map_err(http_error)
        })?;
        let value: Value = serde_json::from_slice(&answer.body)
            .map_err(|e| ModelError::Protocol(format!("answer is not JSON: {e}")))?;
        openai_response(&value)
    }

    /// Streams, asking for the usage in the last chunk; a server that
    /// refuses `stream_options` is asked again without it.
    fn stream(
        &self,
        request: &Request,
        seen: &mut dyn FnMut(Delta) -> bool,
    ) -> Result<Response, ModelError> {
        let mut body = self.body(request);
        body["stream"] = json!(true);
        body["stream_options"] = json!({"include_usage": true});
        let post = |body: &Value| {
            let bytes =
                serde_json::to_vec(body).map_err(|e| ModelError::Protocol(e.to_string()))?;
            self.keys.send(|key| {
                let bearer = Self::bearer(key);
                let headers: Vec<(&str, &str)> = bearer
                    .as_deref()
                    .map(|b| vec![("Authorization", b)])
                    .unwrap_or_default();
                post_stream(&self.client, &self.url(), &headers, &bytes)
            })
        };
        let mut response = match post(&body) {
            Err(ModelError::Refused(why)) if why.contains("stream_options") => {
                if let Some(fields) = body.as_object_mut() {
                    fields.remove("stream_options");
                }
                post(&body)?
            }
            answered => answered?,
        };
        if !streams(&response) {
            return whole(response, openai_response, seen);
        }
        openai_stream(&mut Events::new(response.body(), MAX_EVENT), seen)
    }
}

/// What Gemini is sent for a call it did not make itself (one another
/// model made, earlier in the conversation), which has no thought
/// signature of Gemini's: the value its documentation gives for that, which
/// its check of signatures lets through.
pub const GEMINI_NO_SIGNATURE: &str = "skip_thought_signature_validator";

/// Google's Gemini API (`generateContent`, streamed as Server-Sent Events
/// with `streamGenerateContent?alt=sse`), with an API key (`x-goog-api-key`).
/// Its thinking is not told, nor kept; the thought signature it gives with
/// a call is kept on the call and sent back with it, as its models since
/// Gemini 3 require within a turn.
pub struct Gemini {
    client: sion_http::Client,
    /// The API's base URL, `https://generativelanguage.googleapis.com` by
    /// default.
    pub base: String,
    /// The model, for example `gemini-3-pro-preview`.
    pub model: String,
    keys: Keys,
}

impl std::fmt::Debug for Gemini {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Gemini({} at {})", self.model, self.base)
    }
}

impl Gemini {
    /// A provider for `model` with the API key `key` (several, separated
    /// by commas, are used in turn: [`Keys`]).
    pub fn new(client: sion_http::Client, model: &str, key: &str) -> Gemini {
        Gemini {
            client,
            base: "https://generativelanguage.googleapis.com".into(),
            model: model.into(),
            keys: Keys::parse(key),
        }
    }

    fn url(&self, stream: bool) -> String {
        let base = self.base.trim_end_matches('/');
        if stream {
            format!(
                "{base}/v1beta/models/{}:streamGenerateContent?alt=sse",
                self.model
            )
        } else {
            format!("{base}/v1beta/models/{}:generateContent", self.model)
        }
    }

    /// The request body for `request`: the system prompt as the system
    /// instruction, each message a content (`user` or `model`), the tools
    /// as function declarations with their JSON Schemas.
    pub fn body(&self, request: &Request) -> Value {
        gemini_body(request)
    }
}

/// [`Gemini::body`].
pub fn gemini_body(request: &Request) -> Value {
    let mut pictures = Pictures::of(&request.messages, &request.images);
    // A function's response names its function: the calls' names by id.
    let names: BTreeMap<&str, &str> = request
        .messages
        .iter()
        .flat_map(|m| m.tool_uses())
        .map(|(id, name, _)| (id, name))
        .collect();
    let contents: Vec<Value> = request
        .messages
        .iter()
        .filter_map(|m| gemini_content(m, &names, &mut pictures))
        .collect();
    let mut body = json!({
        "contents": contents,
        "generationConfig": {"maxOutputTokens": request.max_tokens},
    });
    if !request.system.is_empty() {
        body["systemInstruction"] = json!({"parts": [{"text": request.system}]});
    }
    if !request.tools.is_empty() {
        let declarations: Vec<Value> = request
            .tools
            .iter()
            .map(|t| {
                json!({"name": t.name, "description": t.description,
                    "parametersJsonSchema": t.input_schema})
            })
            .collect();
        body["tools"] = json!([{ "functionDeclarations": declarations }]);
    }
    body
}

/// One message as a Gemini content, if it says anything.
fn gemini_content(
    message: &Message,
    names: &BTreeMap<&str, &str>,
    pictures: &mut Pictures<'_>,
) -> Option<Value> {
    let role = match message.role {
        Role::User => "user",
        Role::Assistant => "model",
    };
    // Calls another model made carry no signature of Gemini's: the first
    // of them is sent with the value that stands for one.
    let signed = message.content.iter().any(|b| {
        matches!(
            b,
            Block::ToolUse {
                signature: Some(_),
                ..
            }
        )
    });
    let mut stood_in = false;
    let mut parts = Vec::new();
    for block in &message.content {
        match block {
            Block::Text { text } if !text.is_empty() => parts.push(json!({ "text": text })),
            Block::Text { .. } => {}
            Block::ToolUse {
                id,
                name,
                input,
                signature,
            } => {
                let mut part = json!({"functionCall": {"id": id, "name": name, "args": input}});
                match signature {
                    Some(signature) => part["thoughtSignature"] = json!(signature),
                    None if !signed && !stood_in => {
                        part["thoughtSignature"] = json!(GEMINI_NO_SIGNATURE);
                        stood_in = true;
                    }
                    None => {}
                }
                parts.push(part);
            }
            Block::ToolResult {
                id,
                content,
                is_error,
                images,
            } => {
                let name = names.get(id.as_str()).copied().unwrap_or("tool");
                let response = if *is_error {
                    json!({ "error": content })
                } else {
                    json!({ "output": content })
                };
                parts.push(
                    json!({"functionResponse": {"id": id, "name": name, "response": response}}),
                );
                // The pictures it returned, after it.
                for image in images {
                    parts.push(match pictures.next(image) {
                        Some(data) => {
                            json!({"inlineData": {"mimeType": image.media_type, "data": data}})
                        }
                        None => json!({ "text": IMAGE_NOT_SHOWN }),
                    });
                }
            }
            Block::Image { image } => parts.push(match pictures.next(image) {
                Some(data) => json!({"inlineData": {"mimeType": image.media_type, "data": data}}),
                None => json!({ "text": IMAGE_NOT_SHOWN }),
            }),
        }
    }
    (!parts.is_empty()).then(|| json!({"role": role, "parts": parts}))
}

/// A call's id: Gemini's, or one made here, which names it uniquely in the
/// conversation.
fn gemini_call_id(call: &Value, at: usize) -> String {
    match call["id"].as_str().filter(|id| !id.is_empty()) {
        Some(id) => id.to_string(),
        None => {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            format!("gemini_{now:x}_{at}")
        }
    }
}

fn gemini_usage(meta: &Value) -> Usage {
    let count = |name: &str| meta[name].as_u64().unwrap_or(0);
    Usage {
        // The prompt's tokens, those of the cached content among them.
        input: count("promptTokenCount"),
        // Its thinking is paid for as output.
        output: count("candidatesTokenCount") + count("thoughtsTokenCount"),
        cache_read: count("cachedContentTokenCount"),
        cache_write: 0,
    }
}

/// What Anthropic's `usage` counts: its `input_tokens` leave out the
/// tokens read from the cache and those written to it, which are input
/// all the same.
fn anthropic_usage(usage: &Value) -> Usage {
    let count = |name: &str| usage[name].as_u64().unwrap_or(0);
    let cache_read = count("cache_read_input_tokens");
    let cache_write = count("cache_creation_input_tokens");
    Usage {
        input: count("input_tokens") + cache_read + cache_write,
        output: count("output_tokens"),
        cache_read,
        cache_write,
    }
}

/// What a Chat Completions `usage` counts: the prompt's tokens, those read
/// from the cache among them.
fn openai_usage(usage: &Value) -> Usage {
    Usage {
        input: usage["prompt_tokens"].as_u64().unwrap_or(0),
        output: usage["completion_tokens"].as_u64().unwrap_or(0),
        cache_read: usage["prompt_tokens_details"]["cached_tokens"]
            .as_u64()
            .unwrap_or(0),
        cache_write: 0,
    }
}

fn gemini_stop(reason: Option<&str>, called: bool) -> Stop {
    match reason {
        _ if called => Stop::ToolUse,
        Some("STOP") | None => Stop::EndTurn,
        Some("MAX_TOKENS") => Stop::MaxTokens,
        Some(
            "SAFETY" | "RECITATION" | "BLOCKLIST" | "PROHIBITED_CONTENT" | "SPII" | "IMAGE_SAFETY",
        ) => Stop::Refusal,
        Some(other) => Stop::Other(other.to_string()),
    }
}

/// An error Gemini reports in its answer (`{"error": {"code", "message",
/// "status"}}`), as the status it stands for.
fn gemini_error(error: &Value) -> ModelError {
    let status = error["status"].as_str().unwrap_or("UNKNOWN");
    let said = format!(
        "{status}: {}",
        error["message"].as_str().unwrap_or("no message")
    );
    match status {
        "RESOURCE_EXHAUSTED" | "UNAVAILABLE" | "INTERNAL" | "DEADLINE_EXCEEDED" => {
            ModelError::Busy(said)
        }
        _ => ModelError::Refused(said),
    }
}

/// Reads a whole `generateContent` answer: the text of its first
/// candidate, a block, then its calls, each with its signature; its
/// thinking left out.
pub fn gemini_response(answer: &Value) -> Result<Response, ModelError> {
    if answer["error"].is_object() {
        return Err(gemini_error(&answer["error"]));
    }
    let model = answer["modelVersion"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let usage = gemini_usage(&answer["usageMetadata"]);
    // A prompt Gemini blocked has no candidate.
    if answer["promptFeedback"]["blockReason"].is_string() {
        return Ok(Response {
            content: Vec::new(),
            stop: Stop::Refusal,
            usage,
            model,
        });
    }
    let candidate = &answer["candidates"][0];
    if !candidate.is_object() {
        return Err(ModelError::Protocol("the answer has no candidate".into()));
    }
    let mut text = String::new();
    let mut calls = Vec::new();
    for part in candidate["content"]["parts"]
        .as_array()
        .into_iter()
        .flatten()
    {
        gemini_part(part, &mut text, &mut calls);
    }
    let stop = gemini_stop(candidate["finishReason"].as_str(), !calls.is_empty());
    let mut content = Vec::new();
    if !text.is_empty() {
        content.push(Block::text(text));
    }
    content.extend(calls);
    Ok(Response {
        content,
        stop,
        usage,
        model,
    })
}

/// Takes one part of an answer: its text onto `text`, a call onto
/// `calls`; thinking left out. What it adds, for a stream to tell.
fn gemini_part(part: &Value, text: &mut String, calls: &mut Vec<Block>) -> Option<Delta> {
    if part["thought"] == true {
        return None;
    }
    if let Some(more) = part["text"].as_str().filter(|t| !t.is_empty()) {
        text.push_str(more);
        return Some(Delta::Text(more.to_string()));
    }
    let call = part.get("functionCall")?;
    let name = call["name"].as_str().unwrap_or_default().to_string();
    calls.push(Block::ToolUse {
        id: gemini_call_id(call, calls.len()),
        name: name.clone(),
        input: if call["args"].is_object() {
            call["args"].clone()
        } else {
            json!({})
        },
        signature: part["thoughtSignature"].as_str().map(str::to_string),
    });
    Some(Delta::Call(name))
}

/// Reads a streamed answer (`streamGenerateContent?alt=sse`) from its
/// events, telling `seen` its text and calls as they come: each event is a
/// part of the answer, and the answer is what [`gemini_response`] reads
/// from them all. It is whole once a candidate says why it finished.
pub fn gemini_stream<R: Read>(
    events: &mut Events<R>,
    seen: &mut dyn FnMut(Delta) -> bool,
) -> Result<Response, ModelError> {
    let mut text = String::new();
    let mut calls = Vec::new();
    let mut finish: Option<String> = None;
    let mut usage = Usage::default();
    let mut model = String::new();
    let mut cut = false;
    while let Some(event) = events.next_event() {
        let event = event.map_err(ModelError::Transport)?;
        let Ok(chunk) = serde_json::from_str::<Value>(&event.data) else {
            continue;
        };
        if chunk["error"].is_object() {
            return Err(gemini_error(&chunk["error"]));
        }
        if model.is_empty() {
            model = chunk["modelVersion"]
                .as_str()
                .unwrap_or_default()
                .to_string();
        }
        if chunk["usageMetadata"].is_object() {
            usage = gemini_usage(&chunk["usageMetadata"]);
        }
        if chunk["promptFeedback"]["blockReason"].is_string() {
            return Ok(Response {
                content: Vec::new(),
                stop: Stop::Refusal,
                usage,
                model,
            });
        }
        let candidate = &chunk["candidates"][0];
        for part in candidate["content"]["parts"]
            .as_array()
            .into_iter()
            .flatten()
        {
            if let Some(delta) = gemini_part(part, &mut text, &mut calls) {
                cut = cut || !seen(delta);
            }
        }
        if let Some(reason) = candidate["finishReason"].as_str() {
            finish = Some(reason.to_string());
        }
        // Cut off: nothing more is read, and the connection closes.
        if cut {
            break;
        }
    }
    if cut {
        return Ok(cut_off([text], usage, model));
    }
    if finish.is_none() {
        return Err(ModelError::Transport(
            "the answer stopped before its end".into(),
        ));
    }
    let stop = gemini_stop(finish.as_deref(), !calls.is_empty());
    let mut content = Vec::new();
    if !text.is_empty() {
        content.push(Block::text(text));
    }
    content.extend(calls);
    Ok(Response {
        content,
        stop,
        usage,
        model,
    })
}

impl Model for Gemini {
    fn id(&self) -> &str {
        &self.model
    }

    fn complete(&self, request: &Request) -> Result<Response, ModelError> {
        let body = serde_json::to_vec(&self.body(request))
            .map_err(|e| ModelError::Protocol(e.to_string()))?;
        let answer = self.keys.send(|key| {
            self.client
                .post_json(
                    &self.url(false),
                    &[("x-goog-api-key", key.unwrap_or_default())],
                    &body,
                    MAX_ANSWER,
                )
                .map_err(http_error)
        })?;
        let value: Value = serde_json::from_slice(&answer.body)
            .map_err(|e| ModelError::Protocol(format!("answer is not JSON: {e}")))?;
        gemini_response(&value)
    }

    fn stream(
        &self,
        request: &Request,
        seen: &mut dyn FnMut(Delta) -> bool,
    ) -> Result<Response, ModelError> {
        let body = serde_json::to_vec(&self.body(request))
            .map_err(|e| ModelError::Protocol(e.to_string()))?;
        let mut response = self.keys.send(|key| {
            post_stream(
                &self.client,
                &self.url(true),
                &[("x-goog-api-key", key.unwrap_or_default())],
                &body,
            )
        })?;
        if !streams(&response) {
            return whole(response, gemini_response, seen);
        }
        gemini_stream(&mut Events::new(response.body(), MAX_EVENT), seen)
    }
}

/// Where a provider's bearer comes from ([`OpenAiResponses`]).
pub enum Bearer {
    /// API keys, used in turn ([`Keys`]).
    Keys(Keys),
    /// A token a sign-in gave: `token(false)` the one to use, `token(true)`
    /// a new one, after the provider refused the last.
    Token(Box<dyn Fn(bool) -> Result<String, ModelError> + Send + Sync>),
}

impl std::fmt::Debug for Bearer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Bearer::Keys(keys) => write!(f, "Bearer::{keys:?}"),
            Bearer::Token(_) => write!(f, "Bearer::Token"),
        }
    }
}

/// OpenAI's Responses API (`POST {base}/responses`), streamed and not
/// stored (`"store": false`): each call carries the whole conversation, as
/// the chat completions do. Its reasoning is not kept between calls.
pub struct OpenAiResponses {
    client: sion_http::Client,
    /// The API's base URL, `https://api.openai.com/v1` by default.
    pub base: String,
    /// The model.
    pub model: String,
    bearer: Bearer,
}

impl std::fmt::Debug for OpenAiResponses {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "OpenAiResponses({} at {})", self.model, self.base)
    }
}

impl OpenAiResponses {
    /// A provider for `model`, its bearer from `bearer`.
    pub fn new(client: sion_http::Client, model: &str, bearer: Bearer) -> OpenAiResponses {
        OpenAiResponses {
            client,
            base: "https://api.openai.com/v1".into(),
            model: model.into(),
            bearer,
        }
    }

    /// The request body for `request` ([`responses_body`]).
    pub fn body(&self, request: &Request) -> Value {
        responses_body(&self.model, request)
    }

    fn url(&self) -> String {
        format!("{}/responses", self.base.trim_end_matches('/'))
    }

    /// Posts `body` with the bearer: keys in turn, or the token, a new one
    /// asked for once if the provider refuses it.
    fn post(&self, body: &[u8]) -> Result<sion_http::Response, ModelError> {
        let send = |token: &str| {
            let bearer = format!("Bearer {token}");
            post_stream(
                &self.client,
                &self.url(),
                &[("Authorization", bearer.as_str())],
                body,
            )
        };
        match &self.bearer {
            Bearer::Keys(keys) => keys.send(|key| send(key.unwrap_or_default())),
            Bearer::Token(token) => match send(&token(false)?) {
                Err(ModelError::Refused(why)) if why.starts_with("HTTP 401") => send(&token(true)?),
                answered => answered,
            },
        }
    }
}

/// The body of a Responses API request for `request`: the system prompt as
/// the instructions, each message an input item (the model's calls and
/// their outputs items of their own, the pictures results returned in a
/// user message after them), the tools as functions; streamed, not
/// stored.
pub fn responses_body(model: &str, request: &Request) -> Value {
    let mut pictures = Pictures::of(&request.messages, &request.images);
    let mut input = Vec::new();
    for message in &request.messages {
        match message.role {
            Role::User => {
                let mut parts = Vec::new();
                let mut shown = Vec::new();
                for block in &message.content {
                    match block {
                        Block::Text { text } if !text.is_empty() => {
                            parts.push(json!({"type": "input_text", "text": text}));
                        }
                        Block::ToolResult {
                            id,
                            content,
                            is_error,
                            images,
                        } => {
                            let output = if *is_error {
                                format!("error: {content}")
                            } else {
                                content.clone()
                            };
                            input.push(json!({"type": "function_call_output", "call_id": id,
                                "output": output}));
                            for image in images {
                                shown.push(match pictures.next(image) {
                                    Some(data) => json!({"type": "input_image",
                                        "image_url": format!("data:{};base64,{data}", image.media_type)}),
                                    None => json!({"type": "input_text", "text": IMAGE_NOT_SHOWN}),
                                });
                            }
                        }
                        Block::Image { image } => parts.push(match pictures.next(image) {
                            Some(data) => json!({"type": "input_image",
                                "image_url": format!("data:{};base64,{data}", image.media_type)}),
                            None => json!({"type": "input_text", "text": IMAGE_NOT_SHOWN}),
                        }),
                        _ => {}
                    }
                }
                parts.extend(shown);
                if !parts.is_empty() {
                    input.push(json!({"role": "user", "content": parts}));
                }
            }
            Role::Assistant => {
                for block in &message.content {
                    match block {
                        Block::Text { text } if !text.is_empty() => input.push(
                            json!({"role": "assistant", "content": [{"type": "output_text", "text": text}]}),
                        ),
                        Block::ToolUse {
                            id,
                            name,
                            input: arguments,
                            ..
                        } => input.push(json!({"type": "function_call", "call_id": id,
                            "name": name, "arguments": arguments.to_string()})),
                        _ => {}
                    }
                }
            }
        }
    }
    let mut body = json!({
        "model": model,
        "input": input,
        "max_output_tokens": request.max_tokens,
        "store": false,
        "stream": true,
    });
    if !request.system.is_empty() {
        body["instructions"] = json!(request.system);
    }
    if !request.tools.is_empty() {
        let tools: Vec<Value> = request
            .tools
            .iter()
            .map(|t| {
                json!({"type": "function", "name": t.name, "description": t.description,
                    "parameters": t.input_schema, "strict": false})
            })
            .collect();
        body["tools"] = json!(tools);
    }
    body
}

/// Reads a whole Responses API answer (a `response` object): its text, a
/// block, then its calls; its reasoning left out.
pub fn responses_response(answer: &Value) -> Result<Response, ModelError> {
    if answer["status"] == "failed" || answer["error"].is_object() {
        return Err(responses_error(&answer["error"]));
    }
    let mut text = String::new();
    let mut calls = Vec::new();
    let mut refused = false;
    for item in answer["output"].as_array().into_iter().flatten() {
        match item["type"].as_str() {
            Some("message") => {
                for part in item["content"].as_array().into_iter().flatten() {
                    match part["type"].as_str() {
                        Some("output_text") => text.push_str(part["text"].as_str().unwrap_or("")),
                        Some("refusal") => {
                            refused = true;
                            text.push_str(part["refusal"].as_str().unwrap_or(""));
                        }
                        _ => {}
                    }
                }
            }
            Some("function_call") => calls.push(Block::ToolUse {
                id: item["call_id"].as_str().unwrap_or_default().to_string(),
                name: item["name"].as_str().unwrap_or_default().to_string(),
                input: input_of(item["arguments"].as_str().unwrap_or_default()),
                signature: None,
            }),
            _ => {}
        }
    }
    let stop = if refused {
        Stop::Refusal
    } else if !calls.is_empty() {
        Stop::ToolUse
    } else {
        match (
            answer["status"].as_str(),
            answer["incomplete_details"]["reason"].as_str(),
        ) {
            (Some("incomplete"), Some("max_output_tokens")) => Stop::MaxTokens,
            (Some("incomplete"), Some("content_filter")) => Stop::Refusal,
            (Some("incomplete"), Some(other)) => Stop::Other(other.to_string()),
            _ => Stop::EndTurn,
        }
    };
    let mut content = Vec::new();
    if !text.is_empty() {
        content.push(Block::text(text));
    }
    content.extend(calls);
    Ok(Response {
        content,
        stop,
        usage: Usage {
            // Its input's tokens, those read from the cache among them.
            input: answer["usage"]["input_tokens"].as_u64().unwrap_or(0),
            output: answer["usage"]["output_tokens"].as_u64().unwrap_or(0),
            cache_read: answer["usage"]["input_tokens_details"]["cached_tokens"]
                .as_u64()
                .unwrap_or(0),
            cache_write: 0,
        },
        model: answer["model"].as_str().unwrap_or_default().to_string(),
    })
}

/// An error a Responses API answer reports: a limit or a failure on the
/// provider's side may pass (another model may answer meanwhile); the rest
/// does not.
fn responses_error(error: &Value) -> ModelError {
    let code = error["code"]
        .as_str()
        .or_else(|| error["type"].as_str())
        .unwrap_or("error");
    let said = format!(
        "{code}: {}",
        error["message"].as_str().unwrap_or("no message")
    );
    if code.contains("rate_limit")
        || code.contains("server_error")
        || code.contains("usage_limit")
        || code.contains("usage_unavailable")
        || code.contains("overloaded")
        || code.contains("timeout")
    {
        ModelError::Busy(said)
    } else {
        ModelError::Refused(said)
    }
}

/// Reads a streamed Responses API answer from its events, telling `seen`
/// the text as it comes and each call as it begins: the answer is the one
/// the terminal event carries (`response.completed`, or
/// `response.incomplete`), as [`responses_response`] reads it. One that
/// ends without a terminal event is a transport failure.
pub fn responses_stream<R: Read>(
    events: &mut Events<R>,
    seen: &mut dyn FnMut(Delta) -> bool,
) -> Result<Response, ModelError> {
    let mut text = String::new();
    let mut cut = false;
    let mut model = String::new();
    while let Some(event) = events.next_event() {
        let event = event.map_err(ModelError::Transport)?;
        let Ok(data) = serde_json::from_str::<Value>(&event.data) else {
            continue;
        };
        let kind = data["type"]
            .as_str()
            .unwrap_or(event.name.as_str())
            .to_string();
        match kind.as_str() {
            "response.created" | "response.in_progress" => {
                if model.is_empty() {
                    model = data["response"]["model"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string();
                }
            }
            "response.output_text.delta" => {
                let more = data["delta"].as_str().unwrap_or_default();
                if !more.is_empty() {
                    text.push_str(more);
                    cut = !seen(Delta::Text(more.to_string()));
                }
            }
            "response.output_item.added" if data["item"]["type"] == "function_call" => {
                let name = data["item"]["name"].as_str().unwrap_or_default();
                cut = !seen(Delta::Call(name.to_string()));
            }
            "response.completed" | "response.incomplete" => {
                return responses_response(&data["response"]);
            }
            "response.failed" => return Err(responses_error(&data["response"]["error"])),
            "error" => return Err(responses_error(&data)),
            _ => {}
        }
        // Cut off: nothing more is read, and the connection closes.
        if cut {
            return Ok(cut_off([text], Usage::default(), model));
        }
    }
    Err(ModelError::Transport(
        "the answer stopped before its end".into(),
    ))
}

impl Model for OpenAiResponses {
    fn id(&self) -> &str {
        &self.model
    }

    /// Streamed, whatever is asked: a sign-in's token is good for streamed
    /// requests only.
    fn complete(&self, request: &Request) -> Result<Response, ModelError> {
        self.stream(request, &mut |_| true)
    }

    fn stream(
        &self,
        request: &Request,
        seen: &mut dyn FnMut(Delta) -> bool,
    ) -> Result<Response, ModelError> {
        let body = serde_json::to_vec(&self.body(request))
            .map_err(|e| ModelError::Protocol(e.to_string()))?;
        let mut response = self.post(&body)?;
        if !streams(&response) {
            return whole(response, responses_response, seen);
        }
        responses_stream(&mut Events::new(response.body(), MAX_EVENT), seen)
    }
}
