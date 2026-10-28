//! Pooled stake: DelegateStake (§5.45), Undelegate (§5.46),
//! WithdrawDelegation (§5.47), ClaimDelegatorRewards (§5.48) and
//! SetCommission (§5.49), with the pool's share of an equivocation's slash
//! (§5.30 step 4).
//!
//! Written from the specification: the share arithmetic, the cap and the
//! resolution of pending delegations are implemented here; this crate never
//! calls the `sion_exec::delegation` helpers. Only the types and constants
//! are shared.

use sion_core::ids::{AccountId, Pulse, ValidatorId, Version};
use sion_core::Amount;
use sion_exec::delegation::{
    Delegation, DelegationBook, DelegationState, Pool, CAP_FLOOR_PERMILLE,
    COMMISSION_CEILING_PERMILLE, COMMISSION_STEP_PERMILLE, DELEGATION_SLASH_PERMILLE,
    MIN_DELEGATION, REWARD_SCALE,
};
use sion_exec::execute::Event;
use sion_exec::object::{well_known, Object, ObjectData, Owner};
use sion_exec::validators::{permille_of, ValidatorRecord, ValidatorSet, ValidatorStatus};

use crate::error::{fail, Step};
use crate::tx::{usd, TxRun};

/// `mul_div(a, b, c) = (a / c) × b + (a mod c) × b / c` (§5.45), an error on
/// overflow or if `c = 0`.
pub(crate) fn floor_mul_div(a: u128, b: u128, c: u128) -> Option<u128> {
    if c == 0 {
        return None;
    }
    let whole = (a / c).checked_mul(b)?;
    let rest = (a % c).checked_mul(b)? / c;
    whole.checked_add(rest)
}

/// `shares(p) = earning + pending + unbonding`, saturating (§5.45).
pub(crate) fn all_shares(pool: &Pool) -> u128 {
    pool.earning
        .saturating_add(pool.pending)
        .saturating_add(pool.unbonding)
}

/// `value_of(p, s)` (§5.45): 0 for a pool without shares.
pub(crate) fn stake_behind(pool: &Pool, shares: u128) -> Option<Amount> {
    let total = all_shares(pool);
    if total == 0 {
        return Some(Amount::ZERO);
    }
    floor_mul_div(shares, pool.value.0, total).map(Amount)
}

/// An operation error for share arithmetic that overflowed.
fn arithmetic() -> crate::error::Abort {
    fail("pool arithmetic overflows")
}

/// The delegation book, if it exists (a `try_load`); it must be a book.
struct LoadedBook {
    object: Object,
    book: DelegationBook,
}

fn try_book(run: &mut TxRun<'_>) -> Step<Option<LoadedBook>> {
    let Some(object) = run.try_load(&well_known::delegations())? else {
        return Ok(None);
    };
    let ObjectData::Delegations(book) = &object.data else {
        return Err(fail("the delegation book object is not a delegation book"));
    };
    let book = (**book).clone();
    Ok(Some(LoadedBook { object, book }))
}

/// The book, or an empty one to be created at the well-known identifier.
fn book_or_new(run: &mut TxRun<'_>) -> Step<LoadedBook> {
    if let Some(loaded) = try_book(run)? {
        return Ok(loaded);
    }
    Ok(LoadedBook {
        object: Object {
            id: well_known::delegations(),
            version: Version(0),
            owner: Owner::Shared(well_known::validators_domain()),
            data: ObjectData::Delegations(Box::default()),
        },
        book: DelegationBook::default(),
    })
}

/// The book, which must exist.
fn existing_book(run: &mut TxRun<'_>) -> Step<LoadedBook> {
    try_book(run)?.ok_or_else(|| fail("no delegation book exists"))
}

fn store_book(run: &mut TxRun<'_>, loaded: LoadedBook) -> Step {
    run.store(Object {
        data: ObjectData::Delegations(Box::new(loaded.book)),
        ..loaded.object
    })
}

/// The validator set's contents ("load the validator set").
fn validator_set(run: &mut TxRun<'_>) -> Step<ValidatorSet> {
    let object = run.load(&well_known::validators())?;
    match object.data {
        ObjectData::Validators(set) => Ok(*set),
        _ => Err(fail("the validator set object is not a validator set")),
    }
}

