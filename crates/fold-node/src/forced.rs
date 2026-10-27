//! Forced inclusion: censorship becomes visible, then costly.
//!
//! A user whose transaction is being ignored, by a censoring gateway or by
//! a censoring part of a committee, posts it directly to consensus as a
//! forced intent ([`crate::payload::PayloadItem::Forced`]). Every validator
//! proposes every round, so one honest proposer is enough to get it
//! ordered.
//!
//! The state transition takes it from there (specification §6.3): the
//! Pulse that commits the intent posts it as an obligation of its sender's
//! shard, and at the deadline an intent that could still execute is
//! overdue and the shard's committee is replaced by validators outside it.
//! The node's part is to keep the intents' signed transactions, have honest
//! validators of the shard approve them among themselves (so certificates
//! form without the gateway), and summarize the obligations in every Pulse
//! header ([`fold_light::ForcedSummary`]) so light clients see them without
//! a state proof.

use fold_core::hash::{hash_parts, Hash32};
use fold_core::ids::{Pulse, TxDigest};
use fold_core::merkle;
use fold_exec::validators::ForcedObligations;
use fold_light::ForcedSummary;
use serde::{Deserialize, Serialize};

/// One pending obligation, as the API shows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntentView {
    /// The transaction's digest.
    pub digest: TxDigest,
    /// The shard that must include it.
    pub shard: u16,
    /// The Pulse that posted it.
    pub posted_at: Pulse,
    /// The Pulse at whose end it is checked next.
    pub deadline: Pulse,
    /// Deadlines it has already missed.
    pub missed: u32,
}

/// The pending obligations, in digest order.
pub fn pending(obligations: &ForcedObligations) -> Vec<IntentView> {
    obligations
        .pending
        .iter()
        .map(|(digest, o)| IntentView {
            digest: *digest,
            shard: o.shard,
            posted_at: o.posted_at,
            deadline: o.deadline,
            missed: o.missed,
        })
        .collect()
}

/// What a Pulse header commits to: how many obligations are pending, how
/// many have missed a deadline, and a Merkle root over the overdue ones
/// (digest, shard, posting Pulse, next deadline, misses).
pub fn summary(obligations: &ForcedObligations) -> ForcedSummary {
    let mut leaves: Vec<Hash32> = obligations
        .overdue()
        .map(|(digest, o)| {
            hash_parts(
                "fold/node/forced-overdue",
                &[
                    &digest.0 .0,
                    &o.shard.to_be_bytes(),
                    &o.posted_at.0.to_be_bytes(),
                    &o.deadline.0.to_be_bytes(),
                    &o.missed.to_be_bytes(),
                ],
            )
        })
        .collect();
    leaves.sort();
    let overdue = leaves.len() as u32;
    ForcedSummary {
        pending: (obligations.pending.len() as u32).saturating_sub(overdue),
        overdue,
        overdue_root: if leaves.is_empty() {
            Hash32::ZERO
        } else {
            merkle::root(&leaves)
        },
    }
}

#[cfg(test)]
mod tests {
    use fold_core::ids::{AccountId, ObjectId, Version};
    use fold_exec::object::ObjectRef;
    use fold_exec::validators::{ForcedParams, Obligation};

    use super::*;

    #[test]
    fn the_summary_counts_and_commits_to_overdue_obligations() {
        let mut obligations = ForcedObligations {
            params: ForcedParams::devnet(),
            pending: Default::default(),
        };
        assert_eq!(summary(&obligations), ForcedSummary::default());
        for (n, missed) in [(1u8, 0u32), (2, 1), (3, 2)] {
            obligations.pending.insert(
                TxDigest::derive(&[n]),
                Obligation {
                    shard: 1,
                    sender: AccountId::derive(&[n]),
                    inputs: vec![ObjectRef {
                        id: ObjectId::derive(&[n]),
                        version: Version(0),
                    }],
                    expires_at: Pulse(100),
                    rotations: 0,
                    posted_at: Pulse(3),
                    deadline: Pulse(11),
                    missed,
                },
            );
        }
        let s = summary(&obligations);
        assert_eq!((s.pending, s.overdue), (1, 2));
        assert_ne!(s.overdue_root, Hash32::ZERO);
        assert_eq!(pending(&obligations).len(), 3);
    }
}
