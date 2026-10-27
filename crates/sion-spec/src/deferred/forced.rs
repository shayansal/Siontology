//! Forced inclusion in the deferred phase (spec §6.3): posting the Pulse's
//! forced intents as obligations of their senders' shards, checking each
//! obligation at its deadline, and rotating the committee of a shard that
//! let an intent it owed stay executable.

use std::collections::BTreeSet;

use sion_consensus::{resample_committee, sample_committee, Committee, ConsensusError};
use sion_core::ids::{Epoch, Pulse, ShardId, TxDigest, ValidatorId};
use sion_exec::execute::Event;
use sion_exec::object::{well_known, ObjectData, Owner, ScheduledEvent};
use sion_exec::tx::{SignedTransaction, TxKind, MAX_INPUTS};
use sion_exec::validators::{rotation_seed, Obligation, ShardRotation};

use super::{broken, rejected, DeferredRun};
use crate::error::SpecError;
use crate::prepare::signing_context;
use crate::tx::usd;

/// Why a posted intent creates no obligation (free text, not committed).
type Dismissal = String;

/// The identifiers of a sampled committee's members, in its order; a
/// committee that cannot be sampled makes the deferred phase invalid.
fn member_ids(
    committee: Result<Committee, ConsensusError>,
    what: &str,
) -> Result<Vec<ValidatorId>, SpecError> {
    committee
        .map(|committee| {
            committee
                .members()
                .iter()
                .map(|(validator, _)| *validator)
                .collect()
        })
        .map_err(|error| broken(format!("{what}: {error}")))
}

