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
//!   The committees come from the validator set in force when the Pulse
//!   began, which the proof carries with a state proof against its
//!   starting state root: every committee change follows the staking rules.

use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use fold_consensus::lanes::canonical_inputs;
use fold_consensus::{
    verify_lane_certificate, LaneCertificate, ObjectRef as LaneRef, SignatureVerifier,
};
use fold_core::encode::encode;
use fold_core::hash::{hash_parts, Hash32};
use fold_core::ids::{Pulse, ShardId, TxDigest};
use fold_core::merkle;
use fold_core::smt::{verify_proof, SmtProof};
use fold_da::{DaHeader, ExtendedSquare};
use fold_exec::object::well_known;
use fold_exec::pulse::{
    chain_effects_leaf, effects_leaf, steps_digest_of, ChunkStatement, ChunkTrace, PulseOutput,
};
use fold_exec::validators::ValidatorSet;
use fold_exec::{Object, ObjectData};
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
    /// Forced intents posted in the Pulse, in posting order (not executed:
    /// each became an obligation of its sender's shard).
    pub forced: Vec<Transaction>,
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

    /// Digests of the posted forced intents, in order.
    pub fn forced_digests(&self) -> Vec<TxDigest> {
        self.forced.iter().map(Transaction::digest).collect()
    }
}

/// The validator set in force when a Pulse began, with its state proof.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct ValidatorsWitness {
    /// The validator-set object.
    pub object: Object,
    /// Its proof against the Pulse's starting state root.
    pub proof: SmtProof,
}

impl ValidatorsWitness {
    /// The witness for the validator set of `state` (taken before the
    /// Pulse executes).
    pub fn of(state: &fold_exec::State) -> Option<ValidatorsWitness> {
        let id = well_known::validators();
        Some(ValidatorsWitness {
            object: state.objects.get(&id)?.clone(),
            proof: state.tree.snapshot().prove(&id.0),
        })
    }

