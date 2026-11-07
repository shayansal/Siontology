//! An agent's memory, as runtime tools over its memory log (specification
//! v1.9): facts, preferences, procedures and episodes recorded as atoms in a
//! log only the agent can append to, whose head the ledger keeps.
//!
//! - `memory_remember` records an atom in the local store; the session
//!   commits the head to the ledger when it ends (or `memory_commit`).
//! - `memory_recall` answers for a subject and a relation with every atom
//!   the committed log holds, proved complete against the index root on the
//!   ledger, and lists what is recorded but not committed yet.
//! - `memory_search` scans the local store by words.
//! - `memory_forget` records a retraction: nothing is deleted, and recall
//!   and search leave retracted atoms out.
//!
//! The agent's **core memory** (its live preferences, goals and procedures)
//! is rendered into the system prompt of every session ([`core`]), and each
//! finished session is recorded as an episode with its trajectory's digest
//! ([`record_episode`]), so that what the agent did cannot be rewritten
//! afterwards: `session_search` ([`crate::recall`]) checks the sessions it
//! finds against those digests.

use std::collections::BTreeSet;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value as Json};
use sion_agent::tools::{Effect, Outcome, Tool};
use sion_agent::ToolSpec;
use sion_core::hash::Hash32;
use sion_core::ids::ObjectId;
use sion_memory::{
    prove_subject, verify_subject, AiUsage, AtomClass, BeliefState, Interval, LogStore, MemoryAtom,
    MemoryAtomId, MemoryKind, Statement, Taint,
};

use crate::error::CliError;
use crate::memory::{committed, describe_atom, open_store, parse_value, statement_text, store_dir};
use crate::runtime::Acting;
use crate::session::Session;

/// The kinds an agent records, by the names its tools use.
pub const KINDS: [(&str, MemoryKind); 7] = [
    ("fact", MemoryKind::Semantic),
    ("episode", MemoryKind::Episodic),
    ("procedure", MemoryKind::Procedural),
    ("preference", MemoryKind::Preference),
    ("goal", MemoryKind::Goal),
    ("plan", MemoryKind::Plan),
    ("reflection", MemoryKind::Reflection),
];

/// The kinds rendered into every prompt.
pub const CORE: [MemoryKind; 3] = [
    MemoryKind::Preference,
    MemoryKind::Goal,
    MemoryKind::Procedural,
];

/// The most characters of core memory in a prompt.
pub const MAX_CORE_CHARS: usize = 4_000;

fn nanos() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

fn kind_name(kind: MemoryKind) -> String {
    KINDS
        .iter()
        .find(|(_, k)| *k == kind)
        .map(|(name, _)| name.to_string())
        .unwrap_or_else(|| format!("{kind:?}").to_lowercase())
}

/// The atoms the log's own revisions retracted, refuted or superseded.
pub(crate) fn withdrawn(store: &LogStore) -> BTreeSet<MemoryAtomId> {
    let mut out = BTreeSet::new();
    for atom in store.log().atoms() {
        if let Statement::Revision { targets, state, .. } = &atom.statement {
            if matches!(
                state,
                BeliefState::Retracted | BeliefState::Refuted | BeliefState::Superseded
            ) {
                out.extend(targets.iter().copied());
            }
        }
    }
    out
}

fn line(atom: &MemoryAtom, committed_size: u64) -> Json {
    let mut value = describe_atom(atom);
    value["kind"] = json!(kind_name(atom.kind));
    value["committed"] = json!(atom.seq < committed_size);
    if !promptable(atom) {
        value["from_outside"] = json!(true);
    }
    value
}

/// Whether an atom may be put into a prompt as something to keep in mind:
/// not one that may carry instructions from outside, nor one marked unsafe
/// to plan with.
pub fn promptable(atom: &MemoryAtom) -> bool {
    !matches!(
        atom.policy.taint,
        Taint::PromptInjectionRisk | Taint::Poisoned
    ) && !matches!(
        atom.policy.ai_usage,
        AiUsage::UnsafeForPlanning | AiUsage::SimulationOnly
    )
}

