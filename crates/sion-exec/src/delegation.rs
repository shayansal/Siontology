//! Pooled stake: delegation without custody (specification §5.45 to
//! §5.49 and the epoch end, §6.3).
//!
//! Anyone may add stake to a validator's consensus weight without handing
//! it over. A delegation is an object its delegator owns: it holds
//! **shares** of the validator's **pool**, and the pool holds the stake. The
//! operator never controls it, and there is no pool token.
//!
//! - **Joining.** A delegation made during an epoch is *pending*: it starts
//!   earning, and counting towards the validator's weight, when the epoch
//!   ends. Delegating just before an epoch's end therefore earns nothing
//!   for that epoch.
//! - **Rewards.** At each epoch's end a validator's reward is split between
//!   its own stake and the pool's earning stake in proportion to their
//!   sizes; the operator keeps a commission on the pool's part, and the
//!   rest is credited per earning share (an exact integer accumulator,
//!   scaled by [`REWARD_SCALE`]). Delegators claim theirs whenever they like.
//! - **Commission.** At most [`COMMISSION_CEILING_PERMILLE`]; a raise is at
//!   most [`COMMISSION_STEP_PERMILLE`] at a time, and every change takes
//!   effect at the next epoch's end, so delegators always see it coming.
//! - **Slashing.** An equivocating validator loses its own stake, as
//!   before, and its pool loses [`DELEGATION_SLASH_PERMILLE`] of its stake,
//!   unbonding delegations included: delegators share the risk of the
//!   operator they chose, and the operator's own stake is lost first and in
//!   full.
//! - **Concentration.** A delegation is refused if it would give its
//!   validator more than the weight cap: twice an equal share of the
//!   eligible validators' total stake, and never less than
//!   [`CAP_FLOOR_PERMILLE`].
//! - **Leaving.** Undelegating pays out the rewards earned, stops earning
//!   and starts unbonding, during which the stake stays slashable; after it,
//!   the delegator withdraws.
//!
//! This module defines the objects, the constants and the share arithmetic
//! wallets use. The rules are in [`crate::execute`] and the epoch end in
//! [`crate::pulse`].

use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use sion_core::ids::{AccountId, Pulse, ValidatorId};
use sion_core::Amount;

/// The scale of the per-share reward accumulator.
pub const REWARD_SCALE: u128 = 1_000_000_000_000;

/// The least stake a delegation may add: one unit of the stake asset.
pub const MIN_DELEGATION: Amount = Amount(1_000_000);

/// The most commission a validator may charge, in thousandths.
pub const COMMISSION_CEILING_PERMILLE: u16 = 250;

/// The most a commission may rise at a time, in thousandths.
pub const COMMISSION_STEP_PERMILLE: u16 = 20;

/// The share of a pool's stake an equivocation slashes, in thousandths.
pub const DELEGATION_SLASH_PERMILLE: u16 = 100;

/// The least weight cap, in thousandths of the eligible total.
pub const CAP_FLOOR_PERMILLE: u128 = 100;

/// The weight cap, in thousandths, with `eligible` validators: twice an
/// equal share, and never less than [`CAP_FLOOR_PERMILLE`].
pub fn cap_permille(eligible: u64) -> u128 {
    (2000 / u128::from(eligible.max(1))).max(CAP_FLOOR_PERMILLE)
}

/// `floor(a × b / c)` in exact integer arithmetic; `None` if `c = 0` or an
/// intermediate overflows. Computed as `(a / c) × b + (a mod c) × b / c`.
pub fn mul_div(a: u128, b: u128, c: u128) -> Option<u128> {
    if c == 0 {
        return None;
    }
    (a / c)
        .checked_mul(b)?
        .checked_add((a % c).checked_mul(b)? / c)
}

/// Where a delegation's pending shares join at the end of an epoch, and
/// how many delegations have not yet resolved it.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct Join {
    /// The per-share reward accumulator after the epoch's rewards: the
    /// joining delegations' starting point.
    pub reward_per_share: u128,
    /// Delegations that joined at that epoch's end and have not yet been
    /// used since.
    pub waiting: u64,
}