/// "Joining or Active" (§5.45 steps 1 and 4).
fn may_receive(record: &ValidatorRecord) -> bool {
    record.status == ValidatorStatus::Joining || record.status == ValidatorStatus::Active
}

/// A validator's bonded stake (§5.45).
fn bonded_stake(record: &ValidatorRecord, book: &DelegationBook) -> Step<Amount> {
    let pooled = match book.pools.get(&record.id) {
        None => Amount::ZERO,
        Some(pool) => {
            stake_behind(pool, pool.earning.saturating_add(pool.pending)).ok_or_else(arithmetic)?
        }
    };
    Ok(record.stake.checked_add(pooled)?)
}

/// `tx.inputs[1]`, loaded: it must be a delegation the sender owns.
fn own_delegation(run: &mut TxRun<'_>) -> Step<(Object, Delegation)> {
    let id = run
        .tx
        .inputs
        .get(1)
        .map(|input| input.id)
        .ok_or_else(|| fail("the delegation must be the second input"))?;
    let object = run.load(&id)?;
    let ObjectData::Delegation(delegation) = &object.data else {
        return Err(fail(format!("{id} is not a delegation")));
    };
    let delegation = delegation.clone();
    if delegation.owner != run.sender() {
        return Err(fail("the delegation is not the sender's"));
    }
    Ok((object, delegation))
}

/// How a delegation resolved (§5.45, "Resolving").
enum Resolved {
    /// Made in the current epoch.
    Pending,
    /// Earning from `entry`.
    Earning(u128),
}

/// Resolves `delegation` in pool `pool` with `epoch` the set's current
/// epoch, updating both.
fn resolve_in(delegation: &mut Delegation, pool: &mut Pool, epoch: u64) -> Step<Resolved> {
    match delegation.state {
        DelegationState::Unbonding { .. } => Err(fail("the delegation is unbonding")),
        DelegationState::Earning { entry } => Ok(Resolved::Earning(entry)),
        DelegationState::Pending { epoch: made } => {
            if made >= epoch {
                return Ok(Resolved::Pending);
            }
            let Some(join) = pool.joins.get(&made).cloned() else {
                return Err(fail(format!(
                    "the pool has no join at the end of epoch {made}"
                )));
            };
            if join.waiting == 0 {
                return Err(fail("the join has no delegation waiting"));
            }
            let waiting = join.waiting - 1;
            if waiting == 0 {
                pool.joins.remove(&made);
            } else if let Some(entry) = pool.joins.get_mut(&made) {
                entry.waiting = waiting;
            }
            delegation.state = DelegationState::Earning {
                entry: join.reward_per_share,
            };
            Ok(Resolved::Earning(join.reward_per_share))
        }
    }
}

/// What an earning delegation of `shares` from `entry` is owed (§5.45).
fn owed_to(pool: &Pool, shares: u128, entry: u128) -> Step<Amount> {
    if entry > pool.reward_per_share {
        return Err(fail("the delegation's entry is ahead of the pool"));
    }
    floor_mul_div(shares, pool.reward_per_share - entry, REWARD_SCALE)
        .map(Amount)
        .ok_or_else(arithmetic)
}

/// The pool `validator` has in `book`, which must exist.
fn pool_of<'b>(book: &'b mut DelegationBook, validator: &ValidatorId) -> Step<&'b mut Pool> {
    book.pools
        .get_mut(validator)
        .ok_or_else(|| fail(format!("validator {validator} has no pool")))
}