/// What an agent's core memory is headed with in its prompt.
pub const CORE_HEADER: &str =
    "What you remember and always keep in mind (from your memory log; use memory_recall for more):";

/// What a curated log's core memory is headed with in a prompt.
pub const SHARED_HEADER: &str =
    "What your team keeps in mind, from a curated log that only its dreams write (read-only):";

/// The agent's core memory as prompt text: its live preferences, goals and
/// procedures, newest first, within [`MAX_CORE_CHARS`]. Atoms that may
/// carry instructions from outside are left out ([`promptable`]).
pub fn core(log: &ObjectId) -> String {
    core_under(log, CORE_HEADER)
}

/// A curated log's core memory as prompt text, as [`core`] gives an
/// agent's own, under [`SHARED_HEADER`].
pub fn shared_core(log: &ObjectId) -> String {
    let Ok(store) = LogStore::open(store_dir(log)) else {
        return String::new();
    };
    // What a reader here found of the log's vouchings and revocations
    // ([`crate::vouching::refresh`], before each run): a revoked fact and
    // whatever rests on it are left out; a vouched one says who vouched.
    let found = crate::vouching::found(log);
    let atoms = store.log().atoms();
    let standings = found.standings(atoms);
    let gone = withdrawn(&store);
    let mut lines = Vec::new();
    let mut used = 0;
    for (atom, standing) in atoms.iter().zip(&standings).rev() {
        if !CORE.contains(&atom.kind)
            || gone.contains(&atom.id())
            || !promptable(atom)
            || !standing.kept()
        {
            continue;
        }
        if let Statement::Claim { .. } = atom.statement {
            let mut text = core_line(atom.kind, &atom.statement);
            if let crate::vouching::Standing::Vouched { .. } = standing {
                text.push_str(&format!(" ({})", standing.describe()));
            }
            used += text.len() + 1;
            if used > MAX_CORE_CHARS {
                break;
            }
            lines.push(text);
        }
    }
    if lines.is_empty() {
        return String::new();
    }
    format!("{SHARED_HEADER}\n{}\n", lines.join("\n"))
}

/// The line a note is in a prompt's memory: its kind and its statement.
pub fn core_line(kind: MemoryKind, statement: &Statement) -> String {
    format!("- ({}) {}", kind_name(kind), statement_text(statement))
}

/// Core memory text as [`core`] gives it (or nothing), with `lines` before
/// its own, under [`CORE_HEADER`].
pub fn with_lines(core: &str, lines: &[String]) -> String {
    if lines.is_empty() {
        return core.to_string();
    }
    let mut all = lines.to_vec();
    all.extend(
        core.lines()
            .skip(1)
            .filter(|l| !l.trim().is_empty())
            .map(str::to_string),
    );
    format!("{CORE_HEADER}\n{}\n", all.join("\n"))
}

fn core_under(log: &ObjectId, header: &str) -> String {
    let Ok(store) = LogStore::open(store_dir(log)) else {
        return String::new();
    };
    let gone = withdrawn(&store);
    let mut lines = Vec::new();
    let mut used = 0;
    for atom in store.log().atoms().iter().rev() {
        if !CORE.contains(&atom.kind) || gone.contains(&atom.id()) || !promptable(atom) {
            continue;
        }
        if let Statement::Claim { .. } = atom.statement {
            let text = core_line(atom.kind, &atom.statement);
            used += text.len() + 1;
            if used > MAX_CORE_CHARS {
                break;
            }
            lines.push(text);
        }
    }
    if lines.is_empty() {
        return String::new();
    }
    format!("{header}\n{}\n", lines.join("\n"))
}

fn schema(properties: Json, required: &[&str]) -> Json {
    json!({"type": "object", "properties": properties, "required": required})
}

