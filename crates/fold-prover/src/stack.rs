//! Proof stacks and leaf proofs.

use borsh::{BorshDeserialize, BorshSerialize};
use fold_assurance::atom::standard;
use fold_core::hash::Hash32;
use fold_core::ids::AtomId;
use fold_exec::pulse::{ChunkStatement, ChunkTrace};
use serde::{Deserialize, Serialize};

use crate::error::ProveError;

/// One stack's proof of one chunk statement.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct LeafProof {
    /// The stack that produced it.
    pub stack: String,
    /// Digest of the statement it proves.
    pub statement: Hash32,
    /// The proof itself (stack-specific encoding).
    pub body: Vec<u8>,
}

impl LeafProof {
    /// Size in bytes.
    pub fn size(&self) -> usize {
        self.body.len()
    }
}

/// A proof system for chunk statements.
pub trait ProofStack: Send + Sync {
    /// The stack's name (`"A"`, `"B"`).
    fn name(&self) -> &str;

    /// The atom standing for this stack's soundness.
    fn atom(&self) -> AtomId {
        standard::proof_stack(self.name()).id
    }

    /// Whether proofs from this stack are simulated (replay) rather than succinct.
    fn simulated(&self) -> bool;

    /// Proves the chunk's statement. Fails if the trace does not establish it.
    fn prove(&self, trace: &ChunkTrace) -> Result<LeafProof, ProveError>;

    /// Verifies a leaf proof of `statement`.
    fn verify(&self, statement: &ChunkStatement, proof: &LeafProof) -> Result<(), ProveError>;
}
