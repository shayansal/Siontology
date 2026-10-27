//! Linear Lanes: consensus-free certificates for owned objects.
//!
//! An owned object version can be spent only by its owner, so the only
//! thing that must be prevented is the owner signing two spends of the same
//! version. Each validator therefore approves an `(object, version)` for at
//! most one transaction digest, and a transaction is final once validators
//! holding more than two thirds of the stake approve it: two such sets share
//! an honest validator, who approved only one of the conflicting
//! transactions. No ordering of unrelated traffic is needed (FastPay, Sui's
//! owned objects; Guerraoui et al. show this needs only reliable broadcast).
//!
//! **Batch-root voting.** Post-quantum signatures are several kilobytes, so
//! a validator does not sign each approval. It accumulates approvals and
//! signs one Merkle root over the batch; an [`ApprovalProof`] is that one
//! signature plus a logarithmic inclusion path. At 100 validators this turns
//! about 540 KB of dual signatures per certificate into one signature per
//! validator per batch interval, at the cost of one batch interval of latency.

use std::collections::{BTreeMap, BTreeSet};

use borsh::{BorshDeserialize, BorshSerialize};
use fold_core::{
    encode, hash_parts, merkle, Epoch, Hash32, ObjectId, ShardId, TxDigest, ValidatorId, Version,
};
use serde::{Deserialize, Serialize};

use crate::auth::{signing_digest, Authenticator, SignatureVerifier};
use crate::committee::Committee;
use crate::error::ConsensusError;

/// A specific version of an owned object.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    BorshSerialize,
    BorshDeserialize,
    Serialize,
    Deserialize,
)]
pub struct ObjectRef {
    /// The object.
    pub id: ObjectId,
    /// The version being spent.
    pub version: Version,
}

/// A request to lock owned inputs to a transaction. `owner_proof` is the
/// owner's authorization (in production, the signed transaction), checked by
/// an [`OwnerAuthorization`].
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct LaneRequest {
    /// The transaction digest.
    pub tx: TxDigest,
    /// The owned inputs it spends.
    pub inputs: Vec<ObjectRef>,
    /// Opaque proof that the owner authorized this transaction.
    pub owner_proof: Vec<u8>,
}

impl LaneRequest {
    /// The inputs sorted and deduplicated.
    pub fn canonical_inputs(&self) -> Vec<ObjectRef> {
        canonical_inputs(&self.inputs)
    }
}

/// Checks that a request was authorized by the owner of its inputs.
pub trait OwnerAuthorization {
    /// Whether the owner authorized `request`.
    fn authorizes(&self, request: &LaneRequest) -> bool;
}

/// Sorts and deduplicates inputs.
pub fn canonical_inputs(inputs: &[ObjectRef]) -> Vec<ObjectRef> {
    let set: BTreeSet<ObjectRef> = inputs.iter().copied().collect();
    set.into_iter().collect()
}

/// The Merkle leaf for one approval.
pub fn approval_leaf(tx: &TxDigest, inputs: &[ObjectRef]) -> Hash32 {
    let digest = hash_parts(
        "fold/lanes/approval",
        &[&tx.0 .0, &encode(&canonical_inputs(inputs))],
    );
    merkle::leaf_hash(&digest.0)
}

/// The signed part of a batch.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct BatchHeader {
    /// The approving validator.
    pub validator: ValidatorId,
    /// The chain ([`Committee::chain`]).
    pub chain: Hash32,
    /// The committee's epoch.
    pub epoch: Epoch,
    /// The committee's shard.
    pub shard: ShardId,
    /// The batch's sequence number for this validator.
    pub index: u64,
    /// Merkle root over the batch's approval leaves.
    pub root: Hash32,
    /// Number of approvals in the batch.
    pub size: u64,
}

impl BatchHeader {
    /// The digest the validator signs.
    pub fn signing_digest(&self) -> Hash32 {
        signing_digest("fold/lanes/batch", &[&encode(self)])
    }
}

