//! The end of an epoch in the deferred phase (spec §6.3, EpochEnd): a share
//! of the treasury's gain paid to the epoch's validators by weight, joining
//! validators admitted and leaving ones retired, the next active set, and
//! the next epoch's start, seed and treasury mark.

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
        let balance = self
            .treasury()?
            .balances
            .get(&usd())
            .copied()
            .unwrap_or(Amount::ZERO);
        let set = self.validator_set()?;

        // 1. Rewards: a share of the treasury's gain, by weight, to the
        //    active set's records that are Active or Leaving.
        let gain = balance
            .checked_sub(set.treasury_mark)
            .unwrap_or(Amount::ZERO);
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
        //    weighted, sorted by identifier; kept if no record is active.
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
        if !next.is_empty() {
            set.active = next;
        }

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
        set.treasury_mark = sub(balance, paid, "the treasury mark")?;
        let next_end = last_pulse_of_epoch(set);
        let epoch = set.epoch;
        let active = u32::try_from(set.active.len())
            .map_err(|_| broken("the active set does not fit an event"))?;

        // 5. The rewards leave the treasury. `balances[USD] −= paid` writes
        //    the entry even when nothing is paid and the treasury held no
        //    USD (read, like the `+=` of treasury credits, as an update of
        //    the entry, which starts at 0): the reference executor does the
        //    same, and the state root depends on it.
        let treasury = self.treasury()?;
        let balance = treasury.balances.entry(usd()).or_insert(Amount::ZERO);
        *balance = sub(*balance, paid, "the treasury's USD balance")?;
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
    use sion_exec::validators::ValidatorParams;

    use super::*;
    use crate::context::SpecContext;
    use crate::deferred::run_deferred_phase;
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
            treasury_mark: Amount::ZERO,
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

    fn phase(pulse: u64, validators: ValidatorSet) -> Result<(), SpecError> {
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
            shared(
                well_known::treasury(),
                ObjectData::Treasury(Treasury {
                    balances: BTreeMap::new(),
                    minted: BTreeMap::new(),
                }),
            ),
        ];
        let view = ChunkView::new(objects.into_iter().map(|o| (o.id, Some(o))).collect());
        run_deferred_phase(&context(pulse), &view, Frontier::default(), &[], &[]).map(|_| ())
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
}
