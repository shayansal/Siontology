//! Consolidating an agent's memory log (`sion memory consolidate`): a model
//! reads the episodes recorded since the last consolidation and the notes
//! the agent keeps, and proposes notes worth keeping for good, which
//! [`sion_agent::consolidate`]'s gates check.
//!
//! What is kept is recorded in the log, never in place of what was there:
//!
//! - each note is a **derived** atom (a fact, a procedure or a
//!   preference) with a `derived_from` link to every episode it cites, a
//!   `supersedes` link to every note it replaces, and an extraction trace
//!   naming the model and the consolidator;
//! - each note it replaces gets a **revision** to `superseded`, so recall,
//!   search and the agent's prompt leave it out while the log keeps it;
//! - a **summary** atom records how far the episodes were consolidated
//!   (`memory consolidated-through <seq>`), so the next run starts after
//!   it.
//!
//! A note derived from an episode of a session that read content from
//! outside is marked as a prompt-injection risk and unsafe for planning,
//! like that episode: it is kept, and recalled, but never put into the
//! agent's prompt as something to keep in mind. The log is then committed,
//! so the ledger holds the consolidation with the rest.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value as Json};
use sion_agent::consolidate::{self, Material, Shown};
use sion_agent::Limits;
use sion_core::hash::hash_domain;
use sion_core::ids::ObjectId;
use sion_memory::{
    AiUsage, AtomClass, BeliefState, Interval, LinkKind, LogStore, MemoryAtom, MemoryAtomId,
    MemoryKind, Statement, Taint, Value,
};

use crate::error::CliError;
use crate::ledger::Ledger;
use crate::memory::{committed, memory_error, open_store};
use crate::session::Session;

/// The most episodes one run is shown (the oldest not consolidated yet);
/// a later run takes the rest.
pub const MAX_EPISODES: usize = 40;

/// The most notes one run is shown (the newest).
pub const MAX_NOTES_SHOWN: usize = 80;

/// The subject and relation of the summary atom that records how far a
/// log's episodes were consolidated.
pub const MARKER: (&str, &str) = ("memory", "consolidated-through");

/// The kinds a note may be, by the names the consolidator uses.
pub const NOTE_KINDS: [(&str, MemoryKind); 3] = [
    ("fact", MemoryKind::Semantic),
    ("procedure", MemoryKind::Procedural),
    ("preference", MemoryKind::Preference),
];

fn nanos() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// A value as a note shows it: text as it is, anything else as the memory
/// commands write it.
pub fn plain(value: &Value) -> String {
    match value {
        Value::Text(text) => text.clone(),
        other => crate::memory::value_text(other),
    }
}

/// Whether an atom quotes content from outside the agent's trust.
pub fn untrusted(atom: &MemoryAtom) -> bool {
    matches!(
        atom.policy.taint,
        Taint::PromptInjectionRisk | Taint::Poisoned
    )
}

/// Where a log's consolidation stands: the last episode consolidated, if
/// any.
pub fn consolidated_through(store: &LogStore) -> Option<u64> {
    store
        .log()
        .atoms()
        .iter()
        .filter_map(|atom| match &atom.statement {
            Statement::Claim {
                subject,
                predicate,
                object: Value::Integer(seq),
            } if subject == MARKER.0 && predicate == MARKER.1 => u64::try_from(*seq).ok(),
            _ => None,
        })
        .max()
}

/// What a run is shown, and the atoms behind each id it may cite.
#[derive(Debug)]
pub struct Gathered {
    /// The episodes and notes.
    pub material: Material,
    /// Each id's atom: its id, its place in the log, and whether it is
    /// untrusted.
    pub atoms: BTreeMap<String, (MemoryAtomId, u64, bool)>,
    /// Episodes not consolidated yet beyond those shown.
    pub left: usize,
}

