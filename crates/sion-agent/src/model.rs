//! What the runtime asks of a language model, independent of any provider.

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
    },
    /// What a tool returned.
    ToolResult {
        /// The call this answers.
        id: String,
        /// The result, as text.
        content: String,
        /// Whether the call failed.
        is_error: bool,
    },
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
            Block::ToolUse { id, name, input } => Some((id.as_str(), name.as_str(), input)),
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
    /// Anything else the provider said.
    Other(String),
}

/// Tokens used by one call.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    /// Input tokens.
    pub input: u64,
    /// Output tokens.
    pub output: u64,
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

/// A language model.
pub trait Model: Send + Sync {
    /// Its identifier, for traces.
    fn id(&self) -> &str;

    /// One call.
    fn complete(&self, request: &Request) -> Result<Response, ModelError>;
}