/// The memory tools of an agent with the log `log`.
#[derive(Clone, Debug)]
pub struct Memory {
    /// Who acts.
    pub acting: Acting,
    /// The agent's memory log.
    pub log: ObjectId,
}

impl Memory {
    fn store(&self) -> Result<LogStore, CliError> {
        let mut session = Session::open(self.acting.ledger.as_ref())?;
        let writer = session.id(&self.acting.wallet);
        open_store(&self.log, writer)
    }

    fn append(
        &self,
        kind: MemoryKind,
        class: AtomClass,
        statement: Statement,
    ) -> Result<(MemoryAtomId, u64), CliError> {
        self.append_marked(kind, class, statement, false)
    }

    /// Appends an atom; one `from_outside` is marked as a prompt-injection
    /// risk, unsafe to plan with.
    fn append_marked(
        &self,
        kind: MemoryKind,
        class: AtomClass,
        statement: Statement,
        from_outside: bool,
    ) -> Result<(MemoryAtomId, u64), CliError> {
        let mut store = self.store()?;
        let now = nanos();
        let mut builder = MemoryAtom::builder(
            store.log().writer(),
            store.log().len(),
            kind,
            class,
            statement,
        )
        .valid(Interval::starting(now))
        .observed_at(now);
        if from_outside {
            builder = builder
                .taint(Taint::PromptInjectionRisk)
                .ai_usage(AiUsage::UnsafeForPlanning);
        }
        let atom = builder
            .build()
            .map_err(|e| CliError::Usage(e.to_string()))?;
        let id = atom.id();
        store.append(atom).map_err(crate::memory::memory_error)?;
        let committed = committed(self.acting.ledger.as_ref(), &self.log)?;
        Ok((id, store.log().len().saturating_sub(committed.size)))
    }

    /// Commits the local head to the ledger if anything is uncommitted.
    pub fn commit(&self) -> Result<Option<String>, CliError> {
        let ledger = self.acting.ledger.as_ref();
        let store = self.store()?;
        let on_ledger = committed(ledger, &self.log)?;
        if store.log().len() == on_ledger.size {
            return Ok(None);
        }
        let mut session = Session::open(ledger)?;
        let view =
            crate::memory::commit(&mut session, ledger, &self.acting.wallet, &self.log, false)?;
        Ok(Some(format!(
            "Pulse {}: {}",
            view.pulse.0,
            crate::describe::receipt(&session.names(), &view.effects)
        )))
    }

    /// The tools.
    pub fn tools(&self) -> Vec<Box<dyn Tool>> {
        vec![
            Box::new(Remember(self.clone())),
            Box::new(Recall(self.clone())),
            Box::new(Search(self.clone())),
            Box::new(Forget(self.clone())),
            Box::new(Commit(self.clone())),
        ]
    }
}

/// `memory_remember`.
#[derive(Clone, Debug)]
pub struct Remember(pub Memory);

impl Tool for Remember {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "memory_remember".into(),
            description: "Record something in your memory log, as subject, relation and value: \
                          a fact, an episode, a procedure, a preference, a goal, a plan or a \
                          reflection. Preferences, goals and procedures are shown to you in every \
                          session. Nothing recorded can be changed later, only retracted."
                .into(),
            input_schema: schema(
                json!({
                    "subject": {"type": "string", "description": "what it is about, for example \"user\" or \"supplier-acme\""},
                    "relation": {"type": "string", "description": "for example \"prefers\" or \"invoice-terms\""},
                    "value": {"type": "string", "description": "text, a number, true or false, or @name for an entity"},
                    "kind": {"type": "string", "enum": KINDS.iter().map(|(n, _)| *n).collect::<Vec<_>>()}
                }),
                &["subject", "relation", "value", "kind"],
            ),
        }
    }

    fn effect(&self) -> Effect {
        Effect::Write
    }

    fn call(&self, input: &Json, _key: &str) -> Outcome {
        self.remember(input, false)
    }

    fn call_tainted(&self, input: &Json, _key: &str) -> Outcome {
        self.remember(input, true)
    }
}

