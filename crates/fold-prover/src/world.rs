//! Folding leaf proofs into a World Proof.

use borsh::{BorshDeserialize, BorshSerialize};
use fold_core::hash::{hash_parts, Hash32};
use fold_core::ids::Pulse;
use fold_core::merkle;
use fold_exec::pulse::{chain_effects_leaf, effects_leaf, ChunkStatement, ChunkTrace, PulseOutput};
use fold_exec::WorldCommitment;
use serde::{Deserialize, Serialize};

use crate::error::ProveError;
use crate::stack::{LeafProof, ProofStack};

/// The proof that a Pulse's transition is valid.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct WorldProof {
    /// The Pulse.
    pub pulse: Pulse,
    /// The world before.
    pub before: WorldCommitment,
    /// The world after.
    pub after: WorldCommitment,
    /// Merkle root of the transaction effects.
    pub receipts_root: Hash32,
    /// One leaf per transaction effect, in execution order.
    pub receipt_leaves: Vec<Hash32>,
    /// The chunk statements, in order.
    pub statements: Vec<ChunkStatement>,
    /// Merkle root over the statements' digests (the aggregation tree).
    pub statements_root: Hash32,
    /// For each statement, one leaf proof per stack.
    pub proofs: Vec<Vec<LeafProof>>,
}

impl WorldProof {
    /// Encoded size in bytes.
    pub fn size(&self) -> usize {
        fold_core::encode::encode(self).len()
    }

    /// Digest of what the proof establishes: (Pulse, before, after,
    /// receipts root, statements root).
    pub fn claim_digest(&self) -> Hash32 {
        hash_parts(
            "fold/prover/world-claim",
            &[
                &self.pulse.0.to_be_bytes(),
                &self.before.digest().0,
                &self.after.digest().0,
                &self.receipts_root.0,
                &self.statements_root.0,
            ],
        )
    }
}

/// What a verification established.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorldReport {
    /// The Pulse.
    pub pulse: Pulse,
    /// Leaves in the proof.
    pub leaves: usize,
    /// Leaves re-verified (all, or a sample).
    pub verified_leaves: usize,
    /// Stacks whose proofs were checked.
    pub stacks: Vec<String>,
    /// Whether any checked stack is simulated (replay) rather than succinct.
    pub simulated: bool,
    /// Proof size in bytes.
    pub bytes: usize,
}

fn statements_root(statements: &[ChunkStatement]) -> Hash32 {
    let leaves: Vec<Hash32> = statements
        .iter()
        .map(|s| merkle::leaf_hash(&s.digest().0))
        .collect();
    merkle::root(&leaves)
}

/// Proves a Pulse with every given stack.
pub fn prove_pulse(
    output: &PulseOutput,
    traces: &[ChunkTrace],
    stacks: &[&dyn ProofStack],
) -> Result<WorldProof, ProveError> {
    if stacks.is_empty() {
        return Err(ProveError::Malformed("no proof stacks".into()));
    }
    let statements: Vec<ChunkStatement> = traces.iter().map(|t| t.statement).collect();
    let mut proofs = Vec::with_capacity(traces.len());
    for trace in traces {
        let mut leaf = Vec::with_capacity(stacks.len());
        for stack in stacks {
            leaf.push(stack.prove(trace)?);
        }
        proofs.push(leaf);
    }
    let proof = WorldProof {
        pulse: output.pulse,
        before: output.before,
        after: output.after,
        receipts_root: output.receipts_root,
        receipt_leaves: output.effects.iter().map(effects_leaf).collect(),
        statements_root: statements_root(&statements),
        statements,
        proofs,
    };
    check_structure(&proof)?;
    Ok(proof)
}

/// Checks everything except the leaf proofs: the statements chain from
/// `before` to `after`, the accumulators start empty and the deferred phase
/// consumes exactly what the transactions produced, the receipts match.
fn check_structure(proof: &WorldProof) -> Result<(), ProveError> {
    let statements = &proof.statements;
    let first = statements
        .first()
        .ok_or_else(|| ProveError::Chain("no statements".into()))?;
    let last = statements.last().expect("non-empty");
    if first.before != proof.before || last.after != proof.after {
        return Err(ProveError::Chain("statements do not span the Pulse".into()));
    }
    if first.deferred_in != Hash32::ZERO || first.effects_in != Hash32::ZERO {
        return Err(ProveError::Chain("accumulators must start empty".into()));
    }
    for (i, statement) in statements.iter().enumerate() {
        if statement.pulse != proof.pulse || statement.index as usize != i {
            return Err(ProveError::Chain(format!("statement {i} is out of place")));
        }
        if statement.deferred_phase != (i + 1 == statements.len()) {
            return Err(ProveError::Chain(
                "exactly the last statement is the deferred phase".into(),
            ));
        }
    }
    for pair in statements.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        if a.after != b.before
            || a.deferred_out != b.deferred_in
            || a.effects_out != b.effects_in
            || a.beacon != b.beacon
        {
            return Err(ProveError::Chain(format!(
                "statements {} and {} do not chain",
                a.index, b.index
            )));
        }
    }
    if proof.before.pulse.next() != proof.pulse || proof.after.pulse != proof.pulse {
        return Err(ProveError::Chain(
            "commitments are not for consecutive Pulses".into(),
        ));
    }
    // Receipts: the hash chain the statements carry and the Merkle root
    // light clients use must describe the same effects.
    let chained = proof
        .receipt_leaves
        .iter()
        .fold(Hash32::ZERO, |acc, leaf| chain_effects_leaf(&acc, leaf));
    if chained != last.effects_out {
        return Err(ProveError::Chain(
            "receipt leaves differ from the proven effects".into(),
        ));
    }
    if merkle::root(&proof.receipt_leaves) != proof.receipts_root {
        return Err(ProveError::Chain(
            "receipts root does not match its leaves".into(),
        ));
    }
    if statements_root(statements) != proof.statements_root {
        return Err(ProveError::Chain("statements root mismatch".into()));
    }
    if proof.proofs.len() != statements.len() {
        return Err(ProveError::Chain(
            "one set of leaf proofs per statement".into(),
        ));
    }
    Ok(())
}