/// §5.45 DelegateStake.
pub(super) fn delegate_stake(run: &mut TxRun<'_>, validator: &ValidatorId, amount: Amount) -> Step {
    let sender: AccountId = run.sender();
    let pulse = run.ctx.pulse;
    // 1.
    if amount < MIN_DELEGATION {
        return Err(fail("a delegation is below the minimum"));
    }
    let set = validator_set(run)?;
    let Some(target) = set.validators.get(validator) else {
        return Err(fail(format!("validator {validator} is not registered")));
    };
    if !may_receive(target) {
        return Err(fail("only a joining or active validator takes delegations"));
    }
    // 2.
    run.draw(set.params.stake_asset, amount)?;
    // 3.
    let mut loaded = book_or_new(run)?;
    // 4.
    let mut eligible: u64 = 0;
    let mut total = Amount::ZERO;
    for record in set.validators.values() {
        if may_receive(record) {
            eligible += 1;
            total = total.checked_add(bonded_stake(record, &loaded.book)?)?;
        }
    }
    let mine = bonded_stake(target, &loaded.book)?.checked_add(amount)?;
    let everyone = total.checked_add(amount)?;
    let cap = (2000u128 / u128::from(eligible.max(1))).max(CAP_FLOOR_PERMILLE);
    let left = mine.0.checked_mul(1000).ok_or_else(arithmetic)?;
    let right = cap.checked_mul(everyone.0).ok_or_else(arithmetic)?;
    if left > right {
        return Err(fail(
            "the delegation would exceed the validator's weight cap",
        ));
    }
    // 5.
    let pool = loaded.book.pools.entry(*validator).or_default();
    let total_shares = all_shares(pool);
    let bought = if total_shares == 0 {
        amount.0
    } else {
        floor_mul_div(amount.0, total_shares, pool.value.0)
            .ok_or_else(|| fail("the pool's shares cannot be priced"))?
    };
    if bought == 0 {
        return Err(fail("the delegation buys no shares"));
    }
    pool.pending = pool.pending.checked_add(bought).ok_or_else(arithmetic)?;
    pool.pending_delegations = pool
        .pending_delegations
        .checked_add(1)
        .ok_or_else(arithmetic)?;
    pool.value = pool.value.checked_add(amount)?;
    // 6.
    let delegation = run.create(
        Owner::Account(sender),
        ObjectData::Delegation(Delegation {
            owner: sender,
            validator: *validator,
            shares: bought,
            state: DelegationState::Pending { epoch: set.epoch },
            since: pulse,
        }),
    )?;
    store_book(run, loaded)?;
    // 7.
    run.event(Event::Delegated {
        delegation,
        validator: *validator,
        owner: sender,
        amount,
        shares: bought,
    });
    Ok(())
}

/// §5.46 Undelegate.
pub(super) fn undelegate(run: &mut TxRun<'_>) -> Step {
    let sender = run.sender();
    let pulse = run.ctx.pulse;
    // 1.
    let (object, mut delegation) = own_delegation(run)?;
    let set = validator_set(run)?;
    let mut loaded = existing_book(run)?;
    let pool = pool_of(&mut loaded.book, &delegation.validator)?;
    // 2.
    let shares = delegation.shares;
    let owed = match resolve_in(&mut delegation, pool, set.epoch)? {
        Resolved::Pending => {
            pool.pending = pool.pending.checked_sub(shares).ok_or_else(arithmetic)?;
            pool.pending_delegations = pool
                .pending_delegations
                .checked_sub(1)
                .ok_or_else(arithmetic)?;
            Amount::ZERO
        }
        Resolved::Earning(entry) => {
            let owed = owed_to(pool, shares, entry)?;
            pool.earning = pool.earning.checked_sub(shares).ok_or_else(arithmetic)?;
            pool.rewards_held = pool.rewards_held.checked_sub(owed)?;
            owed
        }
    };
    pool.unbonding = pool.unbonding.checked_add(shares).ok_or_else(arithmetic)?;
    let at = Pulse(pulse.0.saturating_add(set.params.unbonding));
    delegation.state = DelegationState::Unbonding { at };
    // 3.
    let id = object.id;
    run.store(Object {
        data: ObjectData::Delegation(delegation),
        ..object
    })?;
    store_book(run, loaded)?;
    run.mint(sender, usd(), owed)?;
    // 4.
    run.event(Event::Undelegated {
        delegation: id,
        rewards: owed,
        withdrawable_at: at,
    });
    Ok(())
}

