//! Operations on the assurance domain: Accept (§5.7), Release (§5.8),
//! Challenge (§5.9), Rule with slash settlement (§5.10), WithdrawBond
//! (§5.11). The registry logic itself is the shared Assurance Algebra.

use sion_assurance::{accept_and_reserve, Policy, SlashOutcome};
use sion_core::hash::Hash32;
use sion_core::ids::{AccountId, AtomId, ClaimId, Pulse};
use sion_core::Amount;
use sion_exec::assurance::AssuranceDomain;
use sion_exec::execute::{Deferred, Event};
use sion_exec::object::{well_known, Object, ObjectData, ScheduledEvent};

use crate::error::{fail, Step};
use crate::tx::{usd, TxRun};

/// The assurance object as loaded, and a copy of its domain to work on.
pub(super) struct Loaded {
    /// The object as loaded (identifier, version, owner).
    object: Object,
    /// The domain to change.
    pub(super) domain: AssuranceDomain,
}

/// Loads the assurance object (it must hold the assurance domain).
pub(super) fn load_assurance(run: &mut TxRun<'_>) -> Step<Loaded> {
    let object = run.load(&well_known::registry())?;
    let ObjectData::Assurance(domain) = &object.data else {
        return Err(fail("the registry object is not the assurance domain"));
    };
    let domain = (**domain).clone();
    Ok(Loaded { object, domain })
}

/// Stores the assurance object with its updated domain.
fn store_assurance(run: &mut TxRun<'_>, loaded: Loaded) -> Step {
    let Loaded { object, domain } = loaded;
    run.store(Object {
        data: ObjectData::Assurance(Box::new(domain)),
        ..object
    })
}

/// §5.7 Accept: decides under the consumer's policy and reserves κ·V of the
/// claim's backing until `until`.
pub(super) fn accept(
    run: &mut TxRun<'_>,
    claim: &ClaimId,
    value_at_risk: Amount,
    policy: &Policy,
    until: Pulse,
    nonce: u64,
) -> Step {
    if until <= run.ctx.pulse {
        return Err(fail("reliance must end after this Pulse"));
    }
    let mut loaded = load_assurance(run)?;
    let (decision, reservation) = accept_and_reserve(
        &mut loaded.domain.registry,
        policy,
        run.sender(),
        claim,
        value_at_risk,
        until,
        nonce,
    )?;
    let reservation = reservation.ok_or_else(|| fail("not accepted"))?;
    store_assurance(run, loaded)?;
    run.defer(Deferred::Schedule {
        at: until,
        event: ScheduledEvent::RegistryTick,
    });
    run.event(Event::Reserved {
        reservation,
        claim: *claim,
        amount: decision.required,
    });
    Ok(())
}

/// §5.8 Release: ends the sender's reservation early.
pub(super) fn release(run: &mut TxRun<'_>, reservation: &Hash32) -> Step {
    let mut loaded = load_assurance(run)?;
    sion_assurance::release(&mut loaded.domain.registry, reservation, &run.sender())?;
    store_assurance(run, loaded)?;
    run.defer(Deferred::Schedule {
        at: run.ctx.pulse,
        event: ScheduledEvent::RegistryTick,
    });
    run.event(Event::Released {
        reservation: *reservation,
    });
    Ok(())
}

/// §5.9 Challenge: draws the deposit into the vault and opens a challenge.
pub(super) fn challenge(
    run: &mut TxRun<'_>,
    claim: ClaimId,
    target: AtomId,
    evidence: Hash32,
    deposit: Amount,
) -> Step {
    run.draw(usd(), deposit)?;
    let mut loaded = load_assurance(run)?;
    let sender = run.sender();
    let id = loaded.domain.registry.open_challenge(
        sender,
        claim,
        target,
        evidence,
        deposit,
        run.ctx.pulse,
    )?;
    loaded.domain.vault = loaded.domain.vault.checked_add(deposit)?;
    loaded.domain.deposits.insert(id, (sender, deposit));
    store_assurance(run, loaded)?;
    run.event(Event::ChallengeOpened {
        challenge: id,
        claim,
        target,
    });
    Ok(())
}

