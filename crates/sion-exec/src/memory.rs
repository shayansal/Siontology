//! Memory logs (specification §5.40 and §5.41).
//!
//! An agent's or a robot's memory is a log of atoms that only its writer
//! appends to, kept off the ledger. The ledger keeps the log's **head**: how
//! many atoms it holds and the Merkle root over them (the RFC 9162 tree
//! hash of `sion_core::merkle`, over leaf hashes the writer computes), plus
//! two roots it stores without checking, the root of the log's index and
//! the root of its storage manifest.
//!
//! Every commit carries a consistency proof from the previous head, and the
//! ledger checks it, so a log can only grow: nothing once committed can be
//! changed or removed. The Pulse in which a commit executes is the
//! **transaction time** of the atoms it adds, assigned by the ledger rather
//! than claimed by the writer. A log object is owned by its writer, so a
//! commit needs a lane certificate, not consensus, and two commits that
//! would fork a log can never both be certified: each consumes the log's
//! current version.
//!
//! A log may belong to an agent identity (§5.35). Then opening and every
//! commit run the charter chain check: while the agent, or any agent above
//! it, is suspended or revoked, its log cannot grow.
//!
//! Atoms, the index and query proofs live in the `sion-memory` crate. This
//! module defines the object and the constants; the rules are in
//! [`crate::execute`].

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use sion_core::hash::Hash32;
use sion_core::ids::{AccountId, ObjectId, Pulse};

/// The longest consistency proof a commit may carry. An RFC 9162
/// consistency proof for a log of `n` leaves has at most ⌈log₂ n⌉ + 1
/// hashes, 65 for 2⁶⁴ leaves; the bound leaves room and keeps a commit's
/// checking cost small and fixed.
pub const MAX_CONSISTENCY_PROOF: usize = 128;

/// A memory log's head, kept on the ledger.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct MemoryLog {
    /// The account that writes the log (and owns this object).
    pub writer: AccountId,
    /// The agent identity the log belongs to, if any.
    pub agent: Option<ObjectId>,
    /// A commitment to the log's policy, kept off the ledger: who may read
    /// it, its retention and its encryption epochs.
    pub policy: Hash32,
    /// How many atoms the log holds.
    pub size: u64,
    /// The Merkle tree hash over the atoms' leaf hashes.
    pub root: Hash32,
    /// The root of the log's index (not checked by the ledger).
    pub index: Hash32,
    /// The root of the log's storage manifest: where its segments are kept
    /// (not checked by the ledger).
    pub manifest: Hash32,
    /// Commits so far: the sequence number of the current head.
    pub commits: u64,
    /// The Pulse the log was opened.
    pub opened_at: Pulse,
    /// The Pulse of the last commit: the transaction time of the atoms it
    /// added.
    pub committed_at: Pulse,
    /// Whether the writer has sealed the log: a sealed log takes no more
    /// commits.
    pub sealed: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use sion_core::encode::{decode, encode};
    use sion_core::merkle;

    #[test]
    fn a_memory_log_round_trips_through_its_encoding() {
        let log = MemoryLog {
            writer: AccountId::derive(b"robot"),
            agent: Some(ObjectId::derive(b"agent")),
            policy: Hash32::ZERO,
            size: 3,
            root: merkle::empty_root(),
            index: Hash32::ZERO,
            manifest: Hash32::ZERO,
            commits: 1,
            opened_at: Pulse(4),
            committed_at: Pulse(5),
            sealed: false,
        };
        let back: MemoryLog = decode(&encode(&log)).unwrap();
        assert_eq!(back, log);
    }

    #[test]
    fn consistency_proofs_stay_within_the_bound() {
        let leaves: Vec<Hash32> = (0u32..1_100)
            .map(|i| merkle::leaf_hash(&i.to_be_bytes()))
            .collect();
        for new_size in [1usize, 2, 3, 7, 8, 9, 100, 513, 1_024, 1_100] {
            let bound = (usize::BITS - (new_size - 1).leading_zeros()) as usize + 1;
            for old_size in 1..=new_size {
                let proof = merkle::consistency_proof(&leaves[..new_size], old_size).unwrap();
                assert!(
                    proof.len() <= bound,
                    "{old_size} -> {new_size}: {}",
                    proof.len()
                );
            }
        }
    }

    // ⌈log₂ 2⁶⁴⌉ + 1 = 65.
    const _: () = assert!(MAX_CONSISTENCY_PROOF >= 65);
}