/// A sealed, signed batch of approvals.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct SignedBatch {
    /// The batch header.
    pub header: BatchHeader,
    /// The validator's signature over the header digest.
    pub signature: Vec<u8>,
    /// The approvals, in leaf order.
    pub approvals: Vec<(TxDigest, Vec<ObjectRef>)>,
}

/// One validator's approval of one transaction: a batch signature plus an
/// inclusion path.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct ApprovalProof {
    /// The signed batch header.
    pub batch: BatchHeader,
    /// Signature over the batch header.
    pub signature: Vec<u8>,
    /// Index of this approval in the batch.
    pub leaf_index: u64,
    /// Inclusion path to the batch root.
    pub path: Vec<Hash32>,
}

impl ApprovalProof {
    /// Checks the signature and that the batch includes an approval of
    /// `tx` spending `inputs`.
    pub fn verify<V: SignatureVerifier>(
        &self,
        committee: &Committee,
        tx: &TxDigest,
        inputs: &[ObjectRef],
        verifier: &V,
    ) -> Result<(), ConsensusError> {
        let batch = &self.batch;
        if batch.chain != committee.chain()
            || batch.epoch != committee.epoch()
            || batch.shard != committee.shard()
        {
            return Err(ConsensusError::Mismatch(
                "approval is for another committee",
            ));
        }
        if !committee.contains(&batch.validator) {
            return Err(ConsensusError::UnknownValidator(batch.validator));
        }
        if !verifier.verify(&batch.validator, &batch.signing_digest().0, &self.signature) {
            return Err(ConsensusError::InvalidSignature(batch.validator));
        }
        let leaf = approval_leaf(tx, inputs);
        merkle::verify_inclusion(&batch.root, batch.size, self.leaf_index, &leaf, &self.path)
            .map_err(|e| ConsensusError::InvalidProof(e.to_string()))
    }
}

/// A transaction's lane certificate: approvals from validators holding more
/// than two thirds of the committee's stake.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct LaneCertificate {
    /// The certified transaction.
    pub tx: TxDigest,
    /// Its owned inputs, canonical order.
    pub inputs: Vec<ObjectRef>,
    /// One approval per validator.
    pub approvals: Vec<ApprovalProof>,
}

impl LaneCertificate {
    /// The approving validators.
    pub fn signers(&self) -> BTreeSet<ValidatorId> {
        self.approvals.iter().map(|a| a.batch.validator).collect()
    }
}

/// Verifies a lane certificate for `tx` spending `inputs`; returns the
/// approving stake.
pub fn verify_lane_certificate<V: SignatureVerifier>(
    committee: &Committee,
    tx: &TxDigest,
    inputs: &[ObjectRef],
    certificate: &LaneCertificate,
    verifier: &V,
) -> Result<u64, ConsensusError> {
    if certificate.tx != *tx {
        return Err(ConsensusError::Mismatch(
            "certificate is for another transaction",
        ));
    }
    let inputs = canonical_inputs(inputs);
    if inputs.is_empty() {
        return Err(ConsensusError::NoInputs);
    }
    if canonical_inputs(&certificate.inputs) != inputs {
        return Err(ConsensusError::Mismatch("certificate covers other inputs"));
    }
    let mut seen = BTreeSet::new();
    let mut stake = 0u64;
    for approval in &certificate.approvals {
        let validator = approval.batch.validator;
        if !seen.insert(validator) {
            return Err(ConsensusError::DuplicateSigner(validator));
        }
        approval.verify(committee, tx, &inputs, verifier)?;
        stake += committee.stake_of(&validator);
    }
    if !committee.is_quorum(stake) {
        return Err(ConsensusError::InsufficientStake {
            have: stake,
            need: committee.quorum_threshold(),
        });
    }
    Ok(stake)
}

/// Evidence that an owner authorized two transactions spending the same
/// object version. The object stays locked until the epoch ends.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct OwnerEquivocation {
    /// The contested object version.
    pub input: ObjectRef,
    /// The request the validator locked first.
    pub first: LaneRequest,
    /// The conflicting request.
    pub second: LaneRequest,
}

