//! Verifying a chunk (spec §8).
//!
//! [`execute_chunk`] runs checks 1–4 (the statement's configuration and
//! Pulse numbers, the witness proofs, the knowledge frontier) and the
//! execution of step 5, and returns what execution produced.
//! [`verify_chunk`] then compares that with the statement (step 5's
//! accumulators, steps 6 and 7). A chunk is valid if and only if every check
//! passes.

use std::collections::{BTreeMap, BTreeSet};

use sion_core::hash::Hash32;
use sion_core::ids::{ObjectId, Pulse};
use sion_core::merkle::Frontier;
use sion_core::smt::{verify_proof, PartialSmt};
use sion_exec::execute::{Event, ExecStatus, TxEffects};
use sion_exec::object::Object;
use sion_exec::pulse::{ChunkStatement, ChunkTrace};

use crate::commit::{chain_deferred, chain_effects_leaf, effects_leaf, steps_digest};
use crate::context::SpecContext;
use crate::deferred::run_deferred_phase;
use crate::error::SpecError;
use crate::outcome::run_transaction;
use crate::view::ChunkView;

/// What executing a chunk produced, before it is compared with the
/// statement's results.
#[derive(Clone, Debug)]
pub struct ChunkExecution {
    /// The context the chunk executed in.
    pub context: SpecContext,
    /// Effects of each transaction, in order (empty for the deferred phase,
    /// whose forced intents are posted, not executed).
    pub effects: Vec<TxEffects>,
    /// The deferred-effects accumulator after the chunk.
    pub deferred_out: Hash32,
    /// The receipts accumulator after the chunk.
    pub effects_out: Hash32,
    /// The knowledge log's frontier after the chunk.
    pub frontier: Frontier,
    /// Every object the chunk wrote, as last written.
    pub writes: BTreeMap<ObjectId, Object>,
    /// Identifiers whose reads the witness served.
    pub reads: BTreeSet<ObjectId>,
    /// The state root after applying the writes to the witnessed partial
    /// tree.
    pub state_root: Hash32,
    /// Events of the deferred phase (not committed; empty for transaction
    /// chunks).
    pub deferred_events: Vec<Event>,
}

/// What a verified chunk proved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChunkReport {
    /// Digest of the verified statement.
    pub statement_digest: Hash32,
    /// The Pulse.
    pub pulse: Pulse,
    /// Position of the chunk in the Pulse.
    pub index: u32,
    /// Whether this was the deferred phase.
    pub deferred_phase: bool,
    /// Objects in the witness.
    pub witnessed: usize,
    /// Witnessed objects execution actually read.
    pub read: usize,
    /// Objects written.
    pub written: usize,
    /// Transactions executed (0 for the deferred phase).
    pub transactions: usize,
    /// Transactions that succeeded.
    pub succeeded: usize,
    /// Transactions that failed (fee charged, inputs released).
    pub failed: usize,
    /// Transactions rejected (nothing changed).
    pub rejected: usize,
    /// Deferred items consumed (deferred phase only).
    pub deferred_items: usize,
    /// Forced intents posted (deferred phase only).
    pub intents: usize,
    /// Claims appended to the knowledge log.
    pub claims_appended: u64,
}

/// Checks 1 and 2: configuration and Pulse numbers.
fn check_header(trace: &ChunkTrace) -> Result<(), SpecError> {
    let statement = &trace.statement;
    let witness = trace.witness.config.digest();
    if witness != statement.before.config || witness != statement.after.config {
        return Err(SpecError::ConfigMismatch {
            witness,
            before: statement.before.config,
            after: statement.after.config,
        });
    }
    let pulse = statement.pulse.0;
    let before = statement.before.pulse.0;
    let after = statement.after.pulse.0;
    let before_ok = before == pulse || pulse.checked_sub(1) == Some(before);
    if after != pulse || !before_ok {
        return Err(SpecError::PulseMismatch {
            pulse,
            before,
            after,
        });
    }
    Ok(())
}

/// Check 3: every witnessed object is proven against `S.before.state_root`;
/// returns the verified objects and the partial tree their proofs span.
fn open_witness(
    trace: &ChunkTrace,
) -> Result<(BTreeMap<ObjectId, Option<Object>>, PartialSmt), SpecError> {
    let root = trace.statement.before.state_root;
    let mut objects = BTreeMap::new();
    for (id, object, proof) in &trace.witness.objects {
        if let Some(object) = object {
            if object.id != *id {
                return Err(SpecError::WitnessIdMismatch {
                    id: *id,
                    found: object.id,
                });
            }
        }
        let value = object.as_ref().map(Object::digest);
        verify_proof(&root, &id.0, value.as_ref(), proof)
            .map_err(|reason| SpecError::WitnessProof { id: *id, reason })?;
        if objects.insert(*id, object.clone()).is_some() {
            return Err(SpecError::DuplicateWitness(*id));
        }
    }
    let tree = PartialSmt::from_proofs(
        root,
        trace
            .witness
            .objects
            .iter()
            .map(|(id, _, proof)| (&id.0, proof)),
    )
    .map_err(SpecError::WitnessTree)?;
    Ok((objects, tree))
}

/// Check 4: the frontier is well formed and matches `S.before`.
fn check_frontier(trace: &ChunkTrace) -> Result<(), SpecError> {
    let frontier = &trace.witness.frontier;
    let before = &trace.statement.before;
    if !frontier.is_well_formed()
        || frontier.size != before.knowledge_size
        || frontier.root() != before.knowledge_root
    {
        return Err(SpecError::FrontierMismatch);
    }
    Ok(())
}