fn verify_leaf(
    proof: &WorldProof,
    index: usize,
    stacks: &[&dyn ProofStack],
) -> Result<(), ProveError> {
    let statement = &proof.statements[index];
    for stack in stacks {
        let leaf = proof.proofs[index]
            .iter()
            .find(|p| p.stack == stack.name())
            .ok_or_else(|| ProveError::MissingStack(stack.name().to_string()))?;
        stack.verify(statement, leaf)?;
    }
    Ok(())
}

/// Verifies a World Proof: its structure, and every leaf under every
/// required stack.
pub fn verify_world_proof(
    proof: &WorldProof,
    stacks: &[&dyn ProofStack],
) -> Result<WorldReport, ProveError> {
    check_structure(proof)?;
    for index in 0..proof.statements.len() {
        verify_leaf(proof, index, stacks)?;
    }
    Ok(report(proof, stacks, proof.statements.len()))
}

/// Verifies a World Proof's structure fully and `samples` leaves chosen by
/// `seed` (always including the deferred phase) under every required stack.
pub fn verify_world_proof_sampled(
    proof: &WorldProof,
    stacks: &[&dyn ProofStack],
    samples: usize,
    seed: u64,
) -> Result<WorldReport, ProveError> {
    check_structure(proof)?;
    let leaves = proof.statements.len();
    let mut chosen = std::collections::BTreeSet::new();
    chosen.insert(leaves - 1);
    let mut counter = 0u64;
    while chosen.len() < samples.min(leaves) {
        let draw = hash_parts(
            "fold/prover/sample",
            &[&seed.to_be_bytes(), &counter.to_be_bytes()],
        );
        chosen.insert((draw.prefix_u64() % leaves as u64) as usize);
        counter += 1;
    }
    for index in &chosen {
        verify_leaf(proof, *index, stacks)?;
    }
    Ok(report(proof, stacks, chosen.len()))
}

fn report(proof: &WorldProof, stacks: &[&dyn ProofStack], verified: usize) -> WorldReport {
    WorldReport {
        pulse: proof.pulse,
        leaves: proof.statements.len(),
        verified_leaves: verified,
        stacks: stacks.iter().map(|s| s.name().to_string()).collect(),
        simulated: stacks.iter().any(|s| s.simulated()),
        bytes: proof.size(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::replay::ReplayStack;
    use fold_core::ids::AssetId;
    use fold_core::Amount;
    use fold_exec::sim::Chain;
    use fold_exec::TxKind;

    fn proven_chain() -> (Chain, WorldProof) {
        let mut chain = Chain::new(&["a", "b", "c"], 1_000, 3);
        chain.chunk_size = 2;
        let (b, c) = (chain.id("b"), chain.id("c"));
        let mut txs = Vec::new();
        for (to, amount) in [(b, 5), (c, 7), (b, 9)] {
            txs.push(
                chain
                    .tx(
                        "a",
                        TxKind::Transfer {
                            asset: AssetId::usd(),
                            outputs: vec![(to, Amount::units(amount))],
                        },
                    )
                    .unwrap(),
            );
        }
        chain.pulse(txs).unwrap();
        let record = chain.history.last().unwrap().clone();
        let proof = prove_pulse(&record.output, &record.traces, &[&ReplayStack]).unwrap();
        (chain, proof)
    }

    #[test]
    fn world_proofs_verify_and_bind_receipts() {
        let (_, proof) = proven_chain();
        assert_eq!(
            proof.statements.len(),
            3,
            "two transaction chunks and the deferred phase"
        );
        let report = verify_world_proof(&proof, &[&ReplayStack]).unwrap();
        assert!(report.simulated);
        assert_eq!(report.verified_leaves, 3);
        let sampled = verify_world_proof_sampled(&proof, &[&ReplayStack], 1, 9).unwrap();
        assert_eq!(sampled.verified_leaves, 1);
    }

    #[test]
    fn tampering_is_detected() {
        let (_, proof) = proven_chain();
        let mut forged = proof.clone();
        forged.after.state_root = Hash32::ZERO;
        assert!(verify_world_proof(&forged, &[&ReplayStack]).is_err());

        let mut dropped = proof.clone();
        dropped.receipt_leaves.pop();
        assert!(verify_world_proof(&dropped, &[&ReplayStack]).is_err());

        let mut reordered = proof.clone();
        reordered.statements.swap(0, 1);
        assert!(verify_world_proof(&reordered, &[&ReplayStack]).is_err());

        let mut bad_leaf = proof.clone();
        bad_leaf.proofs[0][0].body.truncate(10);
        assert!(verify_world_proof(&bad_leaf, &[&ReplayStack]).is_err());
    }
}