impl OwnerEquivocation {
    /// Checks that both requests are owner-authorized, differ, and spend
    /// the contested version.
    pub fn verify<O: OwnerAuthorization>(&self, owners: &O) -> bool {
        self.first.tx != self.second.tx
            && self.first.inputs.contains(&self.input)
            && self.second.inputs.contains(&self.input)
            && owners.authorizes(&self.first)
            && owners.authorizes(&self.second)
    }
}

/// Evidence that a validator approved two transactions spending the same
/// object version: slashable, because honest validators never do this.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct ValidatorEquivocation {
    /// The offending validator.
    pub validator: ValidatorId,
    /// The contested object version.
    pub input: ObjectRef,
    /// The first approved transaction and its inputs.
    pub first: (TxDigest, Vec<ObjectRef>, ApprovalProof),
    /// The second approved transaction and its inputs.
    pub second: (TxDigest, Vec<ObjectRef>, ApprovalProof),
}

impl ValidatorEquivocation {
    /// Checks both approvals and that they conflict.
    pub fn verify<V: SignatureVerifier>(&self, committee: &Committee, verifier: &V) -> bool {
        let (tx_a, inputs_a, proof_a) = &self.first;
        let (tx_b, inputs_b, proof_b) = &self.second;
        tx_a != tx_b
            && proof_a.batch.validator == self.validator
            && proof_b.batch.validator == self.validator
            && inputs_a.contains(&self.input)
            && inputs_b.contains(&self.input)
            && proof_a.verify(committee, tx_a, inputs_a, verifier).is_ok()
            && proof_b.verify(committee, tx_b, inputs_b, verifier).is_ok()
    }
}

/// Given two certificates for conflicting transactions, returns evidence
/// against every validator that approved both. Under quorum intersection
/// this list is never empty, and with Byzantine stake within tolerance the
/// two certificates cannot both exist in the first place.
pub fn find_conflicting_approvals(
    a: &LaneCertificate,
    b: &LaneCertificate,
) -> Vec<ValidatorEquivocation> {
    if a.tx == b.tx {
        return Vec::new();
    }
    let inputs_b: BTreeSet<ObjectRef> = b.inputs.iter().copied().collect();
    let Some(input) = a.inputs.iter().find(|i| inputs_b.contains(i)).copied() else {
        return Vec::new();
    };
    let by_validator_b: BTreeMap<ValidatorId, &ApprovalProof> =
        b.approvals.iter().map(|p| (p.batch.validator, p)).collect();
    a.approvals
        .iter()
        .filter_map(|proof_a| {
            by_validator_b
                .get(&proof_a.batch.validator)
                .map(|proof_b| ValidatorEquivocation {
                    validator: proof_a.batch.validator,
                    input,
                    first: (a.tx, a.inputs.clone(), proof_a.clone()),
                    second: (b.tx, b.inputs.clone(), (*proof_b).clone()),
                })
        })
        .collect()
}

/// Why a lane request was rejected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaneRejection {
    /// The reason.
    pub error: ConsensusError,
    /// Evidence of owner equivocation, when the rejection was a conflict.
    pub evidence: Option<Box<OwnerEquivocation>>,
}

/// The state of an accepted request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApprovalStatus {
    /// Approved and waiting for the next sealed batch.
    Pending,
    /// Approved and sealed; the proof can be sent to the client.
    Sealed(ApprovalProof),
}

#[derive(Clone, Debug)]
struct Lock {
    tx: TxDigest,
    /// One shared copy per transaction, however many inputs it locks.
    request: std::sync::Arc<LaneRequest>,
}

/// One validator's lane state for one committee.
#[derive(Debug)]
pub struct LaneValidator<A: Authenticator> {
    chain: Hash32,
    epoch: Epoch,
    shard: ShardId,
    authenticator: A,
    locks: BTreeMap<ObjectRef, Lock>,
    approved: BTreeMap<TxDigest, Vec<ObjectRef>>,
    pending: Vec<(TxDigest, Vec<ObjectRef>)>,
    proofs: BTreeMap<TxDigest, ApprovalProof>,
    next_index: u64,
}

