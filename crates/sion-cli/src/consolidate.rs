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
//!
//! Dreaming (`sion memory dream`) reads the sessions themselves, not their
//! episodes: each by a reader of its own ([`sion_agent::dream`]), each
//! note it proposes kept only if its session shows it word for word. What
//! is left goes through the same gates and is recorded the same way,
//! citing its session's episode, and a summary atom marks how far the
//! dream went (`memory dreamt-through <seq>`).

use std::collections::{BTreeMap, BTreeSet};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value as Json};
use sion_agent::consolidate::{self, Material, Shown};
use sion_agent::Limits;
use sion_core::hash::{hash_domain, Hash32};
use sion_core::ids::{AccountId, ObjectId};
use sion_memory::{
    AiUsage, AtomClass, BeliefState, Interval, LinkKind, LogStore, MemoryAtom, MemoryAtomId,
    MemoryKind, Statement, Taint, Value,
};

use crate::error::CliError;
use crate::ledger::Ledger;
use crate::memory::{committed, memory_error, open_store};
use crate::session::Session;

/// What a dream marks how far it went: `memory dreamt-through <seq>`.
pub const DREAMT: (&str, &str) = ("memory", "dreamt-through");

/// The most sessions one dream reads: those of the oldest episodes not
/// dreamt over yet.
pub const MAX_DREAMT: usize = 12;

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
    marked_through(store, MARKER)
}

/// Where a log's dreaming stands: the last episode whose session a dream
/// read, or found was not on this machine, if any.
pub fn dreamt_through(store: &LogStore) -> Option<u64> {
    marked_through(store, DREAMT)
}