    /// The validator set, checked against `state_root`.
    pub fn verify(&self, state_root: &Hash32) -> Result<ValidatorSet, ProveError> {
        let id = well_known::validators();
        if self.object.id != id {
            return Err(ProveError::Chain(
                "the witness is not the validator set".into(),
            ));
        }
        verify_proof(state_root, &id.0, Some(&self.object.digest()), &self.proof)
            .map_err(|e| ProveError::Chain(format!("validator set: {e}")))?;
        match &self.object.data {
            ObjectData::Validators(set) => Ok((**set).clone()),
            _ => Err(ProveError::Chain(
                "the validator-set object holds something else".into(),
            )),
        }
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
    /// The validator set when the Pulse began (the committees that had to
    /// sign the certificates).
    pub validators: ValidatorsWitness,
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
/// in execution order; `validators` is the validator set when it began.
pub fn prove_pulse(
    output: &PulseOutput,
    traces: &[ChunkTrace],
    stacks: &[&dyn ProofStack],
    data: PulseData,
    certificates: Vec<LaneCertificate>,
    validators: ValidatorsWitness,
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
        validators,
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
    if proof.before.pulse.0.checked_add(1) != Some(proof.pulse.0)
        || proof.after.pulse != proof.pulse
    {
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
    // The deferred phase posts exactly the published forced intents.
    let last = proof.statements.last().expect("checked by check_structure");
    if steps_digest_of(&data.forced_digests()) != last.steps {
        return Err(ProveError::Chain(
            "the deferred phase posted other intents than the data publishes".into(),
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
/// its shard's committee covering exactly its owned inputs. The committees
/// are those of the validator set the proof carries, checked against its
/// starting state root, on chain `chain`; signatures are checked with the
/// keys the set registers, or with `verifier` if given (a devnet that signs
/// with the insecure simulation scheme).
pub fn verify_certificates(
    proof: &WorldProof,
    chain: Hash32,
    shard_count: u16,
    verifier: Option<&dyn SignatureVerifier>,
) -> Result<(), ProveError> {
    if shard_count == 0 {
        return Err(ProveError::Chain("the chain has no shards".into()));
    }
    if proof.certificates.len() != proof.data.transactions.len() {
        return Err(ProveError::Chain("one certificate per transaction".into()));
    }
    let set = proof.validators.verify(&proof.before.state_root)?;
    let committees: BTreeMap<ShardId, fold_consensus::Committee> = set
        .lane_committees(chain, shard_count)
        .map_err(|e| ProveError::Chain(format!("committees: {e}")))?;
    let registered = set.verifier();
    let verifier: &dyn SignatureVerifier = match verifier {
        Some(verifier) => verifier,
        None => &registered,
    };
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

    fn proven_chain() -> (Chain, fold_exec::State, WorldProof) {
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
        let before = chain.state.clone();
        chain.pulse(txs).unwrap();
        let record = chain.history.last().unwrap().clone();
        let proof = prove_pulse(
            &record.output,
            &record.traces,
            &[&ReplayStack],
            data_of(&record),
            Vec::new(),
            ValidatorsWitness::of(&before).unwrap(),
        )
        .unwrap();
        (chain, before, proof)
    }

    /// The data a sim Pulse publishes: the transaction chunks' bodies and
    /// the forced intents the deferred phase posted.
    fn data_of(record: &fold_exec::sim::PulseRecord) -> PulseData {
        let bodies = |deferred: bool| -> Vec<Transaction> {
            record
                .traces
                .iter()
                .filter(|t| t.statement.deferred_phase == deferred)
                .flat_map(|t| t.transactions.iter().map(|s| s.tx.clone()))
                .collect()
        };
        PulseData {
            pulse: record.output.pulse,
            beacon: record.output.beacon,
            transactions: bodies(false),
            forced: bodies(true),
        }
    }

    #[test]
    fn world_proofs_verify_and_bind_receipts_and_data() {
        let (_, _, proof) = proven_chain();
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
        let (_, _, proof) = proven_chain();
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
        assert!(
            verify_certificates(&no_certificates, Hash32::ZERO, 1, Some(&NoKeys)).is_err(),
            "every transaction needs a certificate"
        );

        // The validator set is bound to the state the Pulse started from.
        assert!(proof.validators.verify(&proof.before.state_root).is_ok());
        assert!(proof.validators.verify(&proof.after.state_root).is_err());
        let mut other_set = proof.clone();
        if let ObjectData::Validators(set) = &mut other_set.validators.object.data {
            set.active.clear();
        }
        assert!(
            verify_certificates(&other_set, Hash32::ZERO, 1, Some(&NoKeys)).is_err(),
            "a substituted validator set fails its state proof"
        );
    }

    #[test]
    fn the_deferred_phase_posts_exactly_the_published_intents() {
        let (mut chain, _, _) = proven_chain();
        // A transfer the gateway never saw, posted straight to consensus.
        let c = chain.id("c");
        let intent = chain
            .tx(
                "b",
                TxKind::Transfer {
                    asset: AssetId::usd(),
                    outputs: vec![(c, Amount::units(1))],
                },
            )
            .unwrap();
        let before = chain.state.clone();
        chain
            .pulse_forced(Vec::new(), Vec::new(), vec![intent.clone()])
            .unwrap();
        let record = chain.history.last().unwrap().clone();
        let data = data_of(&record);
        assert_eq!(data.forced, vec![intent.tx.clone()]);
        let witness = ValidatorsWitness::of(&before).unwrap();
        let proof = prove_pulse(
            &record.output,
            &record.traces,
            &[&ReplayStack],
            data.clone(),
            Vec::new(),
            witness.clone(),
        )
        .unwrap();
        assert!(verify_world_proof(&proof, &[&ReplayStack]).is_ok());

        // Hiding the intent from the published data is caught.
        let mut hidden = data;
        hidden.forced.clear();
        assert!(prove_pulse(
            &record.output,
            &record.traces,
            &[&ReplayStack],
            hidden,
            Vec::new(),
            witness,
        )
        .is_err());
    }

    /// Found by the security review: a forged proof for Pulse u64::MAX made
    /// the structure check overflow instead of failing.
    #[test]
    fn forged_pulse_numbers_fail_cleanly() {
        let (_, _, mut proof) = proven_chain();
        proof.before.pulse = Pulse(u64::MAX);
        assert!(verify_world_proof(&proof, &[&ReplayStack]).is_err());
        let (_, _, proof) = proven_chain();
        assert!(verify_certificates(&proof, Hash32::ZERO, 0, Some(&NoKeys)).is_err());
    }

    #[test]
    fn both_stacks_must_accept_every_leaf() {
        use crate::spec::SpecStack;
        let (chain, before, _) = proven_chain();
        let record = chain.history.last().unwrap().clone();
        let both: [&dyn ProofStack; 2] = [&ReplayStack, &SpecStack];
        let proof = prove_pulse(
            &record.output,
            &record.traces,
            &both,
            data_of(&record),
            Vec::new(),
            ValidatorsWitness::of(&before).unwrap(),
        )
        .unwrap();
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

        // The deferred phase posts exactly the published forced intents
        // (none here).
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
