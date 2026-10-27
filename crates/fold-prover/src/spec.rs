//! Stack B: replay proofs with the independent implementation.
//!
//! **Simulation of a succinct proof**, like stack A, but checked by a
//! different program: `fold-spec`, written from
//! `docs/spec/state-transition.md` alone, without calling the reference
//! executor (`scripts/check-independence.sh` enforces this). The proof body
//! has the same layout as stack A's; verification runs
//! [`fold_spec::verify_chunk`]. A false statement now needs the same bug in
//! two independent implementations to verify, which is what the trust
//! formula `(stack_A ∧ circuit_A) ∨ (stack_B ∧ circuit_B)` prices.

use fold_core::encode::decode;
use fold_exec::pulse::{ChunkStatement, ChunkTrace};

use crate::error::ProveError;
use crate::replay::{body_of, trace_from, ReplayBody};
use crate::stack::{LeafProof, ProofStack};

/// Stack B: stateless re-execution with `fold-spec`.
#[derive(Clone, Copy, Debug, Default)]
pub struct SpecStack;

impl ProofStack for SpecStack {
    fn name(&self) -> &str {
        "B"
    }

    fn simulated(&self) -> bool {
        true
    }

    fn prove(&self, trace: &ChunkTrace) -> Result<LeafProof, ProveError> {
        fold_spec::verify_chunk(trace).map_err(|e| ProveError::Leaf {
            index: trace.statement.index,
            stack: "B".into(),
            reason: e.to_string(),
        })?;
        Ok(LeafProof {
            stack: "B".into(),
            statement: trace.statement.digest(),
            body: body_of(trace),
        })
    }

    fn verify(&self, statement: &ChunkStatement, proof: &LeafProof) -> Result<(), ProveError> {
        let fail = |reason: String| ProveError::Leaf {
            index: statement.index,
            stack: "B".into(),
            reason,
        };
        if proof.stack != "B" || proof.statement != statement.digest() {
            return Err(fail("proof is for another statement or stack".into()));
        }
        let body: ReplayBody = decode(&proof.body).map_err(|_| fail("undecodable proof".into()))?;
        let trace = trace_from(statement, body);
        fold_spec::verify_chunk(&trace).map_err(|e| fail(e.to_string()))?;
        Ok(())
    }
}