/// The last episode a summary atom of `marker` says was gone through.
fn marked_through(store: &LogStore, marker: (&str, &str)) -> Option<u64> {
    store
        .log()
        .atoms()
        .iter()
        .filter_map(|atom| match &atom.statement {
            Statement::Claim {
                subject,
                predicate,
                object: Value::Integer(seq),
            } if subject == marker.0 && predicate == marker.1 => u64::try_from(*seq).ok(),
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
    let kept = record_notes(
        &mut store,
        writer,
        &outcome.notes,
        &gathered.atoms,
        (model_digest, code_digest),
        now,
    )?;
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

/// Dreams over `log`, written by `wallet`, with `model` (a model spec, as
/// for `sion agent run`): the sessions of the episodes recorded since the
/// last dream, those on this machine, each read whole by a reader of its
/// own ([`sion_agent::dream`]). A note a reader proposes is kept only if
/// its session shows it word for word (a session that read content from
/// outside, facts only), then goes through consolidation's gates, and is
/// recorded as consolidation records it, citing its session's episode; a
/// summary atom marks how far the dream went (`memory dreamt-through
/// <seq>`), and the log is committed.
pub fn dream(
    session: &mut Session<'_>,
    ledger: &dyn Ledger,
    wallet: &str,
    log: &ObjectId,
    model: &str,
) -> Result<Json, CliError> {
    let writer = session.id(wallet);
    let mut store = open_store(log, writer)?;
    let gone = crate::agent_memory::withdrawn(&store);
    let after = dreamt_through(&store);
    // The notes kept, which a reader is shown and the gates check against.
    let gathered = gather(&store);
    let mut material = Material {
        episodes: Vec::new(),
        notes: gathered.material.notes.clone(),
    };
    let mut atoms: BTreeMap<String, (MemoryAtomId, u64, bool)> = gathered
        .atoms
        .iter()
        .filter(|(id, _)| id.starts_with('n'))
        .map(|(id, atom)| (id.clone(), *atom))
        .collect();
    let mut transcripts = Vec::new();
    let mut elsewhere = Vec::new();
    let mut through = None;
    let mut episodes = Vec::new();
    for atom in store.log().atoms() {
        let Statement::Claim {
            subject,
            predicate,
            object,
        } = &atom.statement
        else {
            continue;
        };
        if gone.contains(&atom.id())
            || atom.kind != MemoryKind::Episodic
            || predicate != "episode"
            || after.is_some_and(|seen| atom.seq <= seen)
        {
            continue;
        }
        let Some(id) = subject.strip_prefix("session-") else {
            continue;
        };
        if transcripts.len() + elsewhere.len() == MAX_DREAMT {
            break;
        }
        through = Some(atom.seq);
        let Ok(saved) = crate::runtime::Saved::load(id) else {
            elsewhere.push(id.to_string());
            continue;
        };
        let episode = format!("e{}", atom.seq);
        let tainted = untrusted(atom) || saved.state.standing.tainted_until.is_some();
        atoms.insert(episode.clone(), (atom.id(), atom.seq, tainted));
        episodes.push(atom.id());
        material.episodes.push(Shown {
            id: episode.clone(),
            text: plain(object),
            untrusted: tainted,
        });
        transcripts.push(sion_agent::dream::Transcript {
            session: id.to_string(),
            episode,
            tainted,
            text: crate::runtime::transcript(&saved.state.messages).join("\n"),
        });
    }
    let Some(through) = through else {
        return Ok(json!({
            "log": log,
            "dreamt": 0,
            "note": match after {
                Some(seq) => format!("nothing to dream over: no episode after atom {seq}"),
                None => "nothing to dream over: the log holds no episode".to_string(),
            },
        }));
    };
    // A reader of its own for each session: a scripted one plays its
    // script for that session alone.
    let readers = transcripts
        .iter()
        .map(|_| crate::runtime::model(model, 0))
        .collect::<Result<Vec<_>, _>>()?;
    let readers: Vec<&dyn sion_agent::model::Model> =
        readers.iter().map(|reader| reader.as_ref()).collect();
    let dreamt = sion_agent::dream::dream_with(&readers, &transcripts, &material.notes);
    if dreamt.read == 0 && !transcripts.is_empty() {
        return Err(CliError::Usage(format!(
            "no reader could answer ({}); nothing was recorded",
            dreamt
                .unread
                .iter()
                .map(|(session, why)| format!("session {session}: {why}"))
                .collect::<Vec<_>>()
                .join("; ")
        )));
    }
    let proposals = dreamt.kept.iter().map(|c| c.proposal()).collect();
    let outcome = consolidate::gate(&material, proposals);
    let extraction = (
        hash_domain("sion-agent/model-id/v1", model.as_bytes()),
        sion_agent::dream::code_digest(),
    );
    let now = nanos();
    let kept = record_notes(&mut store, writer, &outcome.notes, &atoms, extraction, now)?;
    let mut marker = MemoryAtom::builder(
        writer,
        store.log().len(),
        MemoryKind::Reflection,
        AtomClass::Summary,
        Statement::claim(DREAMT.0, DREAMT.1, Value::Integer(i128::from(through))),
    )
    .valid(Interval::starting(now))
    .observed_at(now)
    .extraction(extraction.0, extraction.1);
    for id in &episodes {
        marker = marker.link(LinkKind::DerivedFrom, *id, 1000);
    }
    let marker = marker.build().map_err(|e| CliError::Usage(e.to_string()))?;
    let marker_seq = store.append(marker).map_err(memory_error)?;
    drop(store);
    let view = crate::memory::commit(session, ledger, wallet, log, false)?;
    let on_ledger = committed(ledger, log)?;
    let turned_down: Vec<Json> = dreamt
        .rejected
        .iter()
        .map(|(proposal, reason)| json!({"proposal": proposal, "reason": reason}))
        .chain(
            outcome
                .rejected
                .iter()
                .map(|r| json!({"proposal": r.proposal, "reason": r.reason})),
        )
        .collect();
    Ok(json!({
        "log": log,
        "model": model,
        "sessions_read": dreamt.read,
        "sessions_unread": dreamt.unread.iter().map(|(s, why)| json!({"session": s, "why": why})).collect::<Vec<_>>(),
        "sessions_elsewhere": elsewhere,
        "kept": kept,
        "turned_down": turned_down,
        "dreamt_through": through,
        "marker": marker_seq,
        "tokens": dreamt.usage.total(),
        "committed": format!(
            "Pulse {}: {}",
            view.pulse.0,
            crate::describe::receipt(&session.names(), &view.effects)
        ),
        "size_on_ledger": on_ledger.size,
    }))
}

/// Records `notes` in `store`, written by `writer`: each a derived atom
/// linked to the atoms it cites and to the notes it replaces (by the ids
/// they were shown under, in `atoms`), with the extraction trace given
/// (the model's digest, then the code's); each note it replaces revised to
/// superseded; a note that cites an untrusted atom marked as a
/// prompt-injection risk, unsafe for planning. What was kept, an object
/// per note.
fn record_notes(
    store: &mut LogStore,
    writer: AccountId,
    notes: &[consolidate::Note],
    atoms: &BTreeMap<String, (MemoryAtomId, u64, bool)>,
    extraction: (Hash32, Hash32),
    now: u64,
) -> Result<Vec<Json>, CliError> {
    let (model_digest, code_digest) = extraction;
    let mut kept = Vec::new();
    let mut superseded: BTreeSet<MemoryAtomId> = BTreeSet::new();
    for note in notes {
        let kind = NOTE_KINDS
            .iter()
            .find(|(name, _)| *name == note.kind)
            .map(|(_, kind)| *kind)
            .expect("the gates keep only known kinds");
        let sources: Vec<(MemoryAtomId, u64, bool)> = note
            .sources
            .iter()
            .filter_map(|id| atoms.get(id).copied())
            .collect();
        let replaces: Vec<(MemoryAtomId, u64, bool)> = note
            .replaces
            .iter()
            .filter_map(|id| atoms.get(id).copied())
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
    Ok(kept)
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
