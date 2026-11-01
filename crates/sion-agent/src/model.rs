//! What the runtime asks of a language model, independent of any provider.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Who said a message.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// The person or system the agent works for, and tool results.
    User,
    /// The model.
    Assistant,
}

/// One piece of a message.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Block {
    /// Text.
    Text {
        /// The text.
        text: String,
    },
    /// The model asks for a tool to run.
    ToolUse {
        /// The call's identifier, echoed by its result.
        id: String,
        /// The tool.
        name: String,
        /// Its arguments.
        input: Value,
        /// What the provider gave with the call for its next request to
        /// carry back, if anything (Gemini's thought signature), opaque.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    /// What a tool returned.
    ToolResult {
        /// The call this answers.
        id: String,
        /// The result, as text.
        content: String,
        /// Whether the call failed.
        is_error: bool,
        /// The pictures it returned (a screenshot), by their digests.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        images: Vec<Image>,
    },
    /// A picture the person sent, by its digest.
    Image {
        /// The picture.
        image: Image,
    },
}

/// A picture's bytes, as a person sent it or a tool returned it: its
/// media type, and the bytes in base64.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Picture {
    /// Its media type (`image/png`, `image/jpeg`, `image/gif`,
    /// `image/webp`).
    pub media_type: String,
    /// Its bytes, in base64.
    pub base64: String,
}

impl Picture {
    /// How the conversation names it.
    pub fn image(&self) -> Image {
        Image {
            media_type: self.media_type.clone(),
            digest: image_digest(&self.media_type, &self.base64),
        }
    }
}

/// What a picture counts for, in characters, when a conversation is
/// measured for compacting.
pub const PICTURE_CHARS: usize = 4_000;

/// The media types a picture may have, as models take them.
pub const PICTURE_TYPES: [&str; 4] = ["image/png", "image/jpeg", "image/gif", "image/webp"];

/// A picture in a conversation. The conversation names it by the digest of
/// its bytes, which are kept beside it ([`Request::images`]) while it is
/// among the last few: so that a picture let go changes nothing the
/// conversation's digest covers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Image {
    /// Its media type (`image/png`).
    pub media_type: String,
    /// The digest of its media type and bytes ([`image_digest`]), in hex.
    pub digest: String,
}

/// The most pictures a model is shown at once: the last ones of the
/// conversation. Earlier ones are a line that says so.
pub const MAX_IMAGES_SHOWN: usize = 3;

/// The digest a picture is named by: of its media type and its bytes in
/// base64.
pub fn image_digest(media_type: &str, base64: &str) -> String {
    sion_core::hash::hash_parts(
        "sion-agent/image/v1",
        &[media_type.as_bytes(), base64.as_bytes()],
    )
    .to_hex()
}

/// The pictures of `messages`, in order.
pub fn images_of(messages: &[Message]) -> Vec<&Image> {
    messages
        .iter()
        .flat_map(|m| m.content.iter())
        .flat_map(|b| match b {
            Block::ToolResult { images, .. } => images.as_slice(),
            Block::Image { image } => std::slice::from_ref(image),
            _ => &[],
        })
        .collect()
}

/// The most of the pictures the person sent last that a model is shown,
/// besides the last ones of the conversation: an album sent at once is
/// seen whole, however many screenshots follow it.
pub const MAX_PERSON_PICTURES: usize = 10;

/// The places, in [`images_of`], of the pictures the person sent last: those
/// of the last message that has any, the last [`MAX_PERSON_PICTURES`] of
/// them.
pub fn person_pictures(messages: &[Message]) -> Vec<usize> {
    let mut place = 0;
    let mut last = Vec::new();
    for message in messages {
        let mut theirs = Vec::new();
        for block in &message.content {
            match block {
                Block::ToolResult { images, .. } => place += images.len(),
                Block::Image { .. } => {
                    theirs.push(place);
                    place += 1;
                }
                _ => {}
            }
        }
        if !theirs.is_empty() {
            last = theirs;
        }
    }
    let skip = last.len().saturating_sub(MAX_PERSON_PICTURES);
    last.split_off(skip)
}

