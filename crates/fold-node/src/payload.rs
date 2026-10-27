//! What consensus orders.
//!
//! Consensus never orders the owned-path traffic itself: an owned
//! transaction is final once its lane certificate exists. Consensus only
//! fixes *which Pulse executes it*, alongside the contended transactions it
//! does order, and the beacon reveals. Every transaction, contended or not,
//! first locks its owned inputs with a lane certificate.
//!
//! **Batch-root voting.** A validator signs one Merkle root per batch of
//! approvals, so a certificate is three (or more) references into signed
//! batches. Certified transactions travel in groups that carry each signed
//! batch header once and each approval as a reference (batch, position,
//! path): a group of a hundred transactions costs a handful of dual
//! signatures instead of three hundred.

use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use fold_consensus::lanes::canonical_inputs;
use fold_consensus::{ApprovalProof, BatchHeader, LaneCertificate, ObjectRef as LaneRef};
use fold_core::encode::{decode, encode};
use fold_core::hash::Hash32;
use fold_core::ids::{Pulse, ValidatorId};
use fold_exec::{ObjectRef, SignedTransaction};

/// A validator's signed batch header.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct SignedBatchHeader {
    /// The header.
    pub header: BatchHeader,
    /// The validator's signature over its signing digest.
    pub signature: Vec<u8>,
}

/// One approval, by reference into a group's batch headers.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct ApprovalRef {
    /// Index of the batch header in the group.
    pub batch: u32,
    /// Position of the approval in the batch.
    pub leaf_index: u64,
    /// Inclusion path to the batch root.
    pub path: Vec<Hash32>,
}

/// A certified transaction inside a group.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct GroupedTx {
    /// The transaction.
    pub tx: SignedTransaction,
    /// Its approvals.
    pub approvals: Vec<ApprovalRef>,
}

/// One consensus payload item.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum PayloadItem {
    /// Certified transactions (owned-path or contended) sharing batch headers.
    Certified {
        /// Signed batch headers, each once.
        batches: Vec<SignedBatchHeader>,
        /// Transactions with their approvals.
        txs: Vec<GroupedTx>,
    },
    /// A validator's beacon reveal for a Pulse.
    Reveal {
        /// The validator.
        validator: ValidatorId,
        /// The Pulse.
        pulse: Pulse,
        /// The hash-chain preimage.
        value: Hash32,
    },
}

impl PayloadItem {
    /// Canonical bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        encode(self)
    }

    /// Parses canonical bytes.
    pub fn from_bytes(bytes: &[u8]) -> Option<PayloadItem> {
        decode(bytes).ok()
    }

    /// Groups certified transactions, sharing batch headers.
    pub fn group(certified: Vec<(SignedTransaction, LaneCertificate)>) -> PayloadItem {
        let mut index: BTreeMap<(ValidatorId, u64), u32> = BTreeMap::new();
        let mut batches = Vec::new();
        let mut txs = Vec::with_capacity(certified.len());
        for (tx, certificate) in certified {
            let mut approvals = Vec::with_capacity(certificate.approvals.len());
            for proof in certificate.approvals {
                let key = (proof.batch.validator, proof.batch.index);
                let position = *index.entry(key).or_insert_with(|| {
                    batches.push(SignedBatchHeader {
                        header: proof.batch.clone(),
                        signature: proof.signature.clone(),
                    });
                    (batches.len() - 1) as u32
                });
                approvals.push(ApprovalRef {
                    batch: position,
                    leaf_index: proof.leaf_index,
                    path: proof.path,
                });
            }
            txs.push(GroupedTx { tx, approvals });
        }
        PayloadItem::Certified { batches, txs }
    }
}

/// Expands a grouped transaction back into its full lane certificate
/// (`None` if it references a batch the group does not carry).
pub fn expand(batches: &[SignedBatchHeader], grouped: &GroupedTx) -> Option<LaneCertificate> {
    let approvals = grouped
        .approvals
        .iter()
        .map(|r| {
            let batch = batches.get(r.batch as usize)?;
            Some(ApprovalProof {
                batch: batch.header.clone(),
                signature: batch.signature.clone(),
                leaf_index: r.leaf_index,
                path: r.path.clone(),
            })
        })
        .collect::<Option<Vec<_>>>()?;
    Some(LaneCertificate {
        tx: grouped.tx.digest(),
        inputs: canonical_inputs(&lane_inputs(&grouped.tx.tx.inputs)),
        approvals,
    })
}

/// The lane-layer view of a transaction's owned inputs.
pub fn lane_inputs(inputs: &[ObjectRef]) -> Vec<LaneRef> {
    inputs
        .iter()
        .map(|r| LaneRef {
            id: r.id,
            version: r.version,
        })
        .collect()
}
