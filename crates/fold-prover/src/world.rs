//! Folding leaf proofs into a World Proof.
//!
//! A World Proof for Pulse `t` establishes that the transition from the
//! world before to the world after is valid, and binds three things a light
//! client relies on:
//!
//! - **Execution**: the chunk statements chain from `before` to `after`, the
//!   deferred phase consumes exactly the effects the transactions deferred,
//!   and every leaf verifies under every stack.
//! - **Data**: the Pulse's published data (transaction bodies in execution
//!   order) is exactly what the statements executed, and its
//!   two-dimensional Reed–Solomon encoding has the committed data root — so
//!   availability sampling against that root samples a *correct* encoding
//!   (the property ZODA-style designs prove inside the World Proof).
//! - **Certificates**: every executed transaction carried a lane certificate
//!   from its shard's committee locking its owned inputs
//!   ([`verify_certificates`]), so nothing certified final was overtaken.

use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use fold_consensus::lanes::canonical_inputs;
use fold_consensus::{
    verify_lane_certificate, Committee, LaneCertificate, ObjectRef as LaneRef, SignatureVerifier,
};
use fold_core::encode::encode;
use fold_core::hash::{hash_parts, Hash32};
use fold_core::ids::{Pulse, ShardId, TxDigest};
use fold_core::merkle;
use fold_da::{DaHeader, ExtendedSquare};
use fold_exec::pulse::{
    chain_effects_leaf, effects_leaf, steps_digest_of, ChunkStatement, ChunkTrace, PulseOutput,
};
use fold_exec::{Transaction, WorldCommitment};
use serde::{Deserialize, Serialize};

use crate::error::ProveError;
use crate::stack::{LeafProof, ProofStack};

/// The data a Pulse publishes: transaction bodies in execution order.
/// Signatures are verified by validators and proven, never published (a
/// dual post-quantum signature is twenty times larger than a transfer).
#[derive(
    Clone, Debug, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize,
)]
pub struct PulseData {
    /// The Pulse.
    pub pulse: Pulse,
    /// The Pulse's beacon.
    pub beacon: Hash32,
    /// Executed transactions, in execution order.
    pub transactions: Vec<Transaction>,
}

impl PulseData {
    /// The canonical bytes that are erasure-coded.
    pub fn blob(&self) -> Vec<u8> {
        encode(self)
    }

    /// The extended data square and its header.
    pub fn availability(&self) -> Result<(ExtendedSquare, DaHeader), ProveError> {
        let square = ExtendedSquare::encode(&self.blob())
            .map_err(|e| ProveError::Malformed(format!("data: {e}")))?;
        let header = square.header();
        Ok((square, header))
    }

    /// Digests of the transactions, in order.
    pub fn digests(&self) -> Vec<TxDigest> {
        self.transactions.iter().map(Transaction::digest).collect()
    }
}

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
    /// The Pulse's published data.
    pub data: PulseData,
    /// The data's availability root (2D Reed–Solomon, hash commitments).
    pub data_root: Hash32,
    /// Width `k` of the data square.
    pub data_k: u32,
    /// Transactions per transaction chunk (the deferred phase has none).
    pub chunk_sizes: Vec<u32>,
    /// One lane certificate per executed transaction, in execution order.
    pub certificates: Vec<LaneCertificate>,
}

impl WorldProof {
    /// Encoded size in bytes.
    pub fn size(&self) -> usize {
        encode(self).len()
    }

    /// The beacon every statement executed with (consumers check it against
    /// the certified header's).
    pub fn beacon(&self) -> Hash32 {
        self.statements
            .first()
            .map(|s| s.beacon)
            .unwrap_or(Hash32::ZERO)
    }

