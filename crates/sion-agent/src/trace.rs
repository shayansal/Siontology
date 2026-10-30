//! Sessions as trajectories: for review, evaluation and training.
//!
//! [`chat_line`] writes a session as one JSON line in the chat format with
//! tool calls that fine-tuning and reinforcement-learning tools read
//! (`{"messages": [...]}`), with the session's outcome alongside, so that
//! trajectories can be filtered by how they ended. [`digest`] commits to a
//! trajectory, so that it can be recorded where it cannot be rewritten, such
//! as the agent's memory log.

use serde_json::{json, Value};

use crate::model::Message;
use crate::providers::openai_messages;
use crate::session::{Ended, State};

/// The session as one JSON line: its messages in chat format, and metadata.
pub fn chat_line(state: &State, system: &str, ended: Option<&Ended>) -> String {
    let outcome = ended.map(|e| json!(e)).unwrap_or(Value::Null);
    let calls: usize = state.messages.iter().map(|m| m.tool_uses().count()).sum();
    json!({
        "messages": openai_messages(system, &state.messages, &state.images),
        "metadata": {
            "session": state.id,
            "outcome": outcome,
            "turns": state.turns_taken,
            "tool_calls": calls,
            "compactions": state.compactions,
            "usage": {"input_tokens": state.usage.input, "output_tokens": state.usage.output},
            "journal": state.journal.entries().len(),
        }
    })
    .to_string()
}

/// A commitment to a session's conversation (SHA3-256, domain-separated):
/// the same conversation always gives the same digest.
pub fn digest(state: &State) -> sion_core::hash::Hash32 {
    digest_of(&state.id, &state.messages)
}

/// The digest of the conversation `messages` of session `id`: a prefix of
/// a session's messages has the digest the session had when it held just
/// those.
pub fn digest_of(id: &str, messages: &[Message]) -> sion_core::hash::Hash32 {
    let body = serde_json::to_vec(messages).unwrap_or_default();
    sion_core::hash::hash_parts("sion-agent/trajectory/v1", &[id.as_bytes(), &body])
}
