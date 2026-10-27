//! Forced inclusion: censorship becomes visible instead of silent.
//!
//! A user whose transaction is being ignored, by a censoring gateway or by
//! a censoring minority of validators, posts it directly to consensus as a
//! forced intent ([`crate::payload::PayloadItem::Forced`]). Every validator
//! proposes every round, so one honest proposer is enough to get it
//! ordered.
//!
//! From the Pulse that commits it, the sender's shard has `window` Pulses
//! to do one of two things. It can execute the intent, or the intent can
//! become provably invalid: its inputs consumed by another transaction, its
//! authorization invalid, or its expiry passed. Honest validators of the
//! shard approve forced intents themselves and broadcast their approvals to
//! each other, so a certificate forms without the gateway.
//!
//! Every validator derives the obligations deterministically, from
//! committed data and its executed state, and every Pulse header commits to
//! them ([`fold_light::ForcedSummary`]). If more than a third of a
//! committee censors, the overdue intents therefore show up in certified
//! headers that any light client checks.
//!
//! Not built: the architecture's response to a violation, which freezes the
//! shard in the next World Proof and rotates its committee. Here the
//! violation is detected and reported.

use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use fold_consensus::ForcedInclusionTracker;
use fold_core::hash::{hash_parts, Hash32};
use fold_core::ids::{Pulse, ShardId, TxDigest};
use fold_core::merkle;
use fold_exec::{SignedTransaction, State};
use fold_light::ForcedSummary;
use serde::{Deserialize, Serialize};

use crate::owners::{Authorized, StateOwners};

/// Pulses a shard has to execute (or invalidate) a forced intent.
pub const DEFAULT_WINDOW: u64 = 8;

/// At most this many intents are pending at once; later posts are ignored
/// until some resolve (in commit order, so every validator ignores the same
/// ones).
pub const MAX_PENDING: usize = 4_096;

/// How many resolved intents are remembered for queries.
const RESOLVED_HISTORY: usize = 4_096;

/// One forced intent and where it stands.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntentView {
    /// The transaction's digest.
    pub digest: TxDigest,
    /// The shard that must include it.
    pub shard: u16,
    /// The Pulse that committed it.
    pub posted_at: Pulse,
    /// The last Pulse by which it must be resolved.
    pub deadline: Pulse,
    /// Whether the deadline has passed.
    pub overdue: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
struct Intent {
    tx: SignedTransaction,
    shard: ShardId,
    posted_at: Pulse,
}

/// The forced-inclusion obligations a validator tracks.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct ForcedIntents {
    tracker: ForcedInclusionTracker,
    intents: BTreeMap<TxDigest, Intent>,
    /// Recently resolved intents: (Pulse, included or proven invalid).
    resolved: BTreeMap<TxDigest, (Pulse, bool)>,
}

impl ForcedIntents {
    /// No obligations, with a deadline of `window` Pulses.
    pub fn new(window: u64) -> Self {
        ForcedIntents {
            tracker: ForcedInclusionTracker::new(window),
            intents: BTreeMap::new(),
            resolved: BTreeMap::new(),
        }
    }

    fn resolve(&mut self, digest: TxDigest, pulse: Pulse, included: bool) {
        self.intents.remove(&digest);
        self.resolved.insert(digest, (pulse, included));
        while self.resolved.len() > RESOLVED_HISTORY {
            let oldest = self
                .resolved
                .iter()
                .min_by_key(|(d, (p, _))| (*p, **d))
                .map(|(d, _)| *d)
                .expect("non-empty");
            self.resolved.remove(&oldest);
        }
    }

    /// Updates the obligations after executing `pulse`, in this order:
    ///
    /// 1. intents executed in this Pulse are resolved as included;
    /// 2. intents committed in this Pulse are recorded, unless known or the
    ///    pending set is full;
    /// 3. pending intents the new state makes inadmissible (inputs gone or
    ///    at another version, expired, authorization invalid under the
    ///    current policy) are resolved as proven invalid.
    ///
    /// Returns the intents still pending, which their shard must approve.
    /// Everything here depends only on committed data and the executed
    /// state, so every honest validator computes the same obligations
    /// (`authorized` only saves re-verifying signatures).
    pub fn after_pulse(
        &mut self,
        pulse: Pulse,
        posted: &[SignedTransaction],
        executed: &[TxDigest],
        state: &State,
        shards: u16,
        authorized: &mut Authorized,
    ) -> Vec<SignedTransaction> {
        for digest in executed {
            if self.tracker.mark_included(&digest.0, pulse) {
                self.resolve(*digest, pulse, true);
            }
        }
        for tx in posted {
            if self.intents.len() >= MAX_PENDING {
                break;
            }
            let digest = tx.digest();
            let shard = ShardId::for_account(&tx.tx.sender, shards);
            if self.tracker.post(digest.0, shard, pulse) {
                self.intents.insert(
                    digest,
                    Intent {
                        tx: tx.clone(),
                        shard,
                        posted_at: pulse,
                    },
                );
            }
        }
        let owners = StateOwners { state };
        let invalid: Vec<TxDigest> = self
            .intents
            .iter()
            .filter(|(_, intent)| owners.admissible(&intent.tx, authorized).is_err())
            .map(|(digest, _)| *digest)
            .collect();
        for digest in invalid {
            if self.tracker.mark_proven_invalid(&digest.0, pulse) {
                self.resolve(digest, pulse, false);
            }
        }
        self.intents.values().map(|i| i.tx.clone()).collect()
    }