impl<A: Authenticator> LaneValidator<A> {
    /// Creates lane state for `committee`, signing with `authenticator`.
    pub fn new(committee: &Committee, authenticator: A) -> Self {
        LaneValidator {
            chain: committee.chain(),
            epoch: committee.epoch(),
            shard: committee.shard(),
            authenticator,
            locks: BTreeMap::new(),
            approved: BTreeMap::new(),
            pending: Vec::new(),
            proofs: BTreeMap::new(),
            next_index: 0,
        }
    }

    /// This validator's identity.
    pub fn validator(&self) -> ValidatorId {
        self.authenticator.validator()
    }

    /// Locks every input to the request's transaction, or rejects the whole
    /// request if any input is locked to a different transaction.
    /// Re-submitting an approved transaction is idempotent.
    pub fn approve<O: OwnerAuthorization>(
        &mut self,
        request: LaneRequest,
        owners: &O,
    ) -> Result<ApprovalStatus, LaneRejection> {
        let inputs = request.canonical_inputs();
        if inputs.is_empty() {
            return Err(LaneRejection {
                error: ConsensusError::NoInputs,
                evidence: None,
            });
        }
        if let Some(existing) = self.approved.get(&request.tx) {
            if *existing == inputs {
                // A lock restored after a restart has no approval yet in this
                // epoch: sign it again in the next batch.
                if !self.proofs.contains_key(&request.tx)
                    && !self.pending.iter().any(|(tx, _)| *tx == request.tx)
                {
                    self.pending.push((request.tx, inputs));
                }
                return Ok(self.status(&request.tx));
            }
            return Err(LaneRejection {
                error: ConsensusError::Mismatch("same transaction, different inputs"),
                evidence: None,
            });
        }
        if !owners.authorizes(&request) {
            return Err(LaneRejection {
                error: ConsensusError::Malformed("owner did not authorize the request"),
                evidence: None,
            });
        }
        for input in &inputs {
            if let Some(lock) = self.locks.get(input) {
                if lock.tx != request.tx {
                    return Err(LaneRejection {
                        error: ConsensusError::LockConflict {
                            input: *input,
                            existing: lock.tx,
                        },
                        evidence: Some(Box::new(OwnerEquivocation {
                            input: *input,
                            first: (*lock.request).clone(),
                            second: request.clone(),
                        })),
                    });
                }
            }
        }
        let shared = std::sync::Arc::new(request.clone());
        for input in &inputs {
            self.locks.insert(
                *input,
                Lock {
                    tx: request.tx,
                    request: shared.clone(),
                },
            );
        }
        self.approved.insert(request.tx, inputs.clone());
        self.pending.push((request.tx, inputs));
        Ok(ApprovalStatus::Pending)
    }

    fn status(&self, tx: &TxDigest) -> ApprovalStatus {
        match self.proofs.get(tx) {
            Some(proof) => ApprovalStatus::Sealed(proof.clone()),
            None => ApprovalStatus::Pending,
        }
    }

    /// Seals pending approvals into one signed batch; `None` if nothing is
    /// pending.
    pub fn seal_batch(&mut self) -> Option<SignedBatch> {
        if self.pending.is_empty() {
            return None;
        }
        let approvals = std::mem::take(&mut self.pending);
        let leaves: Vec<Hash32> = approvals
            .iter()
            .map(|(tx, inputs)| approval_leaf(tx, inputs))
            .collect();
        let header = BatchHeader {
            validator: self.authenticator.validator(),
            chain: self.chain,
            epoch: self.epoch,
            shard: self.shard,
            index: self.next_index,
            root: merkle::root(&leaves),
            size: leaves.len() as u64,
        };
        self.next_index += 1;
        let signature = self.authenticator.sign(&header.signing_digest().0);
        for (index, (tx, _)) in approvals.iter().enumerate() {
            let path = merkle::inclusion_proof(&leaves, index).expect("index within batch");
            self.proofs.insert(
                *tx,
                ApprovalProof {
                    batch: header.clone(),
                    signature: signature.clone(),
                    leaf_index: index as u64,
                    path,
                },
            );
        }
        Some(SignedBatch {
            header,
            signature,
            approvals,
        })
    }

