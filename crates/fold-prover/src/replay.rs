//! Stack A: replay proofs with the reference executor.
//!
//! **Simulation of a succinct proof.** The proof body is everything needed
//! to re-execute the chunk statelessly — its transactions or deferred
//! items, its witness and the expected effects — and verification re-runs
//! `fold_exec::replay_chunk` against the statement.

use borsh::{BorshDeserialize, BorshSerialize};
use fold_core::encode::{decode, encode};
use fold_exec::pulse::{ChunkStatement, ChunkTrace, Witness};
use fold_exec::{replay_chunk, Deferred, SignedTransaction, TxEffects};

use crate::error::ProveError;
use crate::stack::{LeafProof, ProofStack};

/// What a replay proof carries.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct ReplayBody {
    /// Transactions of the chunk.
    pub transactions: Vec<SignedTransaction>,
    /// Deferred items (deferred phase only).
    pub deferred_items: Vec<Deferred>,
    /// The witness.
    pub witness: Witness,
    /// The effects the chunk produced (for receipts).
    pub effects: Vec<TxEffects>,
}

/// Rebuilds a trace from a statement and a replay body.
pub fn trace_from(statement: &ChunkStatement, body: ReplayBody) -> ChunkTrace {
    ChunkTrace {
        statement: *statement,
        transactions: body.transactions,
        deferred_items: body.deferred_items,
        witness: body.witness,
        writes: Vec::new(),
        effects: body.effects,
    }
}

/// Encodes a trace as a replay body.
pub fn body_of(trace: &ChunkTrace) -> Vec<u8> {
    encode(&ReplayBody {
        transactions: trace.transactions.clone(),
        deferred_items: trace.deferred_items.clone(),
        witness: trace.witness.clone(),
        effects: trace.effects.clone(),
    })
}

/// Stack A: stateless re-execution with `fold-exec`.
#[derive(Clone, Copy, Debug, Default)]
pub struct ReplayStack;

impl ProofStack for ReplayStack {
    fn name(&self) -> &str {
        "A"
    }

    fn simulated(&self) -> bool {
        true
    }

    fn prove(&self, trace: &ChunkTrace) -> Result<LeafProof, ProveError> {
        replay_chunk(trace).map_err(|e| ProveError::Leaf {
            index: trace.statement.index,
            stack: "A".into(),
            reason: e.to_string(),
        })?;
        Ok(LeafProof {
            stack: "A".into(),
            statement: trace.statement.digest(),
            body: body_of(trace),
        })
    }

    fn verify(&self, statement: &ChunkStatement, proof: &LeafProof) -> Result<(), ProveError> {
        let fail = |reason: String| ProveError::Leaf {
            index: statement.index,
            stack: "A".into(),
            reason,
        };
        if proof.stack != "A" || proof.statement != statement.digest() {
            return Err(fail("proof is for another statement or stack".into()));
        }
        let body: ReplayBody = decode(&proof.body).map_err(|_| fail("undecodable proof".into()))?;
        let trace = trace_from(statement, body);
        replay_chunk(&trace).map_err(|e| fail(e.to_string()))?;
        Ok(())
    }
}
