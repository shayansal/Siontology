//! The record of every call that changed something, so that nothing is done
//! twice by accident. Repeating an identical call returns the recorded
//! outcome; a call whose outcome is unknown is not repeated until someone
//! settles it.
//!
//! "Identical" means the same session, tool and input. To do the same thing
//! twice on purpose, a model changes the input (a reference, a purpose).
//!
//! A call is written down before it runs ([`Journal::begin`]), as in
//! flight, with what it notes on the way ([`Journal::note`]): a session
//! saved then and resumed after a crash finds it unknown, not forgotten, so
//! it is not made again until it is settled, or reconciled by the tool from
//! its notes (see [`crate::tools::Dedup`]). What "exactly once" means here
//! is that: a completed call is never repeated, and a call that may have
//! taken effect is repeated only by a tool that keeps it from happening
//! twice.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::tools::{Dedup, Effect, Outcome};

/// What an in-flight call's entry says until its outcome is recorded.
pub const IN_FLIGHT: &str = "the run stopped while this call ran, before its outcome was \
     recorded: it may or may not have taken effect";

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
    /// What the call noted while it ran (a transaction's digest), from
    /// which its tool can tell what it did.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
    /// How its tool keeps it from happening twice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dedup: Option<Dedup>,
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

    /// Writes a call down as in flight, before it runs: until its outcome
    /// is recorded, it is unknown. Reads are not written down.
    pub fn begin(&mut self, key: &str, mut entry: Entry) {
        if entry.effect > Effect::Read {
            entry.outcome = Outcome::uncertain(IN_FLIGHT);
            self.entries.insert(key.to_string(), entry);
        }
    }

    /// Adds what an in-flight call noted.
    pub fn note(&mut self, key: &str, note: &str) {
        if let Some(entry) = self.entries.get_mut(key) {
            entry.notes.push(note.to_string());
        }
    }

    /// Records a call. Reads are not recorded, and neither is a failure that
    /// changed nothing: it may be tried again, so what [`Journal::begin`]
    /// wrote of it goes. The notes taken while it ran are kept.
    pub fn record(&mut self, key: &str, mut entry: Entry) {
        let failed_cleanly = entry.outcome.is_error && entry.outcome.certain;
        let begun = self.entries.remove(key);
        if entry.effect > Effect::Read && !failed_cleanly {
            if let Some(begun) = begun {
                let mut notes = begun.notes;
                notes.append(&mut entry.notes);
                entry.notes = notes;
            }
            self.entries.insert(key.to_string(), entry);
        }
    }

    /// Forgets a call found not to have taken effect, so that it may be
    /// made again. Returns false if there was no such call.
    pub fn forget(&mut self, key: &str) -> bool {
        self.entries.remove(key).is_some()
    }

    /// The entry of the call `key`, if any.
    pub fn entry(&self, key: &str) -> Option<&Entry> {
        self.entries.get(key)
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
            notes: Vec::new(),
            dedup: None,
        }
    }

    #[test]
    fn a_call_is_written_down_before_it_runs_and_its_notes_kept() {
        let mut journal = Journal::default();
        journal.begin("k1", entry(Effect::Spend, Outcome::ok("unused")));
        // Stopped here, a resumed session finds the call unknown.
        assert_eq!(journal.prior("k1"), Prior::Unknown);
        journal.note("k1", "transaction ab12 may execute until Pulse 9");
        journal.record("k1", entry(Effect::Spend, Outcome::ok("paid")));
        assert_eq!(journal.prior("k1"), Prior::Done(Outcome::ok("paid")));
        assert_eq!(
            journal.entry("k1").unwrap().notes,
            ["transaction ab12 may execute until Pulse 9"]
        );
        // A clean failure is not kept: it may be tried again.
        journal.begin("k2", entry(Effect::Spend, Outcome::ok("unused")));
        journal.record("k2", entry(Effect::Spend, Outcome::error("refused")));
        assert_eq!(journal.prior("k2"), Prior::New);
        // Reads are not written down.
        journal.begin("k3", entry(Effect::Read, Outcome::ok("unused")));
        assert_eq!(journal.prior("k3"), Prior::New);
        // A call found not to have happened is forgotten.
        journal.begin("k4", entry(Effect::Spend, Outcome::ok("unused")));
        assert!(journal.forget("k4"));
        assert_eq!(journal.prior("k4"), Prior::New);
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
