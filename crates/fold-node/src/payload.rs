//! What consensus orders.
//!
//! Consensus never orders the owned-path traffic itself: an owned
//! transaction is final once its lane certificate exists. Consensus only
//! fixes *which Pulse executes it*, alongside the contended transactions it
//! does order, and the beacon reveals.

use borsh::{BorshDeserialize, BorshSerialize};
use fold_consensus::{LaneCertificate, ObjectRef as LaneRef};
use fold_core::encode::{decode, encode};
use fold_core::hash::Hash32;
use fold_core::ids::{Pulse, ValidatorId};
use fold_exec::{ObjectRef, SignedTransaction};

/// One consensus payload item.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum PayloadItem {
    /// An owned-path transaction with its lane certificate.
    Owned {
        /// The transaction.
        tx: SignedTransaction,
        /// Approvals from more than two thirds of its shard's committee.
        certificate: LaneCertificate,
    },
    /// A transaction that touches a shared object.
    Contended {
        /// The transaction.
        tx: SignedTransaction,
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