impl Remember {
    /// Records the atom; one recorded `from_outside` (by a tainted session)
    /// is marked as a prompt-injection risk, kept and recalled but never
    /// shown in a prompt as core memory.
    fn remember(&self, input: &Json, from_outside: bool) -> Outcome {
        let subject = input["subject"].as_str().unwrap_or_default().trim();
        let relation = input["relation"].as_str().unwrap_or_default().trim();
        let value = input["value"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| input["value"].to_string());
        let kind_text = input["kind"].as_str().unwrap_or("fact");
        let Some((_, kind)) = KINDS.iter().find(|(n, _)| *n == kind_text) else {
            return Outcome::error(format!(
                "kind is one of {}",
                KINDS.map(|(n, _)| n).join(", ")
            ));
        };
        let statement = Statement::claim(subject, relation, parse_value(&value));
        match self
            .0
            .append_marked(*kind, AtomClass::AgentMemory, statement, from_outside)
        {
            Ok((id, pending)) => Outcome::ok(format!(
                "remembered as atom {} ({pending} atom(s) not committed to the ledger yet){}",
                id.short(),
                if from_outside {
                    "; this session read content from outside, so the atom is marked as such and never shown in your prompt"
                } else {
                    ""
                }
            )),
            Err(e) => Outcome::error(e.to_string()),
        }
    }
}

/// `memory_recall`.
#[derive(Clone, Debug)]
pub struct Recall(pub Memory);

impl Tool for Recall {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "memory_recall".into(),
            description: "Recall what your memory log holds about a subject. With a relation, \
                          the answer is every atom of the committed log, proved complete against \
                          the ledger; without one, every relation recorded locally."
                .into(),
            input_schema: schema(
                json!({"subject": {"type": "string"}, "relation": {"type": "string"}}),
                &["subject"],
            ),
        }
    }

    fn effect(&self) -> Effect {
        Effect::Read
    }

    fn call(&self, input: &Json, _key: &str) -> Outcome {
        let subject = input["subject"].as_str().unwrap_or_default().trim();
        let relation = input["relation"]
            .as_str()
            .map(str::trim)
            .filter(|r| !r.is_empty());
        let result = (|| -> Result<Json, CliError> {
            let store = self.0.store()?;
            let state = committed(self.0.acting.ledger.as_ref(), &self.0.log)?;
            let gone = withdrawn(&store);
            let log = store.log();
            let live = |atom: &&MemoryAtom| !gone.contains(&atom.id());
            match relation {
                Some(relation) => {
                    let index = log.index(state.size).map_err(crate::memory::memory_error)?;
                    let (ids, proof) = prove_subject(&index, subject, relation);
                    verify_subject(&state.index, subject, relation, &ids, &proof)
                        .map_err(crate::memory::memory_error)?;
                    let proven: Vec<Json> = ids
                        .iter()
                        .filter_map(|id| log.atom_by_id(id))
                        .filter(live)
                        .map(|a| line(a, state.size))
                        .collect();
                    let pending: Vec<Json> = log.atoms()[state.size as usize..]
                        .iter()
                        .filter(live)
                        .filter(|a| matches!(&a.statement, Statement::Claim { subject: s, predicate: p, .. } if s == subject && p == relation))
                        .map(|a| line(a, state.size))
                        .collect();
                    Ok(json!({
                        "subject": subject,
                        "relation": relation,
                        "committed": proven,
                        "complete": format!("proved complete against the index root on the ledger, as of {} atoms", state.size),
                        "not_committed_yet": pending,
                    }))
                }
                None => {
                    let atoms: Vec<Json> = log
                        .atoms()
                        .iter()
                        .filter(live)
                        .filter(|a| matches!(&a.statement, Statement::Claim { subject: s, .. } if s == subject))
                        .map(|a| line(a, state.size))
                        .collect();
                    Ok(json!({
                        "subject": subject,
                        "atoms": atoms,
                        "complete": "a local scan; give a relation for an answer proved complete",
                    }))
                }
            }
        })();
        match result {
            Ok(value) => Outcome::ok(value.to_string()),
            Err(e) => Outcome::error(e.to_string()),
        }
    }
}

