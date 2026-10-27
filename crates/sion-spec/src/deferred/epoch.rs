//! The end of an epoch in the deferred phase (spec §6.3, EpochEnd): a share
//! of the fees the treasury collected during the epoch paid to the epoch's
//! validators by weight, joining validators admitted and leaving ones
//! retired, the next active set, and the next epoch's start, seed and fee
//! mark.
//!
//! Validators are paid from fees only (v1.4): the pool is a share of what
//! the treasury's running total of USD fees gained since the last epoch end
//! (`fees_mark`). Whatever else the treasury received — slashing
//! remainders, released bond pools — stays in its balance.

use sion_core::ids::{Pulse, ValidatorId};
use sion_core::Amount;
use sion_exec::execute::Event;
use sion_exec::object::ScheduledEvent;
use sion_exec::validators::{permille_of, share_of, weight, ValidatorSet, ValidatorStatus};

use super::{add, broken, sub, DeferredRun};
use crate::error::SpecError;
use crate::tx::usd;

/// `epoch_end = epoch_start + max(epoch_length, 1) − 1`, saturating (§2).
pub(crate) fn last_pulse_of_epoch(set: &ValidatorSet) -> Pulse {
    let length = set.params.epoch_length.max(1);
    Pulse(set.epoch_start.0.saturating_add(length - 1))
}