    /// Whether `digest` is a pending forced intent.
    pub fn is_pending(&self, digest: &TxDigest) -> bool {
        self.intents.contains_key(digest)
    }

    /// A pending intent's transaction.
    pub fn intent(&self, digest: &TxDigest) -> Option<&SignedTransaction> {
        self.intents.get(digest).map(|i| &i.tx)
    }

    /// The pending intents and their deadlines, as of `pulse`.
    pub fn pending(&self, pulse: Pulse) -> Vec<IntentView> {
        self.intents
            .iter()
            .map(|(digest, intent)| {
                let deadline = self.tracker.deadline(intent.posted_at);
                IntentView {
                    digest: *digest,
                    shard: intent.shard.0,
                    posted_at: intent.posted_at,
                    deadline,
                    overdue: pulse > deadline,
                }
            })
            .collect()
    }

    /// Recently resolved intents: the Pulse, and whether the intent was
    /// included (otherwise it was proven invalid).
    pub fn resolved(&self) -> &BTreeMap<TxDigest, (Pulse, bool)> {
        &self.resolved
    }

    /// What the Pulse header commits to.
    pub fn summary(&self, pulse: Pulse) -> ForcedSummary {
        let mut leaves: Vec<Hash32> = self
            .tracker
            .violations(pulse)
            .values()
            .flatten()
            .map(|o| {
                hash_parts(
                    "fold/node/forced-overdue",
                    &[
                        &o.intent.id.0,
                        &o.intent.shard.0.to_be_bytes(),
                        &o.intent.posted_at.0.to_be_bytes(),
                        &o.deadline.0.to_be_bytes(),
                    ],
                )
            })
            .collect();
        leaves.sort();
        let overdue = leaves.len() as u32;
        ForcedSummary {
            pending: (self.intents.len() as u32).saturating_sub(overdue),
            overdue,
            overdue_root: if leaves.is_empty() {
                Hash32::ZERO
            } else {
                merkle::root(&leaves)
            },
        }
    }
}

impl Default for ForcedIntents {
    fn default() -> Self {
        ForcedIntents::new(DEFAULT_WINDOW)
    }
}

#[cfg(test)]
mod tests {
    use fold_core::ids::AssetId;
    use fold_core::Amount;
    use fold_exec::sim::Chain;
    use fold_exec::TxKind;

    use super::*;

    fn transfer(chain: &mut Chain, from: &str, to: &str) -> SignedTransaction {
        let to = chain.id(to);
        chain
            .tx(
                from,
                TxKind::Transfer {
                    asset: AssetId::usd(),
                    outputs: vec![(to, Amount::units(1))],
                },
            )
            .unwrap()
    }

    #[test]
    fn intents_resolve_by_execution_and_otherwise_fall_due() {
        let mut chain = Chain::new(&["a", "b", "c"], 100, 2);
        let mut forced = ForcedIntents::new(3);
        let mut authorized = Authorized::default();
        let ignored = transfer(&mut chain, "a", "b");
        let executed = transfer(&mut chain, "b", "c");
        let pulse = chain.state.pulse.next();
        let pending = forced.after_pulse(
            pulse,
            &[ignored.clone(), executed.clone()],
            &[],
            &chain.state,
            2,
            &mut authorized,
        );
        assert_eq!(pending.len(), 2);
        assert_eq!(forced.summary(pulse).pending, 2);

        // The shard executes one; the other stays pending.
        chain.pulse(vec![executed.clone()]).unwrap();
        let pulse = chain.state.pulse;
        forced.after_pulse(
            pulse,
            &[],
            &[executed.digest()],
            &chain.state,
            2,
            &mut authorized,
        );
        assert_eq!(forced.resolved()[&executed.digest()], (pulse, true));
        assert!(forced.is_pending(&ignored.digest()));

        // Past the window, the ignored intent is overdue: a violation the
        // header commits to.
        chain.advance_to(Pulse(pulse.0 + 4)).unwrap();
        let later = chain.state.pulse;
        forced.after_pulse(later, &[], &[], &chain.state, 2, &mut authorized);
        let summary = forced.summary(later);
        assert_eq!((summary.pending, summary.overdue), (0, 1));
        assert_ne!(summary.overdue_root, Hash32::ZERO);
        assert!(forced.pending(later)[0].overdue);
    }

    #[test]
    fn an_intent_whose_inputs_are_spent_is_proven_invalid() {
        let mut chain = Chain::new(&["a", "b"], 100, 1);
        let mut authorized = Authorized::default();
        let intent = transfer(&mut chain, "a", "b");
        chain.pulse(vec![intent.clone()]).unwrap();
        // Posted after its input was consumed: nothing the shard could do.
        let pulse = chain.state.pulse;
        let mut forced = ForcedIntents::new(3);
        forced.after_pulse(
            pulse,
            std::slice::from_ref(&intent),
            &[],
            &chain.state,
            1,
            &mut authorized,
        );
        assert_eq!(forced.resolved()[&intent.digest()], (pulse, false));
        assert_eq!(forced.summary(pulse), ForcedSummary::default());
    }
}