    /// Digest of what the proof establishes: (Pulse, before, after,
    /// receipts root, statements root, data root).
    pub fn claim_digest(&self) -> Hash32 {
        hash_parts(
            "fold/prover/world-claim",
            &[
                &self.pulse.0.to_be_bytes(),
                &self.before.digest().0,
                &self.after.digest().0,
                &self.receipts_root.0,
                &self.statements_root.0,
                &self.data_root.0,
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
    /// Transactions the Pulse executed.
    pub transactions: usize,
}

fn statements_root(statements: &[ChunkStatement]) -> Hash32 {
    let leaves: Vec<Hash32> = statements
        .iter()
        .map(|s| merkle::leaf_hash(&s.digest().0))
        .collect();
    merkle::root(&leaves)
}

/// Proves a Pulse with every given stack. `data` is the Pulse's published
/// data and `certificates` the lane certificates of its transactions, both
/// in execution order.
pub fn prove_pulse(
    output: &PulseOutput,
    traces: &[ChunkTrace],
    stacks: &[&dyn ProofStack],
    data: PulseData,
    certificates: Vec<LaneCertificate>,
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
    let (_, header) = data.availability()?;
    let proof = WorldProof {
        pulse: output.pulse,
        before: output.before,
        after: output.after,
        receipts_root: output.receipts_root,
        receipt_leaves: output.effects.iter().map(effects_leaf).collect(),
        statements_root: statements_root(&statements),
        statements,
        proofs,
        data,
        data_root: header.data_root,
        data_k: header.k,
        chunk_sizes: traces
            .iter()
            .filter(|t| !t.statement.deferred_phase)
            .map(|t| t.transactions.len() as u32)
            .collect(),
        certificates,
    };
    check_structure(&proof)?;
    check_data(&proof)?;
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
        if statement.deferred_phase && statement.steps != steps_digest_of(&[]) {
            return Err(ProveError::Chain(
                "the deferred phase executes no transactions".into(),
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

/// Checks that the published data is exactly what the statements executed
/// and that its encoding has the committed data root.
fn check_data(proof: &WorldProof) -> Result<(), ProveError> {
    let data = &proof.data;
    let first = proof
        .statements
        .first()
        .expect("checked by check_structure");
    if data.pulse != proof.pulse || data.beacon != first.beacon {
        return Err(ProveError::Chain("data is for another Pulse".into()));
    }
    let digests = data.digests();
    if digests.len() != proof.receipt_leaves.len() {
        return Err(ProveError::Chain(
            "one receipt per published transaction".into(),
        ));
    }
    if proof.chunk_sizes.len() + 1 != proof.statements.len() {
        return Err(ProveError::Chain("one size per transaction chunk".into()));
    }
    let mut offset = 0usize;
    for (statement, size) in proof.statements.iter().zip(&proof.chunk_sizes) {
        let end = offset + *size as usize;
        let chunk = digests
            .get(offset..end)
            .ok_or_else(|| ProveError::Chain("chunk sizes exceed the data".into()))?;
        if steps_digest_of(chunk) != statement.steps {
            return Err(ProveError::Chain(format!(
                "chunk {} executed other transactions than the data publishes",
                statement.index
            )));
        }
        offset = end;
    }
    if offset != digests.len() {
        return Err(ProveError::Chain(
            "data publishes unexecuted transactions".into(),
        ));
    }
    let (_, header) = data.availability()?;
    if header.data_root != proof.data_root || header.k != proof.data_k {
        return Err(ProveError::Chain(
            "data root does not match the encoding".into(),
        ));
    }
    Ok(())
}

/// Checks that every executed transaction carried a lane certificate from
/// its shard's committee covering exactly its owned inputs.
pub fn verify_certificates<V: SignatureVerifier + ?Sized>(
    proof: &WorldProof,
    committees: &BTreeMap<ShardId, Committee>,
    shard_count: u16,
    verifier: &V,
) -> Result<(), ProveError> {
    if proof.certificates.len() != proof.data.transactions.len() {
        return Err(ProveError::Chain("one certificate per transaction".into()));
    }
    for (tx, certificate) in proof.data.transactions.iter().zip(&proof.certificates) {
        let shard = ShardId::for_account(&tx.sender, shard_count);
        let committee = committees
            .get(&shard)
            .ok_or_else(|| ProveError::Chain(format!("no committee for shard {}", shard.0)))?;
        let inputs: Vec<LaneRef> = tx
            .inputs
            .iter()
            .map(|r| LaneRef {
                id: r.id,
                version: r.version,
            })
            .collect();
        verify_lane_certificate(
            committee,
            &tx.digest(),
            &canonical_inputs(&inputs),
            certificate,
            &Verifier(verifier),
        )
        .map_err(|e| ProveError::Chain(format!("certificate of {}: {e}", tx.digest())))?;
    }
    Ok(())
}

/// Adapts an unsized verifier to the sized one the lane check expects.
struct Verifier<'a, V: SignatureVerifier + ?Sized>(&'a V);

impl<V: SignatureVerifier + ?Sized> SignatureVerifier for Verifier<'_, V> {
    fn verify(
        &self,
        validator: &fold_core::ids::ValidatorId,
        message: &[u8],
        signature: &[u8],
    ) -> bool {
        self.0.verify(validator, message, signature)
    }
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

/// Verifies a World Proof: its structure, its data binding, and every leaf
/// under every required stack. (Certificates need the committees:
/// [`verify_certificates`].)
pub fn verify_world_proof(
    proof: &WorldProof,
    stacks: &[&dyn ProofStack],
) -> Result<WorldReport, ProveError> {
    check_structure(proof)?;
    check_data(proof)?;
    for index in 0..proof.statements.len() {
        verify_leaf(proof, index, stacks)?;
    }
    Ok(report(proof, stacks, proof.statements.len()))
}

/// Verifies a World Proof's structure and data binding fully, and `samples`
/// leaves chosen by `seed` (always including the deferred phase) under
/// every required stack.
pub fn verify_world_proof_sampled(
    proof: &WorldProof,
    stacks: &[&dyn ProofStack],
    samples: usize,
    seed: u64,
) -> Result<WorldReport, ProveError> {
    check_structure(proof)?;
    check_data(proof)?;
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
        transactions: proof.data.transactions.len(),
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
        let transactions = record
            .traces
            .iter()
            .flat_map(|t| t.transactions.iter().map(|s| s.tx.clone()))
            .collect();
        let data = PulseData {
            pulse: record.output.pulse,
            beacon: record.output.beacon,
            transactions,
        };
        let proof = prove_pulse(
            &record.output,
            &record.traces,
            &[&ReplayStack],
            data,
            Vec::new(),
        )
        .unwrap();
        (chain, proof)
    }

    #[test]
    fn world_proofs_verify_and_bind_receipts_and_data() {
        let (_, proof) = proven_chain();
        assert_eq!(
            proof.statements.len(),
            3,
            "two transaction chunks and the deferred phase"
        );
        let report = verify_world_proof(&proof, &[&ReplayStack]).unwrap();
        assert!(report.simulated);
        assert_eq!(report.verified_leaves, 3);
        assert_eq!(report.transactions, 3);
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

        let mut other_data = proof.clone();
        other_data.data.transactions.swap(0, 1);
        assert!(
            verify_world_proof(&other_data, &[&ReplayStack]).is_err(),
            "published data must match execution"
        );

        let mut wrong_root = proof.clone();
        wrong_root.data_root = Hash32::ZERO;
        assert!(
            verify_world_proof(&wrong_root, &[&ReplayStack]).is_err(),
            "data root must match the encoding"
        );

        let mut no_certificates = proof.clone();
        no_certificates.certificates.clear();
        assert!(verify_certificates(&no_certificates, &BTreeMap::new(), 1, &NoKeys).is_err());
    }

    #[test]
    fn both_stacks_must_accept_every_leaf() {
        use crate::spec::SpecStack;
        let (chain, _) = proven_chain();
        let record = chain.history.last().unwrap().clone();
        let data = PulseData {
            pulse: record.output.pulse,
            beacon: record.output.beacon,
            transactions: record
                .traces
                .iter()
                .flat_map(|t| t.transactions.iter().map(|s| s.tx.clone()))
                .collect(),
        };
        let both: [&dyn ProofStack; 2] = [&ReplayStack, &SpecStack];
        let proof = prove_pulse(&record.output, &record.traces, &both, data, Vec::new()).unwrap();
        let report = verify_world_proof(&proof, &both).unwrap();
        assert_eq!(report.stacks, vec!["A".to_string(), "B".to_string()]);

        // Without stack B's leaves, a verifier that requires both refuses.
        let mut only_a = proof.clone();
        for leaves in &mut only_a.proofs {
            leaves.retain(|leaf| leaf.stack == "A");
        }
        assert!(verify_world_proof(&only_a, &both).is_err());

        // A broken stack-B leaf is caught even though stack A's verifies.
        let mut bad_b = proof.clone();
        let leaf = bad_b.proofs[0]
            .iter_mut()
            .find(|leaf| leaf.stack == "B")
            .unwrap();
        leaf.body.truncate(leaf.body.len() / 2);
        assert!(verify_world_proof(&bad_b, &[&ReplayStack]).is_ok());
        assert!(verify_world_proof(&bad_b, &both).is_err());

        // The deferred phase executes no transactions.
        let mut steps = proof;
        steps.statements.last_mut().unwrap().steps = Hash32::ZERO;
        assert!(verify_world_proof(&steps, &both).is_err());
    }

    struct NoKeys;

    impl SignatureVerifier for NoKeys {
        fn verify(&self, _: &fold_core::ids::ValidatorId, _: &[u8], _: &[u8]) -> bool {
            false
        }
    }
}
