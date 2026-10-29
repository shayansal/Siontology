//! The record of every call that changed something, so that nothing is done
//! twice by accident. Repeating an identical call returns the recorded
//! outcome; a call whose outcome is unknown is not repeated until someone
//! settles it.
//!
//! "Identical" means the same session, tool and input. To do the same thing
//! twice on purpose, a model changes the input (a reference, a purpose).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::tools::{Effect, Outcome};

/// One recorded call.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// The tool.
    pub tool: String,
    /// Its input.
    pub input: Value,
    /// Its effect.
    pub effect: Effect,
    /// What happened.
    pub outcome: Outcome,
    /// When (seconds).
    pub at: u64,
}

/// What the journal knows of a call about to be made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Prior {
    /// Never made: go ahead.
    New,
    /// Made, with a known outcome: return it instead of calling again.
    Done(Outcome),
    /// Made, with an unknown outcome: do not repeat it.
    Unknown,
}

/// Calls that changed something, by key.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Journal {
    entries: BTreeMap<String, Entry>,
}

/// A call's idempotency key: a digest of the session, the tool and the input
/// (`serde_json` writes object keys sorted, so equal inputs give equal
/// keys).
pub fn key(session: &str, tool: &str, input: &Value) -> String {
    let input = input.to_string();
    let digest = sion_core::hash::hash_parts(
        "sion-agent/call-key/v1",
        &[session.as_bytes(), tool.as_bytes(), input.as_bytes()],
    );
    digest.to_hex()[..32].to_string()
}

impl Journal {
    /// What is known of the call `key`.
    pub fn prior(&self, key: &str) -> Prior {
        match self.entries.get(key) {
            None => Prior::New,
            Some(entry) if entry.outcome.certain => Prior::Done(entry.outcome.clone()),
            Some(_) => Prior::Unknown,
        }
    }

    /// Records a call. Reads are not recorded, and neither is a failure that
    /// changed nothing: it may be tried again.
    pub fn record(&mut self, key: &str, entry: Entry) {
        let failed_cleanly = entry.outcome.is_error && entry.outcome.certain;
        if entry.effect > Effect::Read && !failed_cleanly {
            self.entries.insert(key.to_string(), entry);
        }
    }

    /// Settles a call whose outcome was unknown, once someone has checked
    /// what happened. Returns false if there was no such call.
    pub fn settle(&mut self, key: &str, outcome: Outcome) -> bool {
        match self.entries.get_mut(key) {
            Some(entry) if !entry.outcome.certain => {
                entry.outcome = outcome;
                true
            }
            _ => false,
        }
    }

    /// The calls whose outcome is unknown.
    pub fn unknown(&self) -> impl Iterator<Item = (&String, &Entry)> {
        self.entries.iter().filter(|(_, e)| !e.outcome.certain)
    }

    /// Every entry, by key.
    pub fn entries(&self) -> &BTreeMap<String, Entry> {
        &self.entries
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn entry(effect: Effect, outcome: Outcome) -> Entry {
        Entry {
            tool: "pay".into(),
            input: json!({"to": "bob", "amount": "1"}),
            effect,
            outcome,
            at: 0,
        }
    }

    #[test]
    fn keys_depend_on_session_tool_and_input_only() {
        let a = key("s1", "pay", &json!({"to": "bob", "amount": "1"}));
        assert_eq!(a, key("s1", "pay", &json!({"amount": "1", "to": "bob"})));
        assert_ne!(a, key("s2", "pay", &json!({"to": "bob", "amount": "1"})));
        assert_ne!(a, key("s1", "pay", &json!({"to": "bob", "amount": "2"})));
        assert_ne!(a, key("s1", "send", &json!({"to": "bob", "amount": "1"})));
        assert_eq!(a.len(), 32);
    }

    #[test]
    fn a_done_call_replays_and_an_unknown_one_is_held() {
        let mut journal = Journal::default();
        assert_eq!(journal.prior("k1"), Prior::New);
        journal.record("k1", entry(Effect::Spend, Outcome::ok("paid")));
        assert_eq!(journal.prior("k1"), Prior::Done(Outcome::ok("paid")));
        journal.record("k2", entry(Effect::Spend, Outcome::uncertain("timed out")));
        assert_eq!(journal.prior("k2"), Prior::Unknown);
        assert_eq!(journal.unknown().count(), 1);
        assert!(journal.settle("k2", Outcome::ok("paid, checked on the ledger")));
        assert!(matches!(journal.prior("k2"), Prior::Done(_)));
        assert!(!journal.settle("k1", Outcome::ok("again")));
    }

    #[test]
    fn reads_and_clean_failures_are_not_recorded() {
        let mut journal = Journal::default();
        journal.record("r", entry(Effect::Read, Outcome::ok("balance 5")));
        journal.record(
            "f",
            entry(Effect::Spend, Outcome::error("refused by the ledger")),
        );
        assert_eq!(journal.prior("r"), Prior::New);
        assert_eq!(journal.prior("f"), Prior::New);
    }
}
