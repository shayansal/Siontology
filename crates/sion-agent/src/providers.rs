//! Model providers over HTTPS, through `sion-http` (bounded, guarded, with
//! the hybrid post-quantum key exchange first): the Anthropic Messages API
//! and any OpenAI-compatible chat completions API.

use serde_json::{json, Value};

use std::collections::{BTreeMap, BTreeSet};

use crate::model::{
    images_shown, Block, Image, Message, Model, ModelError, Request, Response, Role, Stop, Usage,
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

/// The Anthropic Messages API (`POST /v1/messages`, API version
/// `2026-10-21`: no sampling parameters are sent, and the top-level
/// `cache_control` asks for automatic prompt caching).
pub struct Anthropic {
    client: sion_http::Client,
    /// The API's base URL, `https://api.anthropic.com` by default.
    pub base: String,
    /// The model, for example `claude-opus-5-5`.
    pub model: String,
    key: String,
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
    /// A provider for `model` with the API key `key`.
    pub fn new(client: sion_http::Client, model: &str, key: &str) -> Anthropic {
        Anthropic {
            client,
            base: "https://api.anthropic.com".into(),
            model: model.into(),
            key: key.into(),
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
            Block::ToolUse { id, name, input } => {
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
            }),
            // Thinking and other blocks are not part of the conversation
            // this runtime keeps.
            _ => {}
        }
    }
    let stop = match answer["stop_reason"].as_str() {
        Some("end_turn") | Some("stop_sequence") => Stop::EndTurn,
        Some("tool_use") => Stop::ToolUse,
        Some("max_tokens") => Stop::MaxTokens,
        Some("refusal") => Stop::Refusal,
        Some("pause_turn") => Stop::PauseTurn,
        other => Stop::Other(other.unwrap_or("none").to_string()),
    };
    Ok(Response {
        content,
        stop,
        usage: Usage {
            input: answer["usage"]["input_tokens"].as_u64().unwrap_or(0),
            output: answer["usage"]["output_tokens"].as_u64().unwrap_or(0),
        },
        model: answer["model"].as_str().unwrap_or_default().to_string(),
    })
}

impl Model for Anthropic {
    fn id(&self) -> &str {
        &self.model
    }

    fn complete(&self, request: &Request) -> Result<Response, ModelError> {
        let body = serde_json::to_vec(&self.body(request))
            .map_err(|e| ModelError::Protocol(e.to_string()))?;
        let url = format!("{}/v1/messages", self.base.trim_end_matches('/'));
        let answer = self
            .client
            .post_json(
                &url,
                &[
                    ("x-api-key", &self.key),
                    ("anthropic-version", &self.version),
                ],
                &body,
                MAX_ANSWER,
            )
            .map_err(http_error)?;
        let value: Value = serde_json::from_slice(&answer.body)
            .map_err(|e| ModelError::Protocol(format!("answer is not JSON: {e}")))?;
        anthropic_response(&value)
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
    key: Option<String>,
}

impl std::fmt::Debug for OpenAiCompatible {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "OpenAiCompatible({} at {})", self.model, self.base)
    }
}

impl OpenAiCompatible {
    /// A provider at `base` for `model`, with an optional bearer key.
    pub fn new(client: sion_http::Client, base: &str, model: &str, key: Option<&str>) -> Self {
        OpenAiCompatible {
            client,
            base: base.into(),
            model: model.into(),
            key: key.map(str::to_string),
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
                for block in &message.content {
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
                if !text.is_empty() {
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
        });
    }
    let stop = match choice["finish_reason"].as_str() {
        Some("stop") => Stop::EndTurn,
        Some("tool_calls") => Stop::ToolUse,
        Some("length") => Stop::MaxTokens,
        Some("content_filter") => Stop::Refusal,
        other => Stop::Other(other.unwrap_or("none").to_string()),
    };
    Ok(Response {
        content,
        stop,
        usage: Usage {
            input: answer["usage"]["prompt_tokens"].as_u64().unwrap_or(0),
            output: answer["usage"]["completion_tokens"].as_u64().unwrap_or(0),
        },
        model: answer["model"].as_str().unwrap_or_default().to_string(),
    })
}

impl Model for OpenAiCompatible {
    fn id(&self) -> &str {
        &self.model
    }

    fn complete(&self, request: &Request) -> Result<Response, ModelError> {
        let body = serde_json::to_vec(&self.body(request))
            .map_err(|e| ModelError::Protocol(e.to_string()))?;
        let url = format!("{}/chat/completions", self.base.trim_end_matches('/'));
        let bearer = self.key.as_ref().map(|k| format!("Bearer {k}"));
        let headers: Vec<(&str, &str)> = bearer
            .as_deref()
            .map(|b| vec![("Authorization", b)])
            .unwrap_or_default();
        let answer = self
            .client
            .post_json(&url, &headers, &body, MAX_ANSWER)
            .map_err(http_error)?;
        let value: Value = serde_json::from_slice(&answer.body)
            .map_err(|e| ModelError::Protocol(format!("answer is not JSON: {e}")))?;
        openai_response(&value)
    }
}
