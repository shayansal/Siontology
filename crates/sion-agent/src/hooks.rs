//! The operator's own checks around tool calls. Before a call runs (once
//! the policy lets it, and any approval is given) a hook may block it, and
//! the model is told why; after it ran, a hook may add a note to what the
//! model is shown of it. What a hook is (a command, a function) is the
//! host's: the runtime asks [`ToolHooks`].

use serde_json::Value;

use crate::tools::Outcome;

/// What the operator's hooks say of tool calls.
pub trait ToolHooks: Sync {
    /// Before `tool` runs with `input` in session `session`: why it may not
    /// run, if it may not.
    fn before(&self, session: &str, tool: &str, input: &Value) -> Option<String>;

    /// After `tool` ran with `input` and gave `outcome`: a note to add to
    /// what the model is shown, if any.
    fn after(&self, session: &str, tool: &str, input: &Value, outcome: &Outcome) -> Option<String>;
}

/// What the model is shown of a call a hook blocked.
pub fn blocked(why: &str) -> Outcome {
    Outcome::error(format!("blocked by the operator's hook: {}", why.trim()))
}

/// `outcome` with a hook's `note` after what it says.
pub fn noted(mut outcome: Outcome, note: &str) -> Outcome {
    let note = note.trim();
    if !note.is_empty() {
        if !outcome.content.is_empty() {
            outcome.content.push('\n');
        }
        outcome.content.push_str("[the operator's hook adds] ");
        outcome.content.push_str(note);
    }
    outcome
}
