//! Validators: RegisterValidator (§5.26), Unstake (§5.27), WithdrawStake
//! (§5.28), ClaimRewards (§5.29) and ReportEquivocation (§5.30).
//!
//! Each operation loads the validator set (`well_known::validators()`), the
//! only object these operations read besides the inputs `draw` spends and,
//! for ReportEquivocation, the delegation book (v1.11).
//! Signatures in equivocation evidence are checked with the keys the set
//! records for the signer, through the shared consensus message formats.

use std::collections::BTreeMap;

use sion_consensus::SignatureVerifier;
use sion_core::hash::Hash32;
use sion_core::ids::{ObjectId, ValidatorId, Version};
use sion_core::Amount;
use sion_crypto::{validator_id, verify_validator, PublicKey};
use sion_exec::execute::{Deferred, Event};
use sion_exec::object::{well_known, Object, ObjectData, ObjectRef};
use sion_exec::validators::{
    permille_of, possession_digest, Equivocation, ValidatorRecord, ValidatorSet, ValidatorStatus,
};

use crate::error::{fail, Step};
use crate::tx::{usd, TxRun};

/// The validator set object and its contents, as loaded.
struct LoadedSet {
    object: Object,
    set: ValidatorSet,
}

/// "Load the validator set": `load(well_known::validators())`, which must
/// hold a validator set.
fn load_set(run: &mut TxRun<'_>) -> Step<LoadedSet> {
    let object = run.load(&well_known::validators())?;
    let ObjectData::Validators(set) = &object.data else {
        return Err(fail("the validator set object is not a validator set"));
    };
    let set = (**set).clone();
    Ok(LoadedSet { object, set })
}

/// "Store the set".
fn store_set(run: &mut TxRun<'_>, loaded: LoadedSet) -> Step {
    run.store(Object {
        data: ObjectData::Validators(Box::new(loaded.set)),
        ..loaded.object
    })
}

/// The record of `validator`, which must be operated by the sender (its
/// record exists and its `operator` is the sender).
fn operated<'s>(
    set: &'s mut ValidatorSet,
    validator: &ValidatorId,
    run: &TxRun<'_>,
) -> Step<&'s mut ValidatorRecord> {
    let sender = run.sender();
    match set.validators.get_mut(validator) {
        Some(record) if record.operator == sender => Ok(record),
        Some(_) => Err(fail(format!(
            "validator {validator} is not operated by the sender"
        ))),
        None => Err(fail(format!("validator {validator} is not registered"))),
    }
}

/// §5.26 RegisterValidator: stakes `stake` of the stake asset for a new
/// validator that proves it holds both keys.
pub(super) fn register_validator(
    run: &mut TxRun<'_>,
    lattice: &PublicKey,
    hash: &PublicKey,
    beacon_tip: Hash32,
    stake: Amount,
    possession: &[u8],
) -> Step {
    // 1.
    let id = validator_id(lattice, hash);
    let mut loaded = load_set(run)?;
    // 2.
    if loaded.set.validators.contains_key(&id) {
        return Err(fail(format!("validator {id} is already registered")));
    }
    if stake < loaded.set.params.min_stake {
        return Err(fail("the stake is below the minimum"));
    }
    // 3.
    let sender = run.sender();
    let message = possession_digest(&run.ctx.config, &sender, &id);
    if !verify_validator(lattice, hash, &message.0, possession) {
        return Err(fail(
            "the proof of possession does not verify for this chain and operator",
        ));
    }
    // 4.
    run.draw(loaded.set.params.stake_asset, stake)?;
    // 5.
    loaded.set.validators.insert(
        id,
        ValidatorRecord {
            id,
            lattice: lattice.clone(),
            hash: hash.clone(),
            operator: sender,
            stake,
            status: ValidatorStatus::Joining,
            beacon_tip,
            rewards: Amount::ZERO,
            registered_at: run.ctx.pulse,
        },
    );
    store_set(run, loaded)?;
    run.event(Event::ValidatorRegistered {
        validator: id,
        operator: sender,
        stake,
    });
    Ok(())
}