impl DeferredRun<'_> {
    /// **EpochEnd** (§6.3).
    pub(super) fn end_epoch(&mut self) -> Result<(), SpecError> {
        let pulse = self.ctx.pulse;
        let beacon = self.ctx.beacon;
        if pulse != last_pulse_of_epoch(self.validator_set()?) {
            return Ok(());
        }
        let treasury = self.treasury()?;
        let balance = treasury
            .balances
            .get(&usd())
            .copied()
            .unwrap_or(Amount::ZERO);
        let collected = treasury.fees.get(&usd()).copied().unwrap_or(Amount::ZERO);
        let set = self.validator_set()?;

        // 1. Rewards: a share of the fees collected since the last epoch
        //    end, by weight, to the active set's records that are Active or
        //    Leaving.
        let gain = collected.checked_sub(set.fees_mark).unwrap_or(Amount::ZERO);
        let pool = permille_of(gain, set.params.reward_permille);
        let total = set
            .active
            .iter()
            .try_fold(0u64, |total, (_, weight)| total.checked_add(*weight))
            .ok_or_else(|| broken("the active set's total weight overflows"))?;
        let mut paid = Amount::ZERO;
        for (validator, weight) in &set.active {
            let Some(record) = set.validators.get_mut(validator) else {
                continue;
            };
            if matches!(
                record.status,
                ValidatorStatus::Active | ValidatorStatus::Leaving
            ) {
                let share = share_of(pool, *weight, total);
                record.rewards = add(record.rewards, share, "a validator's rewards")?;
                paid = add(paid, share, "the epoch's rewards")?;
            }
        }

        // 2. Joining validators become active, leaving ones exit.
        let unbonding = set.params.unbonding;
        for record in set.validators.values_mut() {
            match record.status {
                ValidatorStatus::Joining => record.status = ValidatorStatus::Active,
                ValidatorStatus::Leaving => {
                    record.status = ValidatorStatus::Exited {
                        withdrawable_at: Pulse(pulse.0.saturating_add(unbonding)),
                    }
                }
                _ => {}
            }
        }

        // 3. The next active set: the largest stakes (ties by identifier),
        //    weighted, sorted by identifier; empty if no record is active
        //    (the chain halts).
        let mut ranked: Vec<(Amount, ValidatorId)> = set
            .validators
            .iter()
            .filter(|(_, record)| record.status == ValidatorStatus::Active)
            .map(|(id, record)| (record.stake, *id))
            .collect();
        ranked.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        ranked.truncate(usize::from(set.params.max_active.max(1)));
        let mut next: Vec<(ValidatorId, u64)> = ranked
            .into_iter()
            .map(|(stake, id)| (id, weight(stake)))
            .collect();
        next.sort();
        set.active = next;

        // 4. The next epoch.
        set.epoch = set.epoch.saturating_add(1);
        set.epoch_start = Pulse(
            pulse
                .0
                .checked_add(1)
                .ok_or_else(|| broken("the next epoch's first Pulse overflows"))?,
        );
        set.seed = beacon;
        set.rotations.clear();
        set.fees_mark = collected;
        let next_end = last_pulse_of_epoch(set);
        let epoch = set.epoch;
        let active = u32::try_from(set.active.len())
            .map_err(|_| broken("the active set does not fit an event"))?;

        // 5. The rewards leave the treasury: `balances[USD]` is set to
        //    `balance − paid` (an error if more is paid than the treasury
        //    holds), written even when nothing is paid and the treasury held
        //    no USD.
        let remaining = sub(balance, paid, "the treasury's USD balance")?;
        self.treasury()?.balances.insert(usd(), remaining);
        self.schedule(next_end, ScheduledEvent::EpochEnd);
        self.events.push(Event::EpochChanged {
            epoch,
            active,
            rewards: paid,
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use sion_core::hash::Hash32;
    use sion_core::ids::{DomainId, ObjectId, Version};
    use sion_core::merkle::Frontier;
    use sion_exec::object::{well_known, Object, ObjectData, Owner, Schedule, Treasury};
    use sion_exec::state::ChainConfig;
    use sion_exec::validators::{ValidatorParams, ValidatorRecord};

    use super::*;
    use crate::context::SpecContext;
    use crate::deferred::{run_deferred_phase, DeferredOutcome};
    use crate::view::ChunkView;

    fn set(epoch_start: u64, epoch_length: u64) -> ValidatorSet {
        ValidatorSet {
            params: ValidatorParams {
                epoch_length,
                ..ValidatorParams::devnet()
            },
            epoch: 0,
            epoch_start: Pulse(epoch_start),
            seed: Hash32::ZERO,
            active: Vec::new(),
            validators: BTreeMap::new(),
            rotations: BTreeMap::new(),
            fees_mark: Amount::ZERO,
        }
    }

    /// §2's `epoch_end`, cross-checked against the reference's reading,
    /// including where the sum saturates.
    #[test]
    fn epoch_ends_match_the_reference_reading() {
        let cases = [
            (1, 1),
            (1, 0),
            (1, 10),
            (5, 3),
            (u64::MAX / 2, u64::MAX / 2),
            (u64::MAX, 0),
            (u64::MAX, 1),
            (u64::MAX, 2),
            (u64::MAX - 1, 5),
            (2, u64::MAX),
        ];
        for (start, length) in cases {
            let set = set(start, length);
            assert_eq!(
                last_pulse_of_epoch(&set),
                set.epoch_end(),
                "epoch_start {start}, epoch_length {length}"
            );
        }
    }

    fn shared(id: ObjectId, data: ObjectData) -> Object {
        Object {
            id,
            version: Version(1),
            owner: Owner::Shared(DomainId::derive(b"test")),
            data,
        }
    }

    fn context(pulse: u64) -> SpecContext {
        SpecContext {
            pulse: Pulse(pulse),
            beacon: Hash32([7; 32]),
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
        }
    }

    /// The deferred phase of Pulse `pulse` with only an EpochEnd due.
    fn epoch_phase(
        pulse: u64,
        validators: ValidatorSet,
        treasury: Treasury,
    ) -> Result<DeferredOutcome, SpecError> {
        let due: BTreeMap<u64, BTreeSet<ScheduledEvent>> =
            [(pulse, [ScheduledEvent::EpochEnd].into())].into();
        let objects = vec![
            shared(
                well_known::schedule(),
                ObjectData::Schedule(Schedule { due }),
            ),
            shared(
                well_known::validators(),
                ObjectData::Validators(Box::new(validators)),
            ),
            shared(well_known::treasury(), ObjectData::Treasury(treasury)),
        ];
        let view = ChunkView::new(objects.into_iter().map(|o| (o.id, Some(o))).collect());
        run_deferred_phase(&context(pulse), &view, Frontier::default(), &[], &[])
    }

    fn phase(pulse: u64, validators: ValidatorSet) -> Result<(), SpecError> {
        epoch_phase(pulse, validators, Treasury::default()).map(|_| ())
    }

    /// A set of two active validators of equal weight, ending its epoch at
    /// Pulse 10, with `fees_mark` and the given reward share.
    fn paying_set(fees_mark: u128, reward_permille: u16) -> ValidatorSet {
        let mut set = set(1, 10);
        set.params.reward_permille = reward_permille;
        set.fees_mark = Amount(fees_mark);
        let keys =
            sion_crypto::DualKeypair::from_seed(&[9; 32], sion_crypto::Scheme::SlhDsaSha2_128f);
        for tag in [b"a", b"b"] {
            let id = ValidatorId::derive(tag);
            set.active.push((id, 5));
            set.validators.insert(
                id,
                ValidatorRecord {
                    id,
                    lattice: keys.lattice.public().clone(),
                    hash: keys.hash.public().clone(),
                    operator: sion_core::ids::AccountId::derive(tag),
                    stake: Amount::units(5),
                    status: ValidatorStatus::Active,
                    beacon_tip: Hash32::ZERO,
                    rewards: Amount::ZERO,
                    registered_at: Pulse(0),
                },
            );
        }
        set.active.sort();
        set
    }

    fn treasury(balance: u128, fees: u128) -> Treasury {
        let mut treasury = Treasury::default();
        treasury.balances.insert(usd(), Amount(balance));
        treasury.fees.insert(usd(), Amount(fees));
        treasury
    }

    fn written_set(outcome: &DeferredOutcome) -> ValidatorSet {
        match &outcome.writes[&well_known::validators()].data {
            ObjectData::Validators(set) => (**set).clone(),
            other => panic!("{other:?}"),
        }
    }

    fn written_treasury(outcome: &DeferredOutcome) -> Treasury {
        match &outcome.writes[&well_known::treasury()].data {
            ObjectData::Treasury(treasury) => treasury.clone(),
            other => panic!("{other:?}"),
        }
    }

    /// v1.4: the pool is a share of the fees collected since `fees_mark`,
    /// however much else the treasury holds; the mark moves to the total.
    #[test]
    fn rewards_are_a_share_of_the_fees_collected_in_the_epoch() {
        // 1,000,000 in the treasury, of which fees grew from 100 to 300.
        let outcome = epoch_phase(10, paying_set(100, 500), treasury(1_000_000, 300)).unwrap();
        let set = written_set(&outcome);
        let rewards: Vec<Amount> = set.validators.values().map(|r| r.rewards).collect();
        assert_eq!(
            rewards,
            vec![Amount(50), Amount(50)],
            "half of 200, by weight"
        );
        assert_eq!(set.fees_mark, Amount(300));
        let after = written_treasury(&outcome);
        assert_eq!(after.balances[&usd()], Amount(1_000_000 - 100));
        assert_eq!(after.fees[&usd()], Amount(300), "the running total stays");
        assert!(outcome.events.contains(&Event::EpochChanged {
            epoch: 1,
            active: 2,
            rewards: Amount(100)
        }));
    }

    /// No fees since the mark: nothing is paid, whatever the treasury holds.
    #[test]
    fn non_fee_income_pays_no_rewards() {
        let outcome = epoch_phase(10, paying_set(300, 1000), treasury(1_000_000, 300)).unwrap();
        let set = written_set(&outcome);
        assert!(set.validators.values().all(|r| r.rewards == Amount::ZERO));
        assert_eq!(
            written_treasury(&outcome).balances[&usd()],
            Amount(1_000_000)
        );
        // A treasury that never collected a fee: the mark stays 0 and the
        // USD entry is still written.
        let outcome = epoch_phase(10, paying_set(0, 1000), Treasury::default()).unwrap();
        assert_eq!(written_set(&outcome).fees_mark, Amount::ZERO);
        let after = written_treasury(&outcome);
        assert_eq!(after.balances.get(&usd()), Some(&Amount::ZERO));
        assert!(after.fees.is_empty());
    }

    /// `gain` is 0 if the total is below the mark (unreachable: fees only
    /// grow), and the mark follows the total down.
    #[test]
    fn a_mark_above_the_total_pays_nothing() {
        let outcome = epoch_phase(10, paying_set(500, 1000), treasury(1_000, 300)).unwrap();
        let set = written_set(&outcome);
        assert!(set.validators.values().all(|r| r.rewards == Amount::ZERO));
        assert_eq!(set.fees_mark, Amount(300));
    }

    /// Step 5: paying more than the treasury holds is an error (unreachable:
    /// the fees paid out are in the balance), so the phase is invalid.
    #[test]
    fn rewards_beyond_the_balance_invalidate_the_phase() {
        assert!(epoch_phase(10, paying_set(0, 1000), treasury(400, 400)).is_ok());
        assert!(matches!(
            epoch_phase(10, paying_set(0, 1000), treasury(399, 400)),
            Err(SpecError::DeferredPhase(_))
        ));
    }

    /// Step 4's `epoch_start = ctx.pulse + 1` is not marked saturating: at
    /// the last Pulse number it overflows, which makes the deferred phase
    /// invalid (unreachable: Pulse numbers grow by one).
    #[test]
    fn the_last_pulse_number_cannot_start_an_epoch() {
        assert!(phase(u64::MAX - 1, set(u64::MAX - 1, 1)).is_ok());
        assert!(matches!(
            phase(u64::MAX, set(u64::MAX, 1)),
            Err(SpecError::DeferredPhase(_))
        ));
    }

    /// An epoch end due at another Pulse than `epoch_end` is skipped (the
    /// set is still obtained, so it is written back).
    #[test]
    fn misaligned_epoch_ends_are_skipped() {
        assert!(phase(7, set(1, 10)).is_ok());
    }

    /// v1.5, step 3: slashed validators lose their seats at the boundary
    /// even when nobody else is eligible. The next epoch has no active set,
    /// so the chain halts instead of running on proven equivocators.
    #[test]
    fn slashed_validators_leave_the_active_set_even_if_none_remain() {
        let mut slashed = paying_set(0, 700);
        for record in slashed.validators.values_mut() {
            record.status = ValidatorStatus::Slashed { at: Pulse(4) };
            record.stake = Amount::ZERO;
        }
        let outcome = epoch_phase(10, slashed, treasury(1_000, 400)).unwrap();
        let next = written_set(&outcome);
        assert_eq!(next.epoch, 1);
        assert!(next.active.is_empty(), "{:?}", next.active);
        assert!(
            next.validators.values().all(|r| r.rewards == Amount::ZERO),
            "slashed validators are not paid"
        );
        assert!(outcome.events.contains(&Event::EpochChanged {
            epoch: 1,
            active: 0,
            rewards: Amount::ZERO
        }));

        // One validator still Active keeps the seat alone.
        let mut one_left = paying_set(0, 700);
        let first = *one_left.validators.keys().next().unwrap();
        let record = one_left.validators.get_mut(&first).unwrap();
        record.status = ValidatorStatus::Slashed { at: Pulse(4) };
        record.stake = Amount::ZERO;
        let outcome = epoch_phase(10, one_left, treasury(1_000, 400)).unwrap();
        let next = written_set(&outcome);
        assert_eq!(next.active.len(), 1);
        assert_ne!(next.active[0].0, first);
    }
}