    /// Forgets a transaction: its locks, its approval and its proof. Safe
    /// once the transaction has executed (its input versions are gone) or
    /// can no longer execute (it expired), and needed then, or its inputs
    /// would stay frozen.
    pub fn release(&mut self, tx: &TxDigest) {
        if let Some(inputs) = self.approved.remove(tx) {
            for input in inputs {
                if self.locks.get(&input).map(|l| l.tx) == Some(*tx) {
                    self.locks.remove(&input);
                }
            }
        }
        self.pending.retain(|(pending, _)| pending != tx);
        self.proofs.remove(tx);
    }

    /// Re-establishes a lock from durable storage after a restart, without
    /// signing anything: the inputs stay locked to `request`'s transaction,
    /// so no conflicting spend can be approved in the new epoch, and the
    /// same transaction is approved again if it is resubmitted. A lock that
    /// conflicts with one already held is skipped.
    pub fn restore(&mut self, request: LaneRequest) {
        let inputs = request.canonical_inputs();
        if inputs.is_empty()
            || self.approved.contains_key(&request.tx)
            || inputs.iter().any(|i| self.locks.contains_key(i))
        {
            return;
        }
        let shared = std::sync::Arc::new(request.clone());
        for input in &inputs {
            self.locks.insert(
                *input,
                Lock {
                    tx: request.tx,
                    request: shared.clone(),
                },
            );
        }
        self.approved.insert(request.tx, inputs);
    }

    /// The transactions holding locks, with their requests.
    pub fn locked_requests(&self) -> Vec<LaneRequest> {
        let mut seen = BTreeSet::new();
        self.locks
            .values()
            .filter(|lock| seen.insert(lock.tx))
            .map(|lock| (*lock.request).clone())
            .collect()
    }

    /// The sealed approval proof for `tx`, if any.
    pub fn approval_proof(&self, tx: &TxDigest) -> Option<&ApprovalProof> {
        self.proofs.get(tx)
    }

    /// The transaction an object version is locked to.
    pub fn lock_holder(&self, input: &ObjectRef) -> Option<TxDigest> {
        self.locks.get(input).map(|l| l.tx)
    }

    /// Number of locked object versions.
    pub fn locked_count(&self) -> usize {
        self.locks.len()
    }
}

/// Collects sealed approvals into a certificate once they reach a quorum.
#[derive(Clone, Debug)]
pub struct CertificateBuilder {
    tx: TxDigest,
    inputs: Vec<ObjectRef>,
    approvals: BTreeMap<ValidatorId, ApprovalProof>,
}

impl CertificateBuilder {
    /// Starts collecting approvals for `tx`.
    pub fn new(tx: TxDigest, inputs: &[ObjectRef]) -> Self {
        CertificateBuilder {
            tx,
            inputs: canonical_inputs(inputs),
            approvals: BTreeMap::new(),
        }
    }

    /// Adds a verified approval; returns the certificate once the approving
    /// stake reaches a quorum.
    pub fn add<V: SignatureVerifier>(
        &mut self,
        committee: &Committee,
        proof: ApprovalProof,
        verifier: &V,
    ) -> Result<Option<LaneCertificate>, ConsensusError> {
        proof.verify(committee, &self.tx, &self.inputs, verifier)?;
        self.approvals.insert(proof.batch.validator, proof);
        let stake = committee.stake_of_set(self.approvals.keys());
        if committee.is_quorum(stake) {
            Ok(Some(LaneCertificate {
                tx: self.tx,
                inputs: self.inputs.clone(),
                approvals: self.approvals.values().cloned().collect(),
            }))
        } else {
            Ok(None)
        }
    }
}