/// §5.27 Unstake: a joining validator exits at once; an active one leaves
/// at the epoch's end, unless no other validator would stay.
pub(super) fn unstake(run: &mut TxRun<'_>, validator: &ValidatorId) -> Step {
    // 1.
    let mut loaded = load_set(run)?;
    let staying = loaded
        .set
        .validators
        .iter()
        .filter(|(id, record)| {
            *id != validator
                && matches!(
                    record.status,
                    ValidatorStatus::Active | ValidatorStatus::Joining
                )
        })
        .count();
    let pulse = run.ctx.pulse;
    let record = operated(&mut loaded.set, validator, run)?;
    // 2.
    match record.status {
        ValidatorStatus::Joining => {
            record.status = ValidatorStatus::Exited {
                withdrawable_at: pulse,
            };
        }
        ValidatorStatus::Active => {
            if staying == 0 {
                return Err(fail("the last validator cannot leave"));
            }
            record.status = ValidatorStatus::Leaving;
        }
        other => {
            return Err(fail(format!(
                "a validator that is {other:?} cannot unstake"
            )))
        }
    }
    // 3.
    store_set(run, loaded)?;
    run.event(Event::ValidatorLeaving {
        validator: *validator,
    });
    Ok(())
}

/// §5.28 WithdrawStake: an exited validator's stake and unclaimed rewards
/// return to its operator once unbonding is over.
pub(super) fn withdraw_stake(run: &mut TxRun<'_>, validator: &ValidatorId) -> Step {
    // 1.
    let mut loaded = load_set(run)?;
    let stake_asset = loaded.set.params.stake_asset;
    let pulse = run.ctx.pulse;
    let sender = run.sender();
    let record = operated(&mut loaded.set, validator, run)?;
    match record.status {
        ValidatorStatus::Exited { withdrawable_at } if withdrawable_at <= pulse => {}
        ValidatorStatus::Exited { withdrawable_at } => {
            return Err(fail(format!(
                "the stake is withdrawable from Pulse {}",
                withdrawable_at.0
            )))
        }
        other => {
            return Err(fail(format!(
                "a validator that is {other:?} has no withdrawable stake"
            )))
        }
    }
    // 2.
    let stake = record.stake;
    let rewards = record.rewards;
    record.stake = Amount::ZERO;
    record.rewards = Amount::ZERO;
    record.status = ValidatorStatus::Withdrawn;
    store_set(run, loaded)?;
    // 3.
    run.mint(sender, stake_asset, stake)?;
    run.mint(sender, usd(), rewards)?;
    run.event(Event::StakeWithdrawn {
        validator: *validator,
        amount: stake,
    });
    Ok(())
}

/// §5.29 ClaimRewards: pays the validator's accrued rewards to its
/// operator.
pub(super) fn claim_rewards(run: &mut TxRun<'_>, validator: &ValidatorId) -> Step {
    let mut loaded = load_set(run)?;
    let sender = run.sender();
    let record = operated(&mut loaded.set, validator, run)?;
    let rewards = record.rewards;
    if rewards == Amount::ZERO {
        return Err(fail("no rewards to claim"));
    }
    record.rewards = Amount::ZERO;
    store_set(run, loaded)?;
    run.mint(sender, usd(), rewards)?;
    run.event(Event::RewardsClaimed {
        validator: *validator,
        amount: rewards,
    });
    Ok(())
}

/// Signature checks with the keys the set records for the signer
/// (`verify_validator(record.lattice, record.hash, …)`); an unknown signer
/// fails.
struct RecordedKeys<'s>(&'s BTreeMap<ValidatorId, ValidatorRecord>);

impl SignatureVerifier for RecordedKeys<'_> {
    fn verify(&self, validator: &ValidatorId, message: &[u8], signature: &[u8]) -> bool {
        self.0.get(validator).is_some_and(|record| {
            verify_validator(&record.lattice, &record.hash, message, signature)
        })
    }
}

/// The inputs of a transaction in the consensus layer's reference format.
fn lane_inputs(inputs: &[ObjectRef]) -> Vec<sion_consensus::ObjectRef> {
    inputs
        .iter()
        .map(|input| sion_consensus::ObjectRef {
            id: input.id,
            version: input.version,
        })
        .collect()
}

/// Whether `(id, version)` is one of `inputs`.
fn spends(inputs: &[ObjectRef], id: &ObjectId, version: Version) -> bool {
    inputs
        .iter()
        .any(|input| input.id == *id && input.version == version)
}