/// Gathers what a run on `store` is shown: the oldest episodes after the
/// last consolidation, and the newest notes still held.
pub fn gather(store: &LogStore) -> Gathered {
    let gone = crate::agent_memory::withdrawn(store);
    let after = consolidated_through(store);
    let mut episodes = Vec::new();
    let mut notes = Vec::new();
    let mut atoms = BTreeMap::new();
    for atom in store.log().atoms() {
        if gone.contains(&atom.id()) {
            continue;
        }
        let Statement::Claim {
            subject,
            predicate,
            object,
        } = &atom.statement
        else {
            continue;
        };
        if atom.kind == MemoryKind::Episodic && predicate == "episode" {
            if after.is_some_and(|through| atom.seq <= through) {
                continue;
            }
            episodes.push((atom, plain(object)));
        } else if NOTE_KINDS.iter().any(|(_, kind)| *kind == atom.kind) {
            notes.push((
                atom,
                consolidate::note_text(subject, predicate, &plain(object)),
            ));
        }
    }
    let left = episodes.len().saturating_sub(MAX_EPISODES);
    episodes.truncate(MAX_EPISODES);
    let skip = notes.len().saturating_sub(MAX_NOTES_SHOWN);
    let mut material = Material::default();
    for (atom, text) in episodes {
        let id = format!("e{}", atom.seq);
        atoms.insert(id.clone(), (atom.id(), atom.seq, untrusted(atom)));
        material.episodes.push(Shown {
            id,
            text,
            untrusted: untrusted(atom),
        });
    }
    for (atom, text) in notes.into_iter().skip(skip) {
        let id = format!("n{}", atom.seq);
        atoms.insert(id.clone(), (atom.id(), atom.seq, untrusted(atom)));
        material.notes.push(Shown {
            id,
            text,
            untrusted: untrusted(atom),
        });
    }
    Gathered {
        material,
        atoms,
        left,
    }
}