/// `memory_search`.
#[derive(Clone, Debug)]
pub struct Search(pub Memory);

impl Tool for Search {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "memory_search".into(),
            description: "Search your memory by words in subjects, relations and values; newest \
                          first, at most 20."
                .into(),
            input_schema: schema(json!({"words": {"type": "string"}}), &["words"]),
        }
    }

    fn effect(&self) -> Effect {
        Effect::Read
    }

    fn call(&self, input: &Json, _key: &str) -> Outcome {
        let words: Vec<String> = input["words"]
            .as_str()
            .unwrap_or_default()
            .split_whitespace()
            .map(|w| w.to_lowercase())
            .collect();
        let result = (|| -> Result<Json, CliError> {
            let store = self.0.store()?;
            let state = committed(self.0.acting.ledger.as_ref(), &self.0.log)?;
            let gone = withdrawn(&store);
            let found: Vec<Json> = store
                .log()
                .atoms()
                .iter()
                .rev()
                .filter(|a| {
                    !gone.contains(&a.id()) && matches!(a.statement, Statement::Claim { .. })
                })
                .filter(|a| {
                    let text = statement_text(&a.statement).to_lowercase();
                    words.iter().all(|w| text.contains(w))
                })
                .take(20)
                .map(|a| line(a, state.size))
                .collect();
            Ok(json!({"found": found}))
        })();
        match result {
            Ok(value) => Outcome::ok(value.to_string()),
            Err(e) => Outcome::error(e.to_string()),
        }
    }
}

/// `memory_forget`.
#[derive(Clone, Debug)]
pub struct Forget(pub Memory);

impl Tool for Forget {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "memory_forget".into(),
            description: "Retract an atom of your memory that is wrong or no longer holds: its id \
                          (or the start of it) and why. The atom stays in the log; recall and \
                          search leave it out."
                .into(),
            input_schema: schema(
                json!({"atom": {"type": "string"}, "reason": {"type": "string"}}),
                &["atom", "reason"],
            ),
        }
    }

    fn effect(&self) -> Effect {
        Effect::Write
    }

    fn call(&self, input: &Json, _key: &str) -> Outcome {
        let wanted = input["atom"]
            .as_str()
            .unwrap_or_default()
            .trim()
            .to_lowercase();
        let reason = input["reason"]
            .as_str()
            .unwrap_or_default()
            .trim()
            .to_string();
        let target = (|| -> Result<MemoryAtomId, CliError> {
            if wanted.len() < 6 {
                return Err(CliError::Usage(
                    "give at least 6 hex digits of the atom".into(),
                ));
            }
            let store = self.0.store()?;
            let matches: Vec<MemoryAtomId> = store
                .log()
                .ids()
                .iter()
                .filter(|id| id.0.to_hex().starts_with(&wanted))
                .copied()
                .collect();
            match matches.as_slice() {
                [one] => Ok(*one),
                [] => Err(CliError::Usage(format!("no atom {wanted} in your memory"))),
                _ => Err(CliError::Usage(format!(
                    "{wanted} matches more than one atom"
                ))),
            }
        })();
        let target = match target {
            Ok(target) => target,
            Err(e) => return Outcome::error(e.to_string()),
        };
        let statement = Statement::Revision {
            targets: vec![target],
            state: BeliefState::Retracted,
            end_valid: Some(nanos()),
            reason,
        };
        match self
            .0
            .append(MemoryKind::Correction, AtomClass::Revision, statement)
        {
            Ok((id, _)) => Outcome::ok(format!(
                "retracted atom {} (by atom {})",
                target.short(),
                id.short()
            )),
            Err(e) => Outcome::error(e.to_string()),
        }
    }
}