impl DeferredRun<'_> {
    /// Posts one forced intent (§6.3, "Posting forced intents"): it becomes
    /// an obligation of its sender's shard, or is dismissed.
    pub(super) fn post(&mut self, intent: &SignedTransaction) -> Result<(), SpecError> {
        let digest = intent.digest();
        let obligations = self.obligations()?;
        let window = obligations.params.window.max(1);
        let full = u64::try_from(obligations.pending.len()).map_or(true, |pending| {
            pending >= u64::from(obligations.params.max_pending)
        });
        let verdict = if obligations.pending.contains_key(&digest) {
            Err("the intent is already pending".to_owned())
        } else if full {
            Err("the most obligations allowed are pending".to_owned())
        } else {
            self.admissible(intent, &digest)?
        };
        let rotations = match verdict {
            Ok(rotations) => rotations,
            Err(reason) => {
                self.events.push(Event::ForcedDismissed {
                    intent: digest,
                    reason,
                });
                return Ok(());
            }
        };
        let tx = &intent.tx;
        let pulse = self.ctx.pulse;
        let shard = ShardId::for_account(&tx.sender, self.ctx.config.shard_count.max(1));
        let deadline = Pulse(pulse.0.saturating_add(window).min(tx.expires_at.0));
        self.obligations()?.pending.insert(
            digest,
            Obligation {
                shard: shard.0,
                sender: tx.sender,
                inputs: tx.inputs.clone(),
                expires_at: tx.expires_at,
                rotations,
                posted_at: pulse,
                deadline,
                missed: 0,
            },
        );
        self.schedule(deadline, ScheduledEvent::ForcedDeadline { intent: digest });
        self.events.push(Event::ForcedPosted {
            intent: digest,
            shard: shard.0,
            deadline,
        });
        Ok(())
    }

    /// The admissibility checks, in order, stopping at the first failure
    /// (so a dismissed intent reads only what its checks needed). Returns the
    /// sender account's policy rotations, or why the intent is dismissed.
    fn admissible(
        &mut self,
        intent: &SignedTransaction,
        digest: &TxDigest,
    ) -> Result<Result<u32, Dismissal>, SpecError> {
        let tx = &intent.tx;
        let ctx = self.ctx;
        // 1. Shape, expiry and fee, from the intent alone.
        if matches!(tx.kind, TxKind::Faucet { .. }) {
            return Ok(Err("faucet transactions cannot be forced".into()));
        }
        if ctx.pulse >= tx.expires_at {
            return Ok(Err("the intent expires before a later Pulse".into()));
        }
        if tx.inputs.is_empty() || tx.inputs.len() > MAX_INPUTS {
            return Ok(Err(format!("an intent needs 1 to {MAX_INPUTS} inputs")));
        }
        let distinct: BTreeSet<_> = tx.inputs.iter().map(|input| input.id).collect();
        if distinct.len() != tx.inputs.len() {
            return Ok(Err("duplicate input".into()));
        }
        if tx.fee < ctx.config.min_fee(tx.size()) {
            return Ok(Err("fee below the minimum".into()));
        }
        // 2. The sender's policy and rotations.
        let account = self
            .book
            .try_load(&well_known::account(&tx.sender))
            .map_err(rejected)?;
        let (policy, rotations) = match account {
            Some(object) => match object.data {
                ObjectData::Account(state) => (state.policy, state.rotations),
                _ => return Ok(Err("the account object is not an account".into())),
            },
            None => {
                let Some(policy) = tx.genesis_policy.clone() else {
                    return Ok(Err("unknown account and no genesis policy".into()));
                };
                if policy.validate().is_err() {
                    return Ok(Err("invalid genesis policy".into()));
                }
                if policy.account_id() != tx.sender {
                    return Ok(Err("the genesis policy does not hash to the sender".into()));
                }
                (policy, 0)
            }
        };
        // 3. The authorization.
        if policy
            .verify(&signing_context(&ctx.config), &digest.0 .0, &intent.auth)
            .is_err()
        {
            return Ok(Err("the authorization does not verify".into()));
        }
        // 4. The inputs, in order.
        let owner = Owner::Account(tx.sender);
        for (position, input) in tx.inputs.iter().enumerate() {
            let Some(object) = self.book.try_load(&input.id).map_err(rejected)? else {
                return Ok(Err(format!("input {} does not exist", input.id)));
            };
            if object.version != input.version || object.is_tombstone() || object.owner != owner {
                return Ok(Err(format!(
                    "input {} is not spendable by the sender at version {}",
                    input.id, input.version.0
                )));
            }
            if position == 0 {
                match object.as_coin() {
                    Some(coin) if coin.asset == usd() && coin.amount >= tx.fee => {}
                    _ => {
                        return Ok(Err(
                            "the gas input must be a USD coin covering the fee".into()
                        ))
                    }
                }
            }
        }
        Ok(Ok(rotations))
    }

    /// **ForcedDeadline { intent }** (§6.3): an intent that can no longer
    /// execute resolves its obligation; one that still can is overdue, and
    /// its shard's committee is rotated.
    pub(super) fn forced_deadline(&mut self, intent: TxDigest) -> Result<(), SpecError> {
        let obligations = self.obligations()?;
        let window = obligations.params.window.max(1);
        let Some(obligation) = obligations.pending.get(&intent).cloned() else {
            return Ok(());
        };
        if !self.still_executable(&obligation)? {
            self.obligations()?.pending.remove(&intent);
            self.events.push(Event::ForcedResolved { intent });
            return Ok(());
        }
        self.events.push(Event::ForcedOverdue {
            intent,
            shard: obligation.shard,
        });
        self.rotate(obligation.shard, window)?;
        let pulse = self.ctx.pulse;
        if pulse >= obligation.expires_at {
            self.obligations()?.pending.remove(&intent);
            return Ok(());
        }
        let deadline = Pulse(pulse.0.saturating_add(window).min(obligation.expires_at.0));
        if let Some(pending) = self.obligations()?.pending.get_mut(&intent) {
            pending.deadline = deadline;
            pending.missed = pending.missed.saturating_add(1);
        }
        self.schedule(deadline, ScheduledEvent::ForcedDeadline { intent });
        Ok(())
    }

    /// Whether an obligation's intent could still execute: every input, in
    /// order, still exists at its version, is not a tombstone and is owned by
    /// the sender (reading stops at the first that fails), and then the
    /// sender's account shows the same policy rotations (absent counts as 0;
    /// an object that is not an account makes it unexecutable).
    fn still_executable(&mut self, obligation: &Obligation) -> Result<bool, SpecError> {
        let owner = Owner::Account(obligation.sender);
        for input in &obligation.inputs {
            let Some(object) = self.book.try_load(&input.id).map_err(rejected)? else {
                return Ok(false);
            };
            if object.version != input.version || object.is_tombstone() || object.owner != owner {
                return Ok(false);
            }
        }
        let account = self
            .book
            .try_load(&well_known::account(&obligation.sender))
            .map_err(rejected)?;
        let rotations = match account {
            None => 0,
            Some(object) => match object.data {
                ObjectData::Account(state) => state.rotations,
                _ => return Ok(false),
            },
        };
        Ok(rotations == obligation.rotations)
    }

    /// **Rotate** shard `shard` (§6.3): at most once per `window` Pulses,
    /// replaces its lane members by a committee resampled, as far as the
    /// active set allows, from validators outside them. A chain with no
    /// active set has halted (EpochEnd step 3): nothing rotates.
    fn rotate(&mut self, shard: u16, window: u64) -> Result<(), SpecError> {
        let pulse = self.ctx.pulse;
        let beacon = self.ctx.beacon;
        let set = self.validator_set()?;
        if set.active.is_empty() {
            return Ok(());
        }
        let current = set.rotations.get(&shard).cloned();
        if let Some(rotation) = &current {
            if pulse.0 < rotation.at.0.saturating_add(window) {
                return Ok(());
            }
        }
        let seats = usize::from(set.params.seats);
        let epoch = Epoch(set.epoch);
        let lane = ShardId(shard);
        let previous = match &current {
            Some(rotation) => rotation.members.clone(),
            None => member_ids(
                sample_committee(&set.active, seats, &set.seed, epoch, lane),
                "sampling the shard's lane committee",
            )?,
        };
        let count = current
            .as_ref()
            .map_or(1, |rotation| rotation.count.saturating_add(1));
        let members = member_ids(
            resample_committee(
                &set.active,
                seats,
                &rotation_seed(&beacon, lane, count),
                epoch,
                lane,
                &previous,
            ),
            "resampling the shard's lane committee",
        )?;
        set.rotations.insert(
            shard,
            ShardRotation {
                count,
                members,
                at: pulse,
            },
        );
        self.events.push(Event::ShardRotated {
            shard,
            rotation: count,
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use sion_core::hash::Hash32;
    use sion_core::ids::{AccountId, AssetId, DomainId, ObjectId, Version};
    use sion_core::merkle::Frontier;
    use sion_core::Amount;
    use sion_exec::object::{Coin, Object, Schedule};
    use sion_exec::state::ChainConfig;
    use sion_exec::validators::{ForcedObligations, ForcedParams, ValidatorParams, ValidatorSet};

    use super::*;
    use crate::context::SpecContext;
    use crate::deferred::run_deferred_phase;
    use crate::view::ChunkView;

    fn shared(id: ObjectId, data: ObjectData) -> Object {
        Object {
            id,
            version: Version(1),
            owner: Owner::Shared(DomainId::derive(b"test")),
            data,
        }
    }

    /// An overdue obligation of shard 0 at Pulse 10, with a validator set
    /// whose active set is `active` and whose shards have `seats` seats.
    fn overdue(active: Vec<(ValidatorId, u64)>, seats: u16) -> Result<(), SpecError> {
        let sender = AccountId::derive(b"sender");
        let coin = Object {
            id: ObjectId::derive(b"coin"),
            version: Version(3),
            owner: Owner::Account(sender),
            data: ObjectData::Coin(Coin {
                asset: AssetId::usd(),
                amount: Amount::units(1),
            }),
        };
        let intent = TxDigest::derive(b"intent");
        let obligation = Obligation {
            shard: 0,
            sender,
            inputs: vec![coin.reference()],
            expires_at: Pulse(100),
            rotations: 0,
            posted_at: Pulse(5),
            deadline: Pulse(10),
            missed: 0,
        };
        let set = ValidatorSet {
            params: ValidatorParams {
                seats,
                ..ValidatorParams::devnet()
            },
            epoch: 0,
            epoch_start: Pulse(1),
            seed: Hash32::ZERO,
            active,
            validators: BTreeMap::new(),
            rotations: BTreeMap::new(),
            fees_mark: Amount::ZERO,
        };
        let due: BTreeMap<u64, BTreeSet<ScheduledEvent>> =
            [(10, [ScheduledEvent::ForcedDeadline { intent }].into())].into();
        let objects = vec![
            shared(
                well_known::schedule(),
                ObjectData::Schedule(Schedule { due }),
            ),
            shared(
                well_known::forced(),
                ObjectData::Forced(Box::new(ForcedObligations {
                    params: ForcedParams {
                        window: 2,
                        max_pending: 8,
                    },
                    pending: [(intent, obligation)].into(),
                })),
            ),
            shared(
                well_known::validators(),
                ObjectData::Validators(Box::new(set)),
            ),
            coin,
        ];
        let mut witnessed: BTreeMap<ObjectId, Option<Object>> =
            objects.into_iter().map(|o| (o.id, Some(o))).collect();
        witnessed.insert(well_known::account(&sender), None);
        let ctx = SpecContext {
            pulse: Pulse(10),
            beacon: Hash32([3; 32]),
            config: ChainConfig {
                chain_id: "test".into(),
                shard_count: 1,
                base_fee: Amount(1_000),
                fee_per_kib: Amount(100),
                faucet: None,
                max_claim_window: 1_000,
                max_formula_size: 64,
                max_payload_bytes: 1_024,
                challenger_share_permille: 500,
            },
        };
        let view = ChunkView::new(witnessed);
        run_deferred_phase(&ctx, &view, Frontier::default(), &[], &[]).map(|_| ())
    }

    /// §6.3 "Errors": a shard whose committee cannot be sampled (no seats,
    /// which genesis refuses) invalidates the deferred phase. An empty
    /// active set is not such a case since v1.5: the chain has halted, and
    /// Rotate skips the shard.
    #[test]
    fn a_committee_that_cannot_be_sampled_invalidates_the_phase() {
        let validator = ValidatorId::derive(b"v");
        assert!(overdue(vec![(validator, 1)], 2).is_ok());
        assert!(
            overdue(Vec::new(), 2).is_ok(),
            "a halted chain rotates nothing"
        );
        assert!(matches!(
            overdue(vec![(validator, 1)], 0),
            Err(SpecError::DeferredPhase(_))
        ));
    }
}