/// §5.30 step 1: the validator the evidence proves equivocated.
fn offender(set: &ValidatorSet, evidence: &Equivocation, run: &TxRun<'_>) -> Step<ValidatorId> {
    let keys = RecordedKeys(&set.validators);
    match evidence {
        Equivocation::Headers { first, second } => {
            let (one, two) = (&first.header, &second.header);
            if one.author != two.author
                || one.chain != two.chain
                || one.epoch != two.epoch
                || one.shard != two.shard
                || one.round != two.round
            {
                return Err(fail(
                    "the headers differ in author, chain, epoch, shard or round",
                ));
            }
            if one.digest() == two.digest() {
                return Err(fail("the two headers are the same header"));
            }
            if !first.verify_signature(&keys) || !second.verify_signature(&keys) {
                return Err(fail("a header signature does not verify"));
            }
            Ok(one.author)
        }
        Equivocation::Approvals { first, second } => {
            let (one, two) = (&first.proof.batch, &second.proof.batch);
            if one.validator != two.validator
                || one.chain != two.chain
                || one.epoch != two.epoch
                || one.shard != two.shard
            {
                return Err(fail(
                    "the approvals differ in validator, chain, epoch or shard",
                ));
            }
            let (first_digest, second_digest) = (first.tx.digest(), second.tx.digest());
            if first_digest == second_digest {
                return Err(fail("the two approvals are of the same transaction"));
            }
            let pulse = run.ctx.pulse;
            if pulse > first.tx.expires_at || pulse > second.tx.expires_at {
                return Err(fail("an approved transaction has expired"));
            }
            let shared = first
                .tx
                .inputs
                .iter()
                .any(|input| spends(&second.tx.inputs, &input.id, input.version));
            if !shared {
                return Err(fail("the transactions spend no common object version"));
            }
            first
                .proof
                .verify_signed(&first_digest, &lane_inputs(&first.tx.inputs), &keys)
                .map_err(|error| fail(format!("first approval: {error}")))?;
            second
                .proof
                .verify_signed(&second_digest, &lane_inputs(&second.tx.inputs), &keys)
                .map_err(|error| fail(format!("second approval: {error}")))?;
            Ok(one.validator)
        }
    }
}

/// §5.30 ReportEquivocation: slashes the offender's stake and unclaimed
/// rewards; the reporter receives its share of each, the treasury the rest.
pub(super) fn report_equivocation(run: &mut TxRun<'_>, evidence: &Equivocation) -> Step {
    // 1.
    let mut loaded = load_set(run)?;
    let offender = offender(&loaded.set, evidence, run)?;
    let params = loaded.set.params.clone();
    let pulse = run.ctx.pulse;
    // 2.
    let record = loaded
        .set
        .validators
        .get_mut(&offender)
        .ok_or_else(|| fail(format!("validator {offender} is not registered")))?;
    if matches!(
        record.status,
        ValidatorStatus::Slashed { .. } | ValidatorStatus::Withdrawn
    ) {
        return Err(fail(format!(
            "validator {offender} was already slashed or withdrew"
        )));
    }
    // 3.
    let stake = record.stake;
    let rewards = record.rewards;
    record.stake = Amount::ZERO;
    record.rewards = Amount::ZERO;
    record.status = ValidatorStatus::Slashed { at: pulse };
    store_set(run, loaded)?;
    // 4. The pool (v1.11).
    let pool_slashed = super::delegation::slash_pool(run, &offender)?;
    // 5.
    let sender = run.sender();
    let forfeited_stake = stake.checked_add(pool_slashed.unwrap_or(Amount::ZERO))?;
    let mut reward = Amount::ZERO;
    for (position, (asset, forfeited)) in [(params.stake_asset, forfeited_stake), (usd(), rewards)]
        .into_iter()
        .enumerate()
    {
        let paid = permille_of(forfeited, params.reporter_permille);
        run.mint(sender, asset, paid)?;
        let rest = forfeited.checked_sub(paid)?;
        if rest > Amount::ZERO {
            run.defer(Deferred::Treasury {
                asset,
                amount: rest,
            });
        }
        if position == 0 {
            reward = paid;
        }
    }
    // 6.
    run.event(Event::ValidatorSlashed {
        validator: offender,
        amount: stake,
        reporter: sender,
        reward,
    });
    if let Some(amount) = pool_slashed {
        run.event(Event::PoolSlashed {
            validator: offender,
            amount,
        });
    }
    Ok(())
}