/// One validator's pool of delegated stake.
#[derive(
    Clone, Debug, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize,
)]
pub struct Pool {
    /// Shares that earn rewards and count towards the validator's weight.
    pub earning: u128,
    /// Shares delegated this epoch: they join at its end.
    pub pending: u128,
    /// How many delegations hold the pending shares.
    pub pending_delegations: u64,
    /// Shares of delegations that are unbonding: slashable, earning nothing.
    pub unbonding: u128,
    /// The stake, in the stake asset, behind all shares.
    pub value: Amount,
    /// Rewards credited per earning share, times [`REWARD_SCALE`].
    pub reward_per_share: u128,
    /// USD credited to delegators and not yet claimed (rounding included).
    pub rewards_held: Amount,
    /// The commission on the pool's rewards, in thousandths.
    pub commission_permille: u16,
    /// A commission that takes effect at the epoch's end.
    pub next_commission: Option<u16>,
    /// Starting points of delegations that joined at an epoch's end, by
    /// epoch, until every such delegation has been used.
    pub joins: BTreeMap<u64, Join>,
}

impl Pool {
    /// All shares.
    pub fn shares(&self) -> u128 {
        self.earning
            .saturating_add(self.pending)
            .saturating_add(self.unbonding)
    }

    /// The stake behind `shares`, rounded down (zero for an empty pool);
    /// `None` if the arithmetic overflows.
    pub fn value_of(&self, shares: u128) -> Option<Amount> {
        if self.shares() == 0 {
            return Some(Amount::ZERO);
        }
        mul_div(shares, self.value.0, self.shares()).map(Amount)
    }

    /// The shares `amount` buys: one per micro-unit in an empty pool, else
    /// in proportion to the pool's stake (rounded down); `None` if the pool
    /// has shares but no stake left.
    pub fn shares_for(&self, amount: Amount) -> Option<u128> {
        if self.shares() == 0 {
            return Some(amount.0);
        }
        mul_div(amount.0, self.shares(), self.value.0)
    }

    /// The stake that counts towards the validator's bonded stake: behind
    /// the earning and pending shares.
    pub fn bonded(&self) -> Option<Amount> {
        self.value_of(self.earning.saturating_add(self.pending))
    }

    /// The stake behind the earning shares: what counts towards the
    /// validator's weight and shares its rewards.
    pub fn earning_value(&self) -> Option<Amount> {
        self.value_of(self.earning)
    }

    /// The USD owed to an earning delegation of `shares` whose last claim
    /// was at accumulator `entry`; `None` on corrupt state.
    pub fn owed(&self, shares: u128, entry: u128) -> Option<Amount> {
        let gained = self.reward_per_share.checked_sub(entry)?;
        mul_div(shares, gained, REWARD_SCALE).map(Amount)
    }
}

/// Whether a validator with `bonded` stake stays within the weight cap of
/// `total` bonded stake among `eligible` validators; `None` on overflow.
pub fn within_cap(bonded: Amount, total: Amount, eligible: u64) -> Option<bool> {
    Some(bonded.0.checked_mul(1000)? <= cap_permille(eligible).checked_mul(total.0)?)
}

/// Where a delegation stands once resolved in the current epoch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Standing {
    /// Made this epoch: it joins at the epoch's end.
    Pending,
    /// Earning since its last claim at accumulator `entry`.
    Earning {
        /// The accumulator at its last claim (or when it joined).
        entry: u128,
    },
}

/// Resolves `delegation` of `pool` in `epoch` (the validator set's current
/// epoch, spec §5.45): a delegation pending in an earlier epoch joined at
/// that epoch's end, so it becomes earning from the accumulator recorded
/// then, and the record is dropped once every delegation that joined then
/// has resolved. An unbonding delegation does not resolve.
pub fn resolve(
    delegation: &mut Delegation,
    pool: &mut Pool,
    epoch: u64,
) -> Result<Standing, &'static str> {
    match delegation.state {
        DelegationState::Pending { epoch: made } if made < epoch => {
            let join = pool
                .joins
                .get_mut(&made)
                .ok_or("the pool has no record of the delegation joining")?;
            let entry = join.reward_per_share;
            join.waiting = join
                .waiting
                .checked_sub(1)
                .ok_or("the pool's join record is corrupted")?;
            if join.waiting == 0 {
                pool.joins.remove(&made);
            }
            delegation.state = DelegationState::Earning { entry };
            Ok(Standing::Earning { entry })
        }
        DelegationState::Pending { .. } => Ok(Standing::Pending),
        DelegationState::Earning { entry } => Ok(Standing::Earning { entry }),
        DelegationState::Unbonding { .. } => Err("the delegation is unbonding"),
    }
}

/// Every validator's pool: a shared protocol object in the validators'
/// domain, created by the first delegation.
#[derive(
    Clone, Debug, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize,
)]
pub struct DelegationBook {
    /// Pools, by validator.
    pub pools: BTreeMap<ValidatorId, Pool>,
}

