//! Stateless re-execution of a chunk from its witness.
//!
//! This is what a prover (and the replay proof stack) runs: it starts from
//! nothing but the chunk statement and the witness, checks every witnessed
//! object against the starting state root, executes the chunk over the
//! witness alone, applies the writes to a partial state tree and checks the
//! resulting root, knowledge root and accumulators against the statement.
//!
//! An execution that reads an object the witness does not cover is not a
//! faithful execution, so it invalidates the whole chunk (rather than
//! failing one transaction, which a dishonest prover could exploit to make a
//! valid transaction look rejected).

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use fold_core::hash::Hash32;
use fold_core::ids::ObjectId;
use fold_core::smt::{verify_proof, PartialSmt};

use crate::error::ExecError;
use crate::execute::{execute_transaction, PulseContext, TxEffects};
use crate::object::Object;
use crate::pulse::{apply_deferred, chain_deferred, chain_effects, steps_digest, ChunkTrace};
use crate::store::{ObjectStore, Overlay};

/// An object store backed only by a witness. Reads outside the witness are
/// recorded as misses.
#[derive(Debug, Default)]
pub struct WitnessStore {
    objects: BTreeMap<ObjectId, Option<Object>>,
    misses: RefCell<BTreeSet<ObjectId>>,
}

impl WitnessStore {
    /// A store over witnessed objects (`None` = proven absent).
    pub fn new(objects: BTreeMap<ObjectId, Option<Object>>) -> Self {
        WitnessStore {
            objects,
            misses: RefCell::new(BTreeSet::new()),
        }
    }

    /// Identifiers read but not covered.
    pub fn misses(&self) -> BTreeSet<ObjectId> {
        self.misses.borrow().clone()
    }
}

impl ObjectStore for WitnessStore {
    fn get_object(&self, id: &ObjectId) -> Result<Option<Object>, ExecError> {
        match self.objects.get(id) {
            Some(object) => Ok(object.clone()),
            None => {
                self.misses.borrow_mut().insert(*id);
                Err(ExecError::MissingObject(*id))
            }
        }
    }
}

/// What a successful replay established.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplayReport {
    /// The statement's digest.
    pub statement: Hash32,
    /// Objects witnessed.
    pub witnessed: usize,
    /// Objects written.
    pub written: usize,
    /// Transactions executed.
    pub transactions: usize,
}

fn invalid(reason: impl Into<String>) -> ExecError {
    ExecError::Invalid(format!("chunk replay: {}", reason.into()))
}

/// Re-executes a chunk from its witness and checks it against its statement.
pub fn replay_chunk(trace: &ChunkTrace) -> Result<ReplayReport, ExecError> {
    let statement = &trace.statement;
    let witness = &trace.witness;
    let before = &statement.before;
    let after = &statement.after;

    // Configuration and time.
    if witness.config.digest() != before.config || after.config != before.config {
        return Err(invalid("configuration does not match the commitment"));
    }
    if after.pulse != statement.pulse
        || (before.pulse != statement.pulse
            && before.pulse.0.checked_add(1) != Some(statement.pulse.0))
    {
        return Err(invalid("commitments are not for this Pulse"));
    }

    // Every witnessed object against the starting root.
    let mut tree = PartialSmt::new(before.state_root);
    let mut objects = BTreeMap::new();
    for (id, object, proof) in &witness.objects {
        if let Some(object) = object {
            if object.id != *id {
                return Err(invalid(format!("witness entry {id} holds another object")));
            }
        }
        let value = object.as_ref().map(Object::digest);
        verify_proof(&before.state_root, &id.0, value.as_ref(), proof)
            .map_err(|e| invalid(format!("witness proof for {id}: {e}")))?;
        tree.add_proof(&id.0, proof)
            .map_err(|e| invalid(format!("witness proof for {id}: {e}")))?;
        if objects.insert(*id, object.clone()).is_some() {
            return Err(invalid(format!("object {id} witnessed twice")));
        }
    }
    let mut frontier = witness.frontier.clone();
    if !frontier.is_well_formed()
        || frontier.root() != before.knowledge_root
        || frontier.size != before.knowledge_size
    {
        return Err(invalid("knowledge frontier does not match the commitment"));
    }

    let store = WitnessStore::new(objects);
    let ctx = PulseContext {
        pulse: statement.pulse,
        beacon: statement.beacon,
        config: witness.config.clone(),
        verify_signatures: true,
    };
    let mut overlay = Overlay::new(&store);
    let mut effects: Vec<TxEffects> = Vec::new();
    if statement.deferred_phase {
        if !trace.transactions.is_empty() {
            return Err(invalid("the deferred phase executes no transactions"));
        }
        let consumed = chain_deferred(&Hash32::ZERO, &trace.deferred_items);
        if consumed != statement.deferred_in || consumed != statement.deferred_out {
            return Err(invalid(
                "deferred items differ from those the transactions produced",
            ));
        }
        if statement.effects_in != statement.effects_out {
            return Err(invalid("the deferred phase produces no receipts"));
        }
        apply_deferred(&mut overlay, &ctx, &trace.deferred_items, &mut frontier)?;
    } else {
        if !trace.deferred_items.is_empty() {
            return Err(invalid("transaction chunks carry no deferred items"));
        }
        if steps_digest(&trace.transactions) != statement.steps {
            return Err(invalid("transactions differ from the statement"));
        }
        let mut deferred_acc = statement.deferred_in;
        let mut effects_acc = statement.effects_in;
        for tx in &trace.transactions {
            let result = execute_transaction(&mut overlay, &ctx, tx);
            deferred_acc = chain_deferred(&deferred_acc, &result.deferred);
            effects_acc = chain_effects(&effects_acc, &result);
            effects.push(result);
        }
        if deferred_acc != statement.deferred_out {
            return Err(invalid("deferred accumulator mismatch"));
        }
        if effects_acc != statement.effects_out {
            return Err(invalid("receipts accumulator mismatch"));
        }
    }
    let misses = store.misses();
    if !misses.is_empty() {
        return Err(invalid(format!(
            "execution read {} object(s) outside the witness",
            misses.len()
        )));
    }
    if frontier.root() != after.knowledge_root || frontier.size != after.knowledge_size {
        return Err(invalid("knowledge root mismatch"));
    }
    let writes = overlay.into_writes();
    for (id, object) in &writes {
        tree.insert(id.0, object.digest())
            .map_err(|e| invalid(format!("write to {id} outside the witness: {e}")))?;
    }
    if tree.root() != after.state_root {
        return Err(invalid("state root mismatch"));
    }
    Ok(ReplayReport {
        statement: statement.digest(),
        witnessed: trace.witness.objects.len(),
        written: writes.len(),
        transactions: trace.transactions.len(),
    })
}