/// `memory_commit`.
#[derive(Clone, Debug)]
pub struct Commit(pub Memory);

impl Tool for Commit {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "memory_commit".into(),
            description: "Commit what you recorded to the ledger now (the session also does it \
                          when it ends). From then on it cannot be rewritten, and recall proves \
                          it complete."
                .into(),
            input_schema: schema(json!({}), &[]),
        }
    }

    fn effect(&self) -> Effect {
        Effect::Write
    }

    fn call(&self, _input: &Json, _key: &str) -> Outcome {
        match self.0.commit() {
            Ok(Some(receipt)) => Outcome::ok(receipt),
            Ok(None) => Outcome::ok("nothing to commit: the ledger holds the whole log"),
            Err(e) => Outcome::error(e.to_string()),
        }
    }
}

/// What a finished session's episode records about its conversation: the
/// trajectory's digest, how many messages it covers, how many times the
/// session had been compacted, and whether the session read content from
/// outside (a tainted session's episode is marked as a prompt-injection
/// risk, so neither it nor what is consolidated from it reaches a prompt
/// as something to keep in mind).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Trajectory {
    /// The digest ([`sion_agent::trace::digest`]).
    pub digest: Hash32,
    /// The messages it covers.
    pub messages: usize,
    /// The session's compactions so far.
    pub compactions: u32,
    /// Whether the session was ever tainted.
    pub from_outside: bool,
}

/// How an episode names its trajectory: `trajectory <hex> over <n>
/// messages`, and ` after <c> compactions` if there were any
/// ([`crate::recall::parse_episode`] reads it).
pub fn trajectory_text(trajectory: &Trajectory) -> String {
    let plural = |n: u64, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    let mut text = format!(
        "trajectory {} over {}",
        trajectory.digest.to_hex(),
        plural(trajectory.messages as u64, "message", "messages")
    );
    if trajectory.compactions > 0 {
        text.push_str(&format!(
            " after {}",
            plural(
                u64::from(trajectory.compactions),
                "compaction",
                "compactions"
            )
        ));
    }
    text
}

/// Records a finished session as an episode, with its trajectory, and
/// commits the log.
pub fn record_episode(
    memory: &Memory,
    session: &str,
    task: &str,
    outcome: &str,
    trajectory: &Trajectory,
) -> Result<Option<String>, CliError> {
    let short = |text: &str, n: usize| -> String {
        let one: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
        if one.chars().count() > n {
            format!("{}…", one.chars().take(n).collect::<String>())
        } else {
            one
        }
    };
    let text = format!(
        "task: {}; outcome: {}; {}",
        short(task, 300),
        short(outcome, 600),
        trajectory_text(trajectory)
    );
    let statement = Statement::claim(
        format!("session-{session}"),
        "episode",
        sion_memory::Value::Text(text),
    );
    memory.append_marked(
        MemoryKind::Episodic,
        AtomClass::Event,
        statement,
        trajectory.from_outside,
    )?;
    memory.commit()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_go_before_what_core_memory_keeps() {
        let line = core_line(
            MemoryKind::Procedural,
            &Statement::claim(
                "invoices",
                "are paid",
                sion_memory::Value::Text("on Fridays".into()),
            ),
        );
        assert!(
            line.starts_with("- (procedure) invoices are paid"),
            "{line}"
        );
        let core = format!("{CORE_HEADER}\n- (preference) user prefers email\n");
        assert_eq!(
            with_lines(&core, std::slice::from_ref(&line)),
            format!("{CORE_HEADER}\n{line}\n- (preference) user prefers email\n")
        );
        assert_eq!(
            with_lines("", std::slice::from_ref(&line)),
            format!("{CORE_HEADER}\n{line}\n")
        );
        assert_eq!(with_lines(&core, &[]), core);
    }
}