/// Which pictures of `messages`, by their place in [`images_of`], a model
/// is shown, of those whose bytes `kept` has: the last
/// [`MAX_IMAGES_SHOWN`], and the person's last ([`person_pictures`]).
pub fn images_shown(messages: &[Message], kept: &BTreeMap<String, String>) -> BTreeSet<usize> {
    let images = images_of(messages);
    let has = |at: &usize| {
        images
            .get(*at)
            .is_some_and(|image| kept.contains_key(&image.digest))
    };
    let mut shown: BTreeSet<usize> = (0..images.len())
        .rev()
        .filter(has)
        .take(MAX_IMAGES_SHOWN)
        .collect();
    shown.extend(person_pictures(messages).into_iter().filter(has));
    shown
}

impl Block {
    /// A text block.
    pub fn text(text: impl Into<String>) -> Block {
        Block::Text { text: text.into() }
    }
}

/// A message: a role and its blocks.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Message {
    /// Who said it.
    pub role: Role,
    /// What was said.
    pub content: Vec<Block>,
}

impl Message {
    /// A user message of plain text.
    pub fn user(text: impl Into<String>) -> Message {
        Message {
            role: Role::User,
            content: vec![Block::text(text)],
        }
    }

    /// The message's text blocks, joined.
    pub fn text(&self) -> String {
        self.content
            .iter()
            .filter_map(|b| match b {
                Block::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The tool calls in the message.
    pub fn tool_uses(&self) -> impl Iterator<Item = (&str, &str, &Value)> {
        self.content.iter().filter_map(|b| match b {
            Block::ToolUse {
                id, name, input, ..
            } => Some((id.as_str(), name.as_str(), input)),
            _ => None,
        })
    }

    /// Its length in characters, for budgeting the context.
    pub fn chars(&self) -> usize {
        self.content
            .iter()
            .map(|b| match b {
                Block::Text { text } => text.len(),
                Block::ToolUse { name, input, .. } => name.len() + input.to_string().len(),
                Block::ToolResult { content, .. } => content.len(),
                // A picture costs a model what some text would.
                Block::Image { .. } => PICTURE_CHARS,
            })
            .sum()
    }
}

/// A tool as the model sees it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolSpec {
    /// Its name (letters, digits, `_` and `-`).
    pub name: String,
    /// What it does and when to use it.
    pub description: String,
    /// A JSON Schema for its input.
    pub input_schema: Value,
}

/// One call to a model.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Request {
    /// The system prompt.
    pub system: String,
    /// The conversation so far.
    pub messages: Vec<Message>,
    /// The tools it may call.
    pub tools: Vec<ToolSpec>,
    /// The most tokens it may write.
    pub max_tokens: u32,
    /// The bytes (base64) of the conversation's pictures that are kept, by
    /// their digests.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub images: BTreeMap<String, String>,
}

/// Why a model stopped.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stop {
    /// It finished its turn.
    EndTurn,
    /// It asked for tools.
    ToolUse,
    /// It ran out of tokens.
    MaxTokens,
    /// It declined, for safety reasons.
    Refusal,
    /// The provider paused a long turn; send the conversation again to go on.
    PauseTurn,
    /// Its watcher cut it off while it wrote ([`Model::stream`]): the answer
    /// is the text it wrote so far, without its calls.
    Cut,
    /// Anything else the provider said.
    Other(String),
}

/// Tokens used by one call.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    /// Input tokens, those read from or written to the provider's cache
    /// among them.
    pub input: u64,
    /// Output tokens.
    pub output: u64,
    /// Of the input, the tokens the provider read from its prompt cache.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub cache_read: u64,
    /// Of the input, the tokens the provider wrote to its prompt cache.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub cache_write: u64,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

impl Usage {
    /// Adds what `other` used.
    pub fn add(&mut self, other: &Usage) {
        self.input += other.input;
        self.output += other.output;
        self.cache_read += other.cache_read;
        self.cache_write += other.cache_write;
    }

