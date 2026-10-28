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
//! afterwards.

use std::collections::BTreeSet;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value as Json};
use sion_agent::tools::{Effect, Outcome, Tool};
use sion_agent::ToolSpec;
use sion_core::hash::Hash32;
use sion_core::ids::ObjectId;
use sion_memory::{
    prove_subject, verify_subject, AtomClass, BeliefState, Interval, LogStore, MemoryAtom,
    MemoryAtomId, MemoryKind, Statement,
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
fn withdrawn(store: &LogStore) -> BTreeSet<MemoryAtomId> {
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
    value
}

/// The agent's core memory as prompt text: its live preferences, goals and
/// procedures, newest first, within [`MAX_CORE_CHARS`].
pub fn core(log: &ObjectId) -> String {
    let Ok(store) = LogStore::open(store_dir(log)) else {
        return String::new();
    };
    let gone = withdrawn(&store);
    let mut lines = Vec::new();
    let mut used = 0;
    for atom in store.log().atoms().iter().rev() {
        if !CORE.contains(&atom.kind) || gone.contains(&atom.id()) {
            continue;
        }
        if let Statement::Claim { .. } = atom.statement {
            let text = format!(
                "- ({}) {}",
                kind_name(atom.kind),
                statement_text(&atom.statement)
            );
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
    format!(
        "What you remember and always keep in mind (from your memory log; use memory_recall for more):\n{}\n",
        lines.join("\n")
    )
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
        let mut store = self.store()?;
        let now = nanos();
        let atom = MemoryAtom::builder(
            store.log().writer(),
            store.log().len(),
            kind,
            class,
            statement,
        )
        .valid(Interval::starting(now))
        .observed_at(now)
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
        match self.0.append(*kind, AtomClass::AgentMemory, statement) {
            Ok((id, pending)) => Outcome::ok(format!(
                "remembered as atom {} ({pending} atom(s) not committed to the ledger yet)",
                id.short()
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

/// Records a finished session as an episode, with its trajectory's digest,
/// and commits the log.
pub fn record_episode(
    memory: &Memory,
    session: &str,
    task: &str,
    outcome: &str,
    digest: &Hash32,
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
        "task: {}; outcome: {}; trajectory {}",
        short(task, 300),
        short(outcome, 600),
        digest.to_hex()
    );
    let statement = Statement::claim(
        format!("session-{session}"),
        "episode",
        sion_memory::Value::Text(text),
    );
    memory.append(MemoryKind::Episodic, AtomClass::Event, statement)?;
    memory.commit()
}