/// §5.47 WithdrawDelegation.
pub(super) fn withdraw_delegation(run: &mut TxRun<'_>) -> Step {
    let sender = run.sender();
    let pulse = run.ctx.pulse;
    // 1.
    let (object, delegation) = own_delegation(run)?;
    match delegation.state {
        DelegationState::Unbonding { at } if at.0 <= pulse.0 => {}
        DelegationState::Unbonding { .. } => return Err(fail("the delegation is still unbonding")),
        _ => return Err(fail("the delegation is not unbonding")),
    }
    // 2.
    let set = validator_set(run)?;
    let mut loaded = existing_book(run)?;
    let pool = pool_of(&mut loaded.book, &delegation.validator)?;
    // 3.
    let amount = stake_behind(pool, delegation.shares).ok_or_else(arithmetic)?;
    pool.unbonding = pool
        .unbonding
        .checked_sub(delegation.shares)
        .ok_or_else(arithmetic)?;
    pool.value = pool.value.checked_sub(amount)?;
    store_book(run, loaded)?;
    // 4.
    run.consume(&object.id)?;
    run.mint(sender, set.params.stake_asset, amount)?;
    // 5.
    run.event(Event::DelegationWithdrawn {
        delegation: object.id,
        amount,
    });
    Ok(())
}

/// §5.48 ClaimDelegatorRewards.
pub(super) fn claim_delegator_rewards(run: &mut TxRun<'_>) -> Step {
    let sender = run.sender();
    // 1.
    let (object, mut delegation) = own_delegation(run)?;
    let set = validator_set(run)?;
    let mut loaded = existing_book(run)?;
    let pool = pool_of(&mut loaded.book, &delegation.validator)?;
    // 2.
    let Resolved::Earning(entry) = resolve_in(&mut delegation, pool, set.epoch)? else {
        return Err(fail("the delegation is not earning yet"));
    };
    let owed = owed_to(pool, delegation.shares, entry)?;
    if owed == Amount::ZERO {
        return Err(fail("the delegation is owed nothing"));
    }
    // 3.
    pool.rewards_held = pool.rewards_held.checked_sub(owed)?;
    delegation.state = DelegationState::Earning {
        entry: pool.reward_per_share,
    };
    let id = object.id;
    run.store(Object {
        data: ObjectData::Delegation(delegation),
        ..object
    })?;
    store_book(run, loaded)?;
    run.mint(sender, usd(), owed)?;
    // 4.
    run.event(Event::DelegatorRewardsClaimed {
        delegation: id,
        amount: owed,
    });
    Ok(())
}

/// §5.49 SetCommission.
pub(super) fn set_commission(run: &mut TxRun<'_>, validator: &ValidatorId, permille: u16) -> Step {
    // 1.
    let set = validator_set(run)?;
    match set.validators.get(validator) {
        Some(record) if record.operator == run.sender() => {}
        Some(_) => {
            return Err(fail(format!(
                "validator {validator} is not operated by the sender"
            )))
        }
        None => return Err(fail(format!("validator {validator} is not registered"))),
    }
    if permille > COMMISSION_CEILING_PERMILLE {
        return Err(fail("the commission is above the ceiling"));
    }
    // 2.
    let mut loaded = book_or_new(run)?;
    let pool = loaded.book.pools.entry(*validator).or_default();
    if all_shares(pool) > 0
        && u32::from(permille)
            > u32::from(pool.commission_permille) + u32::from(COMMISSION_STEP_PERMILLE)
    {
        return Err(fail("the commission rises by more than the step"));
    }
    // 3.
    pool.next_commission = Some(permille);
    store_book(run, loaded)?;
    // 4.
    run.event(Event::CommissionSet {
        validator: *validator,
        permille,
    });
    Ok(())
}

/// §5.30 step 4: the offender's pool loses `DELEGATION_SLASH_PERMILLE` of
/// its stake. Returns `Some(slashed)` if the offender had a pool.
pub(super) fn slash_pool(run: &mut TxRun<'_>, offender: &ValidatorId) -> Step<Option<Amount>> {
    let Some(mut loaded) = try_book(run)? else {
        return Ok(None);
    };
    let Some(pool) = loaded.book.pools.get_mut(offender) else {
        return Ok(None);
    };
    let slashed = permille_of(pool.value, DELEGATION_SLASH_PERMILLE);
    pool.value = pool.value.checked_sub(slashed)?;
    store_book(run, loaded)?;
    Ok(Some(slashed))
}