/// Check 7's computation: applies every write to the witnessed partial tree.
fn state_root_after(
    mut tree: PartialSmt,
    writes: &BTreeMap<ObjectId, Object>,
) -> Result<Hash32, SpecError> {
    for (id, object) in writes {
        tree.insert(id.0, object.digest())
            .map_err(|reason| SpecError::UncoveredWrite { id: *id, reason })?;
    }
    Ok(tree.root())
}

/// Runs checks 1–4 and executes the chunk (step 5), reading only witnessed
/// objects. The results are not yet compared with the statement; see
/// [`verify_chunk`]. Useful to diagnose a rejected chunk.
pub fn execute_chunk(trace: &ChunkTrace) -> Result<ChunkExecution, SpecError> {
    check_header(trace)?;
    let (objects, tree) = open_witness(trace)?;
    check_frontier(trace)?;
    let statement: &ChunkStatement = &trace.statement;
    let context = SpecContext {
        pulse: statement.pulse,
        beacon: statement.beacon,
        config: trace.witness.config.clone(),
    };
    let mut view = ChunkView::new(objects);
    if statement.deferred_phase {
        let outcome = run_deferred_phase(
            &context,
            &view,
            trace.witness.frontier.clone(),
            &trace.deferred_items,
            &trace.transactions,
        )?;
        view.note_reads(outcome.reads);
        view.keep(outcome.writes);
        let (writes, reads) = view.into_parts();
        let state_root = state_root_after(tree, &writes)?;
        return Ok(ChunkExecution {
            context,
            effects: Vec::new(),
            deferred_out: chain_deferred(&Hash32::ZERO, &trace.deferred_items),
            effects_out: statement.effects_in,
            frontier: outcome.frontier,
            writes,
            reads,
            state_root,
            deferred_events: outcome.events,
        });
    }
    let mut deferred = statement.deferred_in;
    let mut receipts = statement.effects_in;
    let mut effects = Vec::with_capacity(trace.transactions.len());
    for signed in &trace.transactions {
        let produced = run_transaction(&context, &mut view, signed)?;
        deferred = chain_deferred(&deferred, &produced.deferred);
        receipts = chain_effects_leaf(&receipts, &effects_leaf(&produced));
        effects.push(produced);
    }
    let (writes, reads) = view.into_parts();
    let state_root = state_root_after(tree, &writes)?;
    Ok(ChunkExecution {
        context,
        effects,
        deferred_out: deferred,
        effects_out: receipts,
        frontier: trace.witness.frontier.clone(),
        writes,
        reads,
        state_root,
        deferred_events: Vec::new(),
    })
}

/// Verifies one chunk exactly as spec §8 describes, executing it with this
/// crate's own implementation of §3–§6. Returns what the chunk proved, or
/// the first check that failed.
pub fn verify_chunk(trace: &ChunkTrace) -> Result<ChunkReport, SpecError> {
    let statement = &trace.statement;
    // Step 5's shape checks: `S.steps = steps(T)` for both kinds of chunk
    // (for the deferred phase, `T` are the posted forced intents).
    if steps_digest(&trace.transactions) != statement.steps {
        return Err(SpecError::StepsMismatch);
    }
    if statement.deferred_phase {
        let consumed = chain_deferred(&Hash32::ZERO, &trace.deferred_items);
        if consumed != statement.deferred_in || consumed != statement.deferred_out {
            return Err(SpecError::DeferredMismatch);
        }
        if statement.effects_in != statement.effects_out {
            return Err(SpecError::EffectsMismatch);
        }
    } else if !trace.deferred_items.is_empty() {
        return Err(SpecError::UnexpectedDeferredItems(
            trace.deferred_items.len(),
        ));
    }

    let execution = execute_chunk(trace)?;

    // Step 5's results.
    if execution.deferred_out != statement.deferred_out {
        return Err(SpecError::DeferredMismatch);
    }
    if execution.effects_out != statement.effects_out {
        return Err(SpecError::EffectsMismatch);
    }
    // Step 6.
    let after = &statement.after;
    if execution.frontier.size != after.knowledge_size
        || execution.frontier.root() != after.knowledge_root
    {
        return Err(SpecError::KnowledgeMismatch {
            computed_size: execution.frontier.size,
            statement_size: after.knowledge_size,
        });
    }
    // Step 7.
    if execution.state_root != after.state_root {
        return Err(SpecError::StateRootMismatch {
            computed: execution.state_root,
            statement: after.state_root,
        });
    }

    let count = |code: fn(&ExecStatus) -> bool| {
        execution
            .effects
            .iter()
            .filter(|effects| code(&effects.status))
            .count()
    };
    Ok(ChunkReport {
        statement_digest: statement.digest(),
        pulse: statement.pulse,
        index: statement.index,
        deferred_phase: statement.deferred_phase,
        witnessed: trace.witness.objects.len(),
        read: execution.reads.len(),
        written: execution.writes.len(),
        transactions: execution.effects.len(),
        succeeded: count(|status| matches!(status, ExecStatus::Success)),
        failed: count(|status| matches!(status, ExecStatus::Failed(_))),
        rejected: count(|status| matches!(status, ExecStatus::Rejected(_))),
        deferred_items: trace.deferred_items.len(),
        intents: if statement.deferred_phase {
            trace.transactions.len()
        } else {
            0
        },
        claims_appended: execution
            .frontier
            .size
            .saturating_sub(statement.before.knowledge_size),
    })
}
