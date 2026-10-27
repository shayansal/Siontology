//! Forced inclusion: censorship becomes visible instead of silent.
//!
//! A user whose intent is being ignored posts it, with its fee, directly to
//! the data-availability layer. From then on the shard has `k` Pulses to
//! include it in committed output or to prove it invalid. An overdue intent
//! is a liveness violation the World Proof cannot paper over: the node
//! freezes the offending shard in the next World Proof and rotates its
//! committee, while other shards keep running.

use std::collections::{BTreeMap, BTreeSet};

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use sion_core::{Hash32, Pulse, ShardId};

use crate::core::item_hash;

/// An intent posted to the data layer.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize,
)]
pub struct PostedIntent {
    /// The intent's identifier (its item hash).
    pub id: Hash32,
    /// The shard that must include it.
    pub shard: ShardId,
    /// The Pulse it was posted in.
    pub posted_at: Pulse,
}

/// How an intent was resolved.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize,
)]
pub enum Resolution {
    /// It appeared in committed output.
    Included {
        /// The Pulse it was included in.
        pulse: Pulse,
        /// Whether that was after the deadline.
        late: bool,
    },
    /// It was proven invalid (for example, its fee coin was already spent).
    ProvenInvalid {
        /// The Pulse the proof was accepted in.
        pulse: Pulse,
    },
}

/// An intent past its deadline.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize,
)]
pub struct OverdueIntent {
    /// The intent.
    pub intent: PostedIntent,
    /// The last Pulse by which it had to be resolved.
    pub deadline: Pulse,
}

/// Tracks forced-inclusion obligations.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct ForcedInclusionTracker {
    window: u64,
    pending: BTreeMap<Hash32, PostedIntent>,
    resolved: BTreeMap<Hash32, Resolution>,
}

impl ForcedInclusionTracker {
    /// Creates a tracker with a deadline of `window` Pulses.
    pub fn new(window: u64) -> Self {
        ForcedInclusionTracker {
            window,
            pending: BTreeMap::new(),
            resolved: BTreeMap::new(),
        }
    }

    /// The deadline for an intent posted at `posted_at`.
    pub fn deadline(&self, posted_at: Pulse) -> Pulse {
        Pulse(posted_at.0 + self.window)
    }

    /// Records a posted intent; returns false if it is already known.
    pub fn post(&mut self, id: Hash32, shard: ShardId, pulse: Pulse) -> bool {
        if self.pending.contains_key(&id) || self.resolved.contains_key(&id) {
            return false;
        }
        self.pending.insert(
            id,
            PostedIntent {
                id,
                shard,
                posted_at: pulse,
            },
        );
        true
    }

    /// Marks an intent included in committed output at `pulse`.
    pub fn mark_included(&mut self, id: &Hash32, pulse: Pulse) -> bool {
        match self.pending.remove(id) {
            Some(intent) => {
                let late = pulse > self.deadline(intent.posted_at);
                self.resolved
                    .insert(*id, Resolution::Included { pulse, late });
                true
            }
            None => false,
        }
    }

    /// Marks an intent proven invalid at `pulse`.
    pub fn mark_proven_invalid(&mut self, id: &Hash32, pulse: Pulse) -> bool {
        match self.pending.remove(id) {
            Some(_) => {
                self.resolved
                    .insert(*id, Resolution::ProvenInvalid { pulse });
                true
            }
            None => false,
        }
    }

    /// Scans committed payload items (identified by [`item_hash`]) and marks
    /// any pending intents among them as included.
    pub fn observe_committed<'a>(
        &mut self,
        items: impl IntoIterator<Item = &'a [u8]>,
        pulse: Pulse,
    ) -> usize {
        items
            .into_iter()
            .filter(|item| self.mark_included(&item_hash(item), pulse))
            .count()
    }

    /// Intents past their deadline at `pulse`, grouped by shard.
    pub fn violations(&self, pulse: Pulse) -> BTreeMap<ShardId, Vec<OverdueIntent>> {
        let mut by_shard: BTreeMap<ShardId, Vec<OverdueIntent>> = BTreeMap::new();
        for intent in self.pending.values() {
            let deadline = self.deadline(intent.posted_at);
            if pulse > deadline {
                by_shard
                    .entry(intent.shard)
                    .or_default()
                    .push(OverdueIntent {
                        intent: *intent,
                        deadline,
                    });
            }
        }
        by_shard
    }

    /// Shards that must be frozen and have their committees rotated.
    pub fn shards_to_freeze(&self, pulse: Pulse) -> BTreeSet<ShardId> {
        self.violations(pulse).into_keys().collect()
    }

    /// How an intent was resolved, if it was.
    pub fn resolution(&self, id: &Hash32) -> Option<Resolution> {
        self.resolved.get(id).copied()
    }

    /// Number of unresolved intents.
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overdue_intents_freeze_their_shard_only() {
        let mut tracker = ForcedInclusionTracker::new(3);
        let a = item_hash(b"intent-a");
        let b = item_hash(b"intent-b");
        let c = item_hash(b"intent-c");
        assert!(tracker.post(a, ShardId(0), Pulse(10)));
        assert!(tracker.post(b, ShardId(1), Pulse(10)));
        assert!(tracker.post(c, ShardId(1), Pulse(11)));
        assert!(!tracker.post(a, ShardId(0), Pulse(12)));

        // Within the window nothing is overdue.
        assert!(tracker.violations(Pulse(13)).is_empty());
        // Shard 0 includes its intent in time; shard 1 proves one invalid.
        assert_eq!(tracker.observe_committed([&b"intent-a"[..]], Pulse(12)), 1);
        assert!(tracker.mark_proven_invalid(&c, Pulse(13)));
        // Shard 1 still owes intent b, posted at 10 with deadline 13.
        let violations = tracker.violations(Pulse(14));
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[&ShardId(1)][0].intent.id, b);
        assert_eq!(
            tracker.shards_to_freeze(Pulse(14)),
            [ShardId(1)].into_iter().collect()
        );
        // Late inclusion resolves it but is recorded as late.
        assert!(tracker.mark_included(&b, Pulse(15)));
        assert_eq!(
            tracker.resolution(&b),
            Some(Resolution::Included {
                pulse: Pulse(15),
                late: true
            })
        );
        assert_eq!(tracker.pending_count(), 0);
    }
}