/// Where a delegation stands.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize,
)]
pub enum DelegationState {
    /// Joins the pool's earning shares at the end of `epoch`.
    Pending {
        /// The epoch it was made in.
        epoch: u64,
    },
    /// Earning rewards; `entry` is the accumulator it last claimed at.
    Earning {
        /// The accumulator at its last claim.
        entry: u128,
    },
    /// Leaving the pool; withdrawable from `at`.
    Unbonding {
        /// The first Pulse it can be withdrawn.
        at: Pulse,
    },
}

/// A delegation: shares of one validator's pool, owned by the delegator.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct Delegation {
    /// The delegator.
    pub owner: AccountId,
    /// The validator.
    pub validator: ValidatorId,
    /// Its shares of the validator's pool.
    pub shares: u128,
    /// Where it stands.
    pub state: DelegationState,
    /// When it was made.
    pub since: Pulse,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mul_div_is_exact_and_never_overflows_silently() {
        assert_eq!(mul_div(7, 3, 2), Some(10));
        assert_eq!(mul_div(u128::MAX, 1, 1), Some(u128::MAX));
        assert_eq!(mul_div(u128::MAX, 2, 2), Some(u128::MAX));
        assert_eq!(mul_div(u128::MAX, 3, 2), None);
        assert_eq!(mul_div(1, 1, 0), None);
        // (a / c)·b + (a mod c)·b / c equals ⌊a·b/c⌋ when nothing overflows.
        for (a, b, c) in [
            (1_000_003u128, 999_983u128, 1_009u128),
            (5, 7, 11),
            (0, 9, 3),
        ] {
            assert_eq!(mul_div(a, b, c), Some(a * b / c));
        }
    }

    #[test]
    fn shares_track_the_pool_through_a_slash() {
        let mut pool = Pool::default();
        let first = pool.shares_for(Amount(2_000_000)).unwrap();
        pool.earning = first;
        pool.value = Amount(2_000_000);
        // A slash of 10 %: the shares are worth less.
        pool.value = Amount(1_800_000);
        assert_eq!(pool.value_of(first), Some(Amount(1_800_000)));
        // New stake buys more shares per unit.
        let second = pool.shares_for(Amount(900_000)).unwrap();
        assert_eq!(second, 1_000_000);
        pool.pending = second;
        pool.value = Amount(2_700_000);
        assert_eq!(pool.value_of(second), Some(Amount(900_000)));
    }

    #[test]
    fn a_pending_delegation_joins_from_the_accumulator_of_its_epoch_end() {
        let mut pool = Pool {
            reward_per_share: 7,
            ..Pool::default()
        };
        pool.joins.insert(
            3,
            Join {
                reward_per_share: 5,
                waiting: 2,
            },
        );
        let mut first = Delegation {
            owner: AccountId::derive(b"a"),
            validator: ValidatorId::derive(b"v"),
            shares: 10,
            state: DelegationState::Pending { epoch: 3 },
            since: Pulse(1),
        };
        let mut second = first.clone();
        // Still epoch 3: pending.
        assert_eq!(resolve(&mut first, &mut pool, 3), Ok(Standing::Pending));
        assert_eq!(
            resolve(&mut first, &mut pool, 4),
            Ok(Standing::Earning { entry: 5 })
        );
        assert_eq!(first.state, DelegationState::Earning { entry: 5 });
        assert_eq!(pool.joins[&3].waiting, 1);
        assert_eq!(
            resolve(&mut second, &mut pool, 5),
            Ok(Standing::Earning { entry: 5 })
        );
        assert!(pool.joins.is_empty());
        // Resolving again changes nothing.
        assert_eq!(
            resolve(&mut first, &mut pool, 6),
            Ok(Standing::Earning { entry: 5 })
        );
        first.state = DelegationState::Unbonding { at: Pulse(9) };
        assert!(resolve(&mut first, &mut pool, 6).is_err());
        assert_eq!(pool.owed(1_000_000_000_000, 5), Some(Amount(2)));
        assert_eq!(pool.owed(1, 8), None);
    }

    #[test]
    fn the_cap_is_twice_an_equal_share_and_at_least_ten_percent() {
        assert_eq!(cap_permille(1), 2000);
        assert_eq!(cap_permille(4), 500);
        assert_eq!(cap_permille(10), 200);
        assert_eq!(cap_permille(20), 100);
        assert_eq!(cap_permille(100), 100);
        assert_eq!(within_cap(Amount(200), Amount(1000), 10), Some(true));
        assert_eq!(within_cap(Amount(201), Amount(1000), 10), Some(false));
        assert_eq!(within_cap(Amount(u128::MAX), Amount(1), 1), None);
    }
}