/// §5.10 Rule: the claim's adjudicator decides a challenge; the deposit
/// goes back to the challenger if upheld, else to the challenged bond's
/// owner; an upheld challenge slashes and is settled.
pub(super) fn rule(run: &mut TxRun<'_>, challenge: &Hash32, upheld: bool) -> Step {
    let mut loaded = load_assurance(run)?;
    let registry = &loaded.domain.registry;
    let record = registry
        .challenge(challenge)
        .cloned()
        .ok_or_else(|| fail("unknown challenge"))?;
    let claim = registry
        .claim(&record.claim)
        .ok_or_else(|| fail("the challenged claim is unknown"))?;
    let adjudicator = claim
        .adjudicator
        .ok_or_else(|| fail("the claim names no adjudicator"))?;
    let bond = registry
        .bond(&adjudicator)
        .ok_or_else(|| fail("the adjudicator has no bond"))?;
    if bond.owner != run.sender() {
        return Err(fail("only the adjudicator's bond owner may rule"));
    }
    let outcome = loaded
        .domain
        .registry
        .rule(challenge, &adjudicator, upheld, run.ctx.pulse)?;
    let (challenger, deposit) = loaded
        .domain
        .deposits
        .remove(challenge)
        .unwrap_or((record.challenger, Amount::ZERO));
    loaded.domain.vault = loaded.domain.vault.checked_sub(deposit)?;
    let deposit_to = if upheld {
        challenger
    } else {
        loaded
            .domain
            .registry
            .bond(&record.target)
            .map(|bond| bond.owner)
            .unwrap_or(challenger)
    };
    run.mint(deposit_to, usd(), deposit)?;
    if let Some(outcome) = outcome {
        settle_slash(run, &mut loaded.domain, &outcome, challenger)?;
    }
    store_assurance(run, loaded)?;
    run.event(Event::ChallengeRuled {
        challenge: *challenge,
        upheld,
    });
    Ok(())
}

/// §5.10.1: pays compensations from the vault, splits the released pools
/// between the challenger and the treasury.
fn settle_slash(
    run: &mut TxRun<'_>,
    domain: &mut AssuranceDomain,
    outcome: &SlashOutcome,
    challenger: AccountId,
) -> Step {
    run.event(Event::Slashed {
        atom: outcome.atom,
        amount: outcome.slashed,
    });
    for falsified in &outcome.falsified {
        run.event(Event::Falsified {
            claim: falsified.claim,
            causes: falsified.causes.clone(),
        });
    }
    for compensation in &outcome.compensations {
        domain.vault = domain.vault.checked_sub(compensation.amount)?;
        run.mint(compensation.consumer, usd(), compensation.amount)?;
        run.event(Event::Compensated {
            reservation: compensation.reservation,
            consumer: compensation.consumer,
            amount: compensation.amount,
        });
    }
    let released = Amount::sum(outcome.released.values())?;
    domain.vault = domain.vault.checked_sub(released)?;
    let share = u128::from(run.ctx.config.challenger_share_permille);
    let to_challenger = released.mul_ratio(share, 1000)?;
    run.mint(challenger, usd(), to_challenger)?;
    let to_treasury = released.checked_sub(to_challenger)?;
    if to_treasury > Amount::ZERO {
        run.defer(Deferred::Treasury {
            asset: usd(),
            amount: to_treasury,
        });
    }
    Ok(())
}

/// §5.11 WithdrawBond: pays out the free part of the sender's bond after
/// its lock.
pub(super) fn withdraw_bond(run: &mut TxRun<'_>, atom: &AtomId) -> Step {
    let mut loaded = load_assurance(run)?;
    let sender = run.sender();
    let amount = loaded
        .domain
        .registry
        .withdraw_bond(atom, &sender, run.ctx.pulse)?;
    loaded.domain.vault = loaded.domain.vault.checked_sub(amount)?;
    run.mint(sender, usd(), amount)?;
    store_assurance(run, loaded)?;
    run.event(Event::BondWithdrawn {
        atom: *atom,
        amount,
    });
    Ok(())
}