    /// The tokens used in all: input and output.
    pub fn total(&self) -> u64 {
        self.input + self.output
    }

    /// The input tokens read from the provider's cache and written to it,
    /// if it reported either.
    pub fn cached(&self) -> Option<(u64, u64)> {
        (self.cache_read > 0 || self.cache_write > 0).then_some((self.cache_read, self.cache_write))
    }
}

/// A model's answer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Response {
    /// What it said.
    pub content: Vec<Block>,
    /// Why it stopped.
    pub stop: Stop,
    /// Tokens used.
    pub usage: Usage,
    /// The model that answered.
    pub model: String,
}

/// What went wrong calling a model.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ModelError {
    /// Too many requests or overloaded: try later, or another model.
    #[error("busy: {0}")]
    Busy(String),
    /// The connection failed or timed out: the request may or may not have
    /// been processed (a model call has no side effects, so it can be
    /// repeated).
    #[error("transport: {0}")]
    Transport(String),
    /// The provider refused the request as malformed or unauthorized:
    /// repeating it will not help.
    #[error("refused: {0}")]
    Refused(String),
    /// The answer could not be understood.
    #[error("protocol: {0}")]
    Protocol(String),
}

impl ModelError {
    /// Whether another attempt, or another model, may succeed.
    pub fn transient(&self) -> bool {
        matches!(self, ModelError::Busy(_) | ModelError::Transport(_))
    }
}

/// What a model writes, as it writes it ([`Model::stream`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Delta {
    /// More of the text of its answer. The answer's text is what these say,
    /// in order: [`Message::text`] of the answer, its text blocks joined by
    /// line feeds.
    Text(String),
    /// It began a call of the tool named.
    Call(String),
}

/// Tells `seen` what `response` says, all at once: how a model that answers
/// whole streams. An answer that came whole is not cut off: what `seen`
/// returns is not asked.
pub fn tell_whole(response: &Response, seen: &mut dyn FnMut(Delta) -> bool) {
    let mut texts = 0;
    for block in &response.content {
        match block {
            Block::Text { text } => {
                let text = if texts == 0 {
                    text.clone()
                } else {
                    format!("\n{text}")
                };
                texts += 1;
                if !text.is_empty() {
                    seen(Delta::Text(text));
                }
            }
            Block::ToolUse { name, .. } => {
                seen(Delta::Call(name.clone()));
            }
            Block::ToolResult { .. } | Block::Image { .. } => {}
        }
    }
}

/// The answer of a model its watcher cut off ([`Stop::Cut`]): the text it
/// wrote so far, without its calls, which were not finished.
pub fn cut_off(texts: impl IntoIterator<Item = String>, usage: Usage, model: String) -> Response {
    Response {
        content: texts
            .into_iter()
            .filter(|text| !text.is_empty())
            .map(Block::text)
            .collect(),
        stop: Stop::Cut,
        usage,
        model,
    }
}

/// A language model.
pub trait Model: Send + Sync {
    /// Its identifier, for traces.
    fn id(&self) -> &str;

    /// One call.
    fn complete(&self, request: &Request) -> Result<Response, ModelError>;

    /// One call, telling `seen` what the model writes while it writes it;
    /// the answer is the one [`Model::complete`] gives. A model that cannot
    /// stream answers whole, and `seen` is told all of it at once. What
    /// `seen` was told of an answer that fails partway is not part of any
    /// answer. `seen` returns whether the model goes on: once it says no,
    /// the model stops writing, and its answer is what it wrote so far
    /// ([`cut_off`]).
    fn stream(
        &self,
        request: &Request,
        seen: &mut dyn FnMut(Delta) -> bool,
    ) -> Result<Response, ModelError> {
        let response = self.complete(request)?;
        tell_whole(&response, seen);
        Ok(response)
    }

    /// How far a model that replays a script has got: the steps it has
    /// used, so that a resumed session can go on from there. None for a
    /// model that does not replay anything.
    fn position(&self) -> Option<usize> {
        None
    }
}