fn clip(text: &str, bytes: usize) -> String {
    if text.len() <= bytes {
        return text.to_string();
    }
    let mut end = bytes.saturating_sub(3);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

/// Consolidates `log`, written by `wallet`, with the model `model` (a
/// model spec, as for `sion agent run`); records what the gates keep and
/// commits the log.
pub fn run(
    session: &mut Session<'_>,
    ledger: &dyn Ledger,
    wallet: &str,
    log: &ObjectId,
    model: &str,
    limits: &Limits,
) -> Result<Json, CliError> {
    let writer = session.id(wallet);
    let mut store = open_store(log, writer)?;
    let gathered = gather(&store);
    if gathered.material.episodes.is_empty() {
        return Ok(json!({
            "log": log,
            "consolidated": 0,
            "note": match consolidated_through(&store) {
                Some(seq) => format!("nothing to consolidate: no episode after atom {seq}"),
                None => "nothing to consolidate: the log holds no episode".to_string(),
            },
        }));
    }
    let consolidator = crate::runtime::model(model, 0)?;
    let stopped = || crate::runtime::kill_switch().exists();
    let outcome =
        consolidate::consolidate(consolidator.as_ref(), &gathered.material, limits, &stopped);
    match &outcome.ended {
        Some(sion_agent::Ended::ModelsFailed { error }) => {
            return Err(CliError::Usage(format!(
                "the consolidator could not answer: {error}; nothing was recorded"
            )))
        }
        Some(sion_agent::Ended::Stopped) => {
            return Err(CliError::Usage(
                "the kill switch is on: nothing was recorded".into(),
            ))
        }
        _ => {}
    }
    let model_digest = hash_domain("sion-agent/model-id/v1", model.as_bytes());
    let code_digest = consolidate::code_digest();
    let now = nanos();
    let mut kept = Vec::new();
    let mut superseded: BTreeSet<MemoryAtomId> = BTreeSet::new();
    for note in &outcome.notes {
        let kind = NOTE_KINDS
            .iter()
            .find(|(name, _)| *name == note.kind)
            .map(|(_, kind)| *kind)
            .expect("the gates keep only known kinds");
        let sources: Vec<(MemoryAtomId, u64, bool)> = note
            .sources
            .iter()
            .filter_map(|id| gathered.atoms.get(id).copied())
            .collect();
        let replaces: Vec<(MemoryAtomId, u64, bool)> = note
            .replaces
            .iter()
            .filter_map(|id| gathered.atoms.get(id).copied())
            .filter(|(id, _, _)| !superseded.contains(id))
            .collect();
        let tainted = sources.iter().any(|(_, _, untrusted)| *untrusted);
        let mut builder = MemoryAtom::builder(
            writer,
            store.log().len(),
            kind,
            AtomClass::Derived,
            Statement::claim(
                note.subject.clone(),
                note.relation.clone(),
                Value::Text(note.value.clone()),
            ),
        )
        .valid(Interval::starting(now))
        .observed_at(now)
        .extraction(model_digest, code_digest);
        for (id, _, _) in &sources {
            builder = builder.link(LinkKind::DerivedFrom, *id, 1000);
        }
        for (id, _, _) in &replaces {
            builder = builder.link(LinkKind::Supersedes, *id, 1000);
        }
        if tainted {
            builder = builder
                .taint(Taint::PromptInjectionRisk)
                .ai_usage(AiUsage::UnsafeForPlanning);
        }
        let atom = builder
            .build()
            .map_err(|e| CliError::Usage(format!("note {:?}: {e}", note.subject)))?;
        let note_seq = store.append(atom).map_err(memory_error)?;
        let mut revision = None;
        if !replaces.is_empty() {
            let reason = clip(
                &format!("superseded by atom {note_seq}: {}", note.why),
                sion_memory::atom::MAX_REASON_BYTES,
            );
            let atom = MemoryAtom::builder(
                writer,
                store.log().len(),
                MemoryKind::Correction,
                AtomClass::Revision,
                Statement::revision(
                    replaces.iter().map(|(id, _, _)| *id),
                    BeliefState::Superseded,
                    reason,
                ),
            )
            .valid(Interval::starting(now))
            .observed_at(now)
            .build()
            .map_err(|e| CliError::Usage(e.to_string()))?;
            revision = Some(store.append(atom).map_err(memory_error)?);
            superseded.extend(replaces.iter().map(|(id, _, _)| *id));
        }
        kept.push(json!({
            "atom": note_seq,
            "kind": note.kind,
            "note": consolidate::note_text(&note.subject, &note.relation, &note.value),
            "from": sources.iter().map(|(_, seq, _)| format!("atom {seq}")).collect::<Vec<_>>(),
            "replaces": replaces.iter().map(|(_, seq, _)| format!("atom {seq}")).collect::<Vec<_>>(),
            "revision": revision,
            "untrusted": tainted,
            "why": note.why,
        }));
    }
    // How far the episodes were consolidated: the last one shown.
    let shown: Vec<(MemoryAtomId, u64)> = gathered
        .material
        .episodes
        .iter()
        .filter_map(|e| gathered.atoms.get(&e.id).map(|(id, seq, _)| (*id, *seq)))
        .collect();
    let through = shown.iter().map(|(_, seq)| *seq).max().unwrap_or(0);
    let mut marker = MemoryAtom::builder(
        writer,
        store.log().len(),
        MemoryKind::Reflection,
        AtomClass::Summary,
        Statement::claim(MARKER.0, MARKER.1, Value::Integer(i128::from(through))),
    )
    .valid(Interval::starting(now))
    .observed_at(now)
    .extraction(model_digest, code_digest);
    for (id, _) in &shown {
        marker = marker.link(LinkKind::DerivedFrom, *id, 1000);
    }
    let marker = marker.build().map_err(|e| CliError::Usage(e.to_string()))?;
    let marker_seq = store.append(marker).map_err(memory_error)?;
    drop(store);
    let view = crate::memory::commit(session, ledger, wallet, log, false)?;
    let on_ledger = committed(ledger, log)?;
    let rejected: Vec<Json> = outcome
        .rejected
        .iter()
        .map(|r| json!({"proposal": r.proposal, "reason": r.reason}))
        .collect();
    Ok(json!({
        "log": log,
        "model": model,
        "episodes": shown.len(),
        "episodes_left": gathered.left,
        "notes_shown": gathered.material.notes.len(),
        "kept": kept,
        "turned_down": rejected,
        "consolidated_through": through,
        "marker": marker_seq,
        "committed": format!(
            "Pulse {}: {}",
            view.pulse.0,
            crate::describe::receipt(&session.names(), &view.effects)
        ),
        "size_on_ledger": on_ledger.size,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_read_plainly() {
        assert_eq!(plain(&Value::Text("net 30".into())), "net 30");
        assert_eq!(plain(&Value::Integer(42)), "42");
        assert_eq!(clip("abcdef", 10), "abcdef");
        assert_eq!(clip("abcdefghijkl", 8), "abcde…");
        assert_eq!(clip("ééééé", 6), "é…");
    }
}
