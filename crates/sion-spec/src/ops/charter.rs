//! Agent Charters: CreateCharter (§5.12), the charter checks (§5.13),
//! Delegate (§5.14), AgentPay (§5.15), RevokeCharter (§5.16) and
//! ReclaimCharter (§5.17); agent identities: RegisterAgent (§5.35),
//! SpawnAgents (§5.36), SetAgentCard (§5.37), SuspendAgent (§5.38) and
//! ResumeAgent (§5.39).
//!
//! `attenuates` and `spend` are written from §5.13; this crate never calls
//! `Caveats::attenuates` or `Allotment::spend`.

use std::collections::BTreeSet;

use sion_assurance::{decide, AtomClass, ClaimStatus};
use sion_core::encode::encode;
use sion_core::hash::Hash32;
use sion_core::ids::{AccountId, ClaimId, ObjectId, Pulse};
use sion_core::Amount;
use sion_exec::charter::{AgentIdentity, Allotment, Caveats, Charter, Spawn};
use sion_exec::claim::{claim_id, formulas, ClaimBody, Payload};
use sion_exec::execute::{bond_atom, Deferred, Event};
use sion_exec::object::{Object, ObjectData, Owner};
use sion_exec::tx::object_domain;

use sion_exec::charter::{MAX_CHARTER_CHAIN, MAX_SPAWN_BATCH};

use crate::authorize::keyless;
use crate::error::{fail, Abort, Step};
use crate::ops::assurance::load_assurance;
use crate::tx::{usd, TxRun};

/// Receipt claims are valid for at most this many Pulses.
const RECEIPT_VALIDITY: u64 = 604_800;
/// Receipt claims can be challenged for this many Pulses after their window.
const RECEIPT_CHALLENGE_PERIOD: u64 = 86_400;
/// The schema of agent payment receipts.
const RECEIPT_SCHEMA: &str = "receipt.agent.payment/v1";

/// Whether `child` is at least as strict as `parent` (§5.13): delegation
/// only narrows.
pub(crate) fn attenuates(child: &Caveats, parent: &Caveats) -> bool {
    let payees = match (&parent.payees, &child.payees) {
        (None, _) => true,
        (Some(parent), Some(child)) => child.is_subset(parent),
        (Some(_), None) => false,
    };
    let evidence = match (parent.evidence_above, child.evidence_above) {
        (None, _) => true,
        (Some(parent), Some(child)) => child <= parent,
        (Some(_), None) => false,
    };
    payees
        && evidence
        && child.max_per_payment <= parent.max_per_payment
        && child.max_per_window <= parent.max_per_window
        && child.window >= parent.window
        && child.expires_at <= parent.expires_at
        && (!parent.receipts || child.receipts)
        && (!child.may_delegate || parent.may_delegate)
}

/// **spend** (§5.13): checks a payment against the balance and the caveats
/// and records it, in the order the specification gives.
pub(crate) fn spend(
    allotment: &mut Allotment,
    pulse: Pulse,
    payee: &AccountId,
    amount: Amount,
    evidence_ok: Option<bool>,
) -> Step {
    if amount > allotment.remaining {
        return Err(fail("insufficient allotment"));
    }
    if let Some(caveats) = &allotment.caveats {
        if pulse > caveats.expires_at {
            return Err(fail("charter caveat violated: charter expired"));
        }
        if let Some(payees) = &caveats.payees {
            if !payees.contains(payee) {
                return Err(fail("charter caveat violated: payee not allowed"));
            }
        }
        if amount > caveats.max_per_payment {
            return Err(fail("charter caveat violated: per-payment limit"));
        }
        // Saturating: a window that would end after u64::MAX never resets.
        let window_end = allotment.window_start.0.saturating_add(caveats.window);
        if pulse.0 >= window_end {
            allotment.window_start = pulse;
            allotment.spent_in_window = Amount::ZERO;
        }
        let in_window = allotment.spent_in_window.checked_add(amount)?;
        let own_limit = caveats
            .max_per_window
            .checked_sub(allotment.carved_window)?;
        if in_window > own_limit {
            return Err(fail("charter caveat violated: window total"));
        }
        if let Some(threshold) = caveats.evidence_above {
            if amount > threshold && evidence_ok != Some(true) {
                return Err(fail("charter caveat violated: evidence required"));
            }
        }
        allotment.spent_in_window = in_window;
    }
    allotment.remaining = allotment.remaining.checked_sub(amount)?;
    Ok(())
}

/// **carve** (§5.13): a child's window limit must fit in the parent
/// allotment's `max_per_window − carved_window`, and is added to
/// `carved_window`.
fn carve(allotment: &mut Allotment, max_per_window: Amount, child: Amount) -> Step {
    let left = max_per_window.checked_sub(allotment.carved_window)?;
    if child > left {
        return Err(fail("window allowance exhausted"));
    }
    allotment.carved_window = allotment.carved_window.checked_add(child)?;
    Ok(())
}

/// **Chain check** (§5.13): every charter from `start` up through its
/// parents must be a charter, unrevoked and, unless `ignoring_suspension`,
/// unsuspended. At most `MAX_CHARTER_CHAIN` (16) charters are loaded: if
/// the last one loaded still has a parent, the check fails without loading
/// that parent. A passing check yields `n`, the number loaded.
fn chain_check_with(
    run: &mut TxRun<'_>,
    start: ObjectId,
    ignoring_suspension: bool,
) -> Step<usize> {
    let mut next = Some(start);
    let mut loaded = 0usize;
    while let Some(id) = next {
        if loaded == MAX_CHARTER_CHAIN {
            return Err(fail(format!(
                "delegation chain deeper than {MAX_CHARTER_CHAIN} charters"
            )));
        }
        loaded += 1;
        let object = run.load(&id)?;
        let ObjectData::Charter(charter) = &object.data else {
            return Err(fail(format!("{id} in the chain is not a charter")));
        };
        if charter.revoked_at.is_some() {
            return Err(fail("charter revoked"));
        }
        if !ignoring_suspension && charter.suspended_by.is_some() {
            return Err(fail("charter suspended"));
        }
        next = charter.parent;
    }
    Ok(loaded)
}

/// The chain check (§5.13), suspension included.
pub(super) fn chain_check(run: &mut TxRun<'_>, start: ObjectId) -> Step<usize> {
    chain_check_with(run, start, false)
}

/// Loads `id`, which must be a charter with an identity record (§5.35).
pub(super) fn load_identity(run: &mut TxRun<'_>, id: ObjectId) -> Step<Charter> {
    let charter = as_charter(&run.load(&id)?)?;
    if charter.identity.is_none() {
        return Err(fail("the charter is not an agent identity"));
    }
    Ok(charter)
}

/// **Overseers** (§5.13): whether `sender` is `charter`'s backer or
/// principal, or else the agent of one of the charters above it, found by
/// loading parents in order, at most `MAX_CHARTER_CHAIN − 1` of them.
fn oversees(run: &mut TxRun<'_>, charter: &Charter, sender: AccountId) -> Step<bool> {
    if sender == charter.backer || sender == charter.principal {
        return Ok(true);
    }
    let mut next = charter.parent;
    let mut loaded = 0usize;
    while let Some(id) = next {
        if loaded == MAX_CHARTER_CHAIN - 1 {
            return Ok(false);
        }
        loaded += 1;
        let object = run.load(&id)?;
        let ObjectData::Charter(above) = &object.data else {
            return Err(fail(format!("{id} above the charter is not a charter")));
        };
        if above.agent == sender {
            return Ok(true);
        }
        next = above.parent;
    }
    Ok(false)
}

/// Splits a loaded object into its charter.
pub(super) fn as_charter(object: &Object) -> Step<Charter> {
    match &object.data {
        ObjectData::Charter(charter) => Ok(charter.clone()),
        _ => Err(fail(format!("{} is not a charter", object.id))),
    }
}

/// Splits a loaded object into its allotment.
pub(super) fn as_allotment(object: &Object) -> Step<Allotment> {
    match &object.data {
        ObjectData::Allotment(allotment) => Ok(allotment.clone()),
        _ => Err(fail(format!("{} is not an allotment", object.id))),
    }
}

/// The allotment a charter operation spends from: `tx.inputs[1]`.
pub(super) fn allotment_input(run: &TxRun<'_>) -> Step<ObjectId> {
    run.tx
        .inputs
        .get(1)
        .map(|input| input.id)
        .ok_or_else(|| fail("the allotment must be the second input"))
}

/// Loads a charter, inserts `allotment` into its allotments and stores it.
fn attach_allotment(run: &mut TxRun<'_>, charter: ObjectId, allotment: ObjectId) -> Step {
    let mut object = run.load(&charter)?;
    let ObjectData::Charter(state) = &mut object.data else {
        return Err(fail("the new charter is not a charter"));
    };
    state.allotments.insert(allotment);
    run.store(object)
}

/// §5.12 steps 1 to 7, shared by CreateCharter and RegisterAgent (§5.35):
/// a root charter with `identity`, its allotment, the agent atom and its
/// bond. Returns the charter and the allotment.
fn root_charter(
    run: &mut TxRun<'_>,
    agent: AccountId,
    caveats: &Caveats,
    funding: Amount,
    agent_bond: Amount,
    identity: Option<AgentIdentity>,
) -> Step<(ObjectId, ObjectId)> {
    let pulse = run.ctx.pulse;
    if caveats.expires_at <= pulse {
        return Err(fail("the caveats have already expired"));
    }
    if caveats.window == 0 {
        return Err(fail("the spending window must be positive"));
    }
    run.draw(usd(), funding.checked_add(agent_bond)?)?;
    let sender = run.sender();
    let charter_id = run.next_created_id();
    let agent_atom = bond_atom(
        AtomClass::Economic,
        &sender,
        &format!("charter:{charter_id}"),
    );
    let created = run.create(
        Owner::Shared(object_domain("charter", &charter_id)),
        ObjectData::Charter(Charter {
            principal: sender,
            agent,
            caveats: caveats.clone(),
            agent_atom: agent_atom.id,
            backer: sender,
            revoked_at: None,
            parent: None,
            allotments: BTreeSet::new(),
            created_at: pulse,
            suspended_by: None,
            identity,
        }),
    )?;
    debug_assert_eq!(created, charter_id);
    let allotment = run.create(
        Owner::Account(agent),
        ObjectData::Allotment(Allotment {
            source: charter_id,
            writer: agent,
            asset: usd(),
            remaining: funding,
            caveats: Some(caveats.clone()),
            window_start: pulse,
            spent_in_window: Amount::ZERO,
            carved_window: Amount::ZERO,
        }),
    )?;
    attach_allotment(run, charter_id, allotment)?;
    run.defer(Deferred::RegisterAtom {
        atom: agent_atom.clone(),
    });
    if agent_bond > Amount::ZERO {
        run.defer(Deferred::PostBond {
            atom: agent_atom,
            owner: sender,
            amount: agent_bond,
        });
    }
    Ok((charter_id, allotment))
}

/// §5.12 CreateCharter: funds an agent's allotment under a new charter and
/// defers the agent atom and its bond.
pub(super) fn create_charter(
    run: &mut TxRun<'_>,
    agent: AccountId,
    caveats: &Caveats,
    funding: Amount,
    agent_bond: Amount,
) -> Step {
    let (charter, allotment) = root_charter(run, agent, caveats, funding, agent_bond, None)?;
    run.event(Event::CharterCreated { charter, allotment });
    Ok(())
}

/// §5.35 RegisterAgent: CreateCharter with an identity record of depth 0.
pub(super) fn register_agent(
    run: &mut TxRun<'_>,
    agent: AccountId,
    card: Hash32,
    caveats: &Caveats,
    funding: Amount,
    agent_bond: Amount,
) -> Step {
    let identity = AgentIdentity { card, depth: 0 };
    let (charter, allotment) =
        root_charter(run, agent, caveats, funding, agent_bond, Some(identity))?;
    run.event(Event::AgentRegistered {
        agent: charter,
        account: agent,
        allotment,
    });
    Ok(())
}

/// §5.14 Delegate: the agent moves part of its allotment to a sub-agent
/// under narrower caveats.
pub(super) fn delegate(
    run: &mut TxRun<'_>,
    charter_id: ObjectId,
    sub_agent: AccountId,
    caveats: &Caveats,
    amount: Amount,
) -> Step {
    let pulse = run.ctx.pulse;
    let sender = run.sender();
    let allotment_id = allotment_input(run)?;
    let mut allotment_object = run.load(&allotment_id)?;
    let mut allotment = as_allotment(&allotment_object)?;
    let parent = as_charter(&run.load(&charter_id)?)?;
    chain_check(run, charter_id)?;
    if allotment.source != charter_id {
        return Err(fail("the allotment is not funded by this charter"));
    }
    if allotment.writer != sender || parent.agent != sender {
        return Err(fail("only the charter's agent may delegate"));
    }
    let Some(own) = &allotment.caveats else {
        return Err(fail("the allotment has no caveats"));
    };
    if !own.may_delegate {
        return Err(fail("delegation not allowed"));
    }
    if !attenuates(caveats, own) {
        return Err(fail("the delegated caveats must be at least as strict"));
    }
    let own_max = own.max_per_window;
    carve(&mut allotment, own_max, caveats.max_per_window)?;
    allotment.remaining = allotment
        .remaining
        .checked_sub(amount)
        .map_err(|_| fail("insufficient allotment"))?;
    allotment_object.data = ObjectData::Allotment(allotment);
    run.store(allotment_object)?;
    let child_id = run.next_created_id();
    run.create(
        Owner::Shared(object_domain("charter", &child_id)),
        ObjectData::Charter(Charter {
            principal: sender,
            agent: sub_agent,
            caveats: caveats.clone(),
            agent_atom: parent.agent_atom,
            backer: parent.backer,
            revoked_at: None,
            parent: Some(charter_id),
            allotments: BTreeSet::new(),
            created_at: pulse,
            suspended_by: None,
            identity: None,
        }),
    )?;
    let child_allotment = run.create(
        Owner::Account(sub_agent),
        ObjectData::Allotment(Allotment {
            source: child_id,
            writer: sub_agent,
            asset: usd(),
            remaining: amount,
            caveats: Some(caveats.clone()),
            window_start: pulse,
            spent_in_window: Amount::ZERO,
            carved_window: Amount::ZERO,
        }),
    )?;
    attach_allotment(run, child_id, child_allotment)?;
    run.event(Event::CharterCreated {
        charter: child_id,
        allotment: child_allotment,
    });
    Ok(())
}

/// The receipt claim of an agent payment (§5.15 step 4).
#[allow(clippy::too_many_arguments)]
fn receipt_body(
    charter_id: ObjectId,
    charter: &Charter,
    payee: AccountId,
    amount: Amount,
    purpose: &str,
    evidence: Option<ClaimId>,
    run: &TxRun<'_>,
) -> ClaimBody {
    let payload = encode(&(
        charter_id,
        payee,
        amount,
        purpose.to_owned(),
        evidence,
        run.digest,
    ));
    ClaimBody {
        schema: RECEIPT_SCHEMA.into(),
        subject: format!("agent paid {amount} USD for: {purpose}"),
        payload: Payload::Public(payload),
        formula: formulas::bonded_statement(charter.agent_atom),
        adjudicator: None,
        detection_permille: 1000,
        valid_for: RECEIPT_VALIDITY.min(run.ctx.config.max_claim_window),
        challenge_period: RECEIPT_CHALLENGE_PERIOD,
    }
}

/// §5.15 AgentPay: the agent pays from its allotment within the caveats,
/// optionally citing an evidence claim, and emits a receipt claim.
pub(super) fn agent_pay(
    run: &mut TxRun<'_>,
    charter_id: ObjectId,
    payee: AccountId,
    amount: Amount,
    purpose: &str,
    evidence: Option<ClaimId>,
) -> Step {
    let sender = run.sender();
    let allotment_id = allotment_input(run)?;
    let mut allotment_object = run.load(&allotment_id)?;
    let mut allotment = as_allotment(&allotment_object)?;
    let charter = as_charter(&run.load(&charter_id)?)?;
    chain_check(run, charter_id)?;
    if allotment.source != charter_id {
        return Err(fail("the allotment is not funded by this charter"));
    }
    if allotment.writer != sender {
        return Err(fail("only the allotment's writer may pay from it"));
    }
    if charter.agent != sender {
        return Err(fail("only the charter's agent may pay"));
    }
    let caveats = allotment
        .caveats
        .clone()
        .ok_or_else(|| fail("the allotment has no caveats"))?;

    let evidence_ok = match (evidence, caveats.evidence_above) {
        (Some(claim), Some(threshold)) if amount > threshold => {
            let loaded = load_assurance(run)?;
            let registry = &loaded.domain.registry;
            let passes = match registry.claim(&claim) {
                Some(record) if record.status == ClaimStatus::Active => {
                    decide(registry, &caveats.evidence_policy, &claim, amount, false)?.accept
                }
                _ => false,
            };
            Some(passes)
        }
        _ => None,
    };

    spend(&mut allotment, run.ctx.pulse, &payee, amount, evidence_ok)?;
    allotment_object.data = ObjectData::Allotment(allotment);
    run.store(allotment_object)?;
    run.mint(payee, usd(), amount)?;

    let receipt = if caveats.receipts {
        let body = receipt_body(charter_id, &charter, payee, amount, purpose, evidence, run);
        let id = claim_id(&sender, &body);
        run.defer(Deferred::IssueClaim {
            issuer: sender,
            backer: charter.backer,
            body,
        });
        Some(id)
    } else {
        None
    };
    run.event(Event::AgentPaid {
        charter: charter_id,
        payee,
        amount,
        receipt,
    });
    Ok(())
}

/// §5.16 RevokeCharter: the agent itself or an overseer revokes a charter
/// (the first revocation's Pulse is kept).
pub(super) fn revoke_charter(run: &mut TxRun<'_>, charter_id: ObjectId) -> Step {
    let mut object = run.load(&charter_id)?;
    let charter = as_charter(&object)?;
    let sender = run.sender();
    if charter.agent != sender && !oversees(run, &charter, sender)? {
        return Err(fail(
            "only the agent or an overseer (backer, principal, an ancestor's agent) may revoke",
        ));
    }
    let ObjectData::Charter(state) = &mut object.data else {
        return Err(fail(format!("{charter_id} is not a charter")));
    };
    if state.revoked_at.is_none() {
        state.revoked_at = Some(run.ctx.pulse);
    }
    run.store(object)?;
    run.event(Event::CharterRevoked {
        charter: charter_id,
    });
    Ok(())
}

/// §5.17 ReclaimCharter: returns what is left in a dead charter's
/// allotments to its backer.
pub(super) fn reclaim_charter(run: &mut TxRun<'_>, charter_id: ObjectId) -> Step {
    let charter = as_charter(&run.load(&charter_id)?)?;
    let sender = run.sender();
    if sender != charter.principal && sender != charter.backer {
        return Err(fail("only the principal or the backer may reclaim"));
    }
    let dead = charter.revoked_at.is_some()
        || run.ctx.pulse > charter.caveats.expires_at
        || match chain_check_with(run, charter_id, true) {
            Ok(_) => false,
            Err(Abort::Op(_)) => true,
            Err(chunk) => return Err(chunk),
        };
    if !dead {
        return Err(fail("the charter is still live"));
    }
    let mut total = Amount::ZERO;
    for id in &charter.allotments {
        let object = run.load(id)?;
        if let ObjectData::Allotment(allotment) = &object.data {
            total = total.checked_add(allotment.remaining)?;
            run.consume(id)?;
        }
    }
    run.mint(charter.backer, usd(), total)?;
    run.event(Event::CharterReclaimed {
        charter: charter_id,
        amount: total,
    });
    Ok(())
}

/// §5.36 SpawnAgents: the agent launches subagents, each with its own
/// identity, account, attenuated caveats and a budget carved from its
/// allotment.
pub(super) fn spawn_agents(run: &mut TxRun<'_>, charter_id: ObjectId, children: &[Spawn]) -> Step {
    // Step 1.
    if children.is_empty() || children.len() > MAX_SPAWN_BATCH {
        return Err(fail(format!(
            "a spawn launches between 1 and {MAX_SPAWN_BATCH} subagents"
        )));
    }
    let pulse = run.ctx.pulse;
    let sender = run.sender();
    // Step 2.
    let allotment_id = allotment_input(run)?;
    let mut allotment_object = run.load(&allotment_id)?;
    let mut allotment = as_allotment(&allotment_object)?;
    // Step 3.
    let parent = as_charter(&run.load(&charter_id)?)?;
    let n = chain_check(run, charter_id)?;
    if n >= MAX_CHARTER_CHAIN {
        return Err(fail("too deep: no room for another generation"));
    }
    // Step 4.
    if allotment.source != charter_id {
        return Err(fail("the allotment is not funded by this charter"));
    }
    if allotment.writer != sender || parent.agent != sender {
        return Err(fail("only the charter's agent may spawn"));
    }
    // Step 5.
    let Some(own) = allotment.caveats.clone() else {
        return Err(fail("the allotment has no caveats"));
    };
    if !own.may_delegate {
        return Err(fail("spawning not allowed"));
    }
    // Step 6.
    for child in children {
        if !attenuates(&child.caveats, &own) {
            return Err(fail("a subagent's caveats must be at least as strict"));
        }
        carve(
            &mut allotment,
            own.max_per_window,
            child.caveats.max_per_window,
        )?;
        allotment.remaining = allotment
            .remaining
            .checked_sub(child.budget)
            .map_err(|_| fail("insufficient allotment for the budgets"))?;
    }
    // Step 7.
    allotment_object.data = ObjectData::Allotment(allotment);
    run.store(allotment_object)?;
    // Step 8.
    let depth = u8::try_from(n).map_err(|_| fail("too deep"))?;
    for child in children {
        let child_id = run.next_created_id();
        // 8.1: a keyless child's account is derived from its id (§4.3).
        let account = match child.agent {
            Some(agent) => agent,
            None => keyless(&child_id),
        };
        run.create(
            Owner::Shared(object_domain("charter", &child_id)),
            ObjectData::Charter(Charter {
                principal: sender,
                agent: account,
                caveats: child.caveats.clone(),
                agent_atom: parent.agent_atom,
                backer: parent.backer,
                revoked_at: None,
                parent: Some(charter_id),
                allotments: BTreeSet::new(),
                created_at: pulse,
                suspended_by: None,
                identity: Some(AgentIdentity {
                    card: child.card,
                    depth,
                }),
            }),
        )?;
        let child_allotment = run.create(
            Owner::Account(account),
            ObjectData::Allotment(Allotment {
                source: child_id,
                writer: account,
                asset: usd(),
                remaining: child.budget,
                caveats: Some(child.caveats.clone()),
                window_start: pulse,
                spent_in_window: Amount::ZERO,
                carved_window: Amount::ZERO,
            }),
        )?;
        attach_allotment(run, child_id, child_allotment)?;
        run.event(Event::AgentSpawned {
            agent: child_id,
            parent: charter_id,
            account,
            allotment: child_allotment,
        });
    }
    Ok(())
}

/// §5.37 SetAgentCard: the agent replaces its card commitment.
pub(super) fn set_agent_card(run: &mut TxRun<'_>, charter_id: ObjectId, card: Hash32) -> Step {
    let mut object = run.load(&charter_id)?;
    let charter = as_charter(&object)?;
    if charter.identity.is_none() {
        return Err(fail("the charter is not an agent identity"));
    }
    if charter.agent != run.sender() {
        return Err(fail("only the agent may set its card"));
    }
    chain_check(run, charter_id)?;
    if let ObjectData::Charter(state) = &mut object.data {
        if let Some(identity) = &mut state.identity {
            identity.card = card;
        }
    }
    run.store(object)?;
    run.event(Event::AgentCardSet {
        agent: charter_id,
        card,
    });
    Ok(())
}

/// §5.38 SuspendAgent: an overseer suspends a charter (the first
/// suspension is kept).
pub(super) fn suspend_agent(run: &mut TxRun<'_>, charter_id: ObjectId) -> Step {
    let mut object = run.load(&charter_id)?;
    let charter = as_charter(&object)?;
    let sender = run.sender();
    if !oversees(run, &charter, sender)? {
        return Err(fail(
            "only an overseer (backer, principal, an ancestor's agent) may suspend",
        ));
    }
    let by = charter.suspended_by.unwrap_or(sender);
    if let ObjectData::Charter(state) = &mut object.data {
        state.suspended_by = Some(by);
    }
    run.store(object)?;
    run.event(Event::AgentSuspended {
        agent: charter_id,
        by,
    });
    Ok(())
}

/// §5.39 ResumeAgent: the suspender or the backer lifts a suspension.
pub(super) fn resume_agent(run: &mut TxRun<'_>, charter_id: ObjectId) -> Step {
    let mut object = run.load(&charter_id)?;
    let charter = as_charter(&object)?;
    let Some(suspender) = charter.suspended_by else {
        return Err(fail("not suspended"));
    };
    let sender = run.sender();
    if sender != suspender && sender != charter.backer {
        return Err(fail("only the suspender or the backer may resume"));
    }
    if let ObjectData::Charter(state) = &mut object.data {
        state.suspended_by = None;
    }
    run.store(object)?;
    run.event(Event::AgentResumed { agent: charter_id });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sion_assurance::Policy;

    fn account(tag: &[u8]) -> AccountId {
        AccountId::derive(tag)
    }

    fn caveats() -> Caveats {
        Caveats {
            payees: Some([account(b"v1"), account(b"v2")].into_iter().collect()),
            max_per_payment: Amount(100),
            max_per_window: Amount(250),
            window: 10,
            expires_at: Pulse(1_000),
            evidence_above: Some(Amount(80)),
            evidence_policy: Policy::standard(),
            receipts: true,
            may_delegate: true,
        }
    }

    fn allotment(caveats: Option<Caveats>) -> Allotment {
        Allotment {
            source: ObjectId::derive(b"charter"),
            writer: account(b"agent"),
            asset: usd(),
            remaining: Amount(1_000),
            caveats,
            window_start: Pulse(0),
            spent_in_window: Amount::ZERO,
            carved_window: Amount::ZERO,
        }
    }

    #[test]
    fn attenuation_only_narrows() {
        let parent = caveats();
        assert!(attenuates(&parent, &parent));
        let mut child = parent.clone();
        child.payees = Some([account(b"v1")].into_iter().collect());
        child.max_per_payment = Amount(50);
        child.window = 20;
        child.receipts = true;
        child.may_delegate = false;
        assert!(attenuates(&child, &parent));
        // Any widening fails.
        let mut wide = parent.clone();
        wide.payees = None;
        assert!(!attenuates(&wide, &parent));
        let mut wide = parent.clone();
        wide.evidence_above = None;
        assert!(!attenuates(&wide, &parent));
        let mut wide = parent.clone();
        wide.evidence_above = Some(Amount(81));
        assert!(!attenuates(&wide, &parent));
        let mut wide = parent.clone();
        wide.window = 9;
        assert!(!attenuates(&wide, &parent));
        let mut wide = parent.clone();
        wide.expires_at = Pulse(1_001);
        assert!(!attenuates(&wide, &parent));
        let mut wide = parent.clone();
        wide.receipts = false;
        assert!(!attenuates(&wide, &parent));
        let mut no_delegation = parent.clone();
        no_delegation.may_delegate = false;
        let mut child = parent.clone();
        child.may_delegate = true;
        assert!(!attenuates(&child, &no_delegation));
        // A parent without a payee list or threshold allows anything there.
        let mut open = parent.clone();
        open.payees = None;
        open.evidence_above = None;
        let mut child = parent;
        child.payees = None;
        assert!(attenuates(&child, &open));
    }

    #[test]
    fn spend_checks_caveats_in_order() {
        let v1 = account(b"v1");
        let mut a = allotment(Some(caveats()));
        spend(&mut a, Pulse(1), &v1, Amount(70), None).unwrap();
        assert_eq!(a.remaining, Amount(930));
        assert_eq!(a.spent_in_window, Amount(70));
        assert!(spend(&mut a, Pulse(1), &account(b"x"), Amount(1), None).is_err());
        assert!(spend(&mut a, Pulse(1), &v1, Amount(101), None).is_err());
        assert!(
            spend(&mut a, Pulse(1), &v1, Amount(90), None).is_err(),
            "needs evidence"
        );
        spend(&mut a, Pulse(1), &v1, Amount(90), Some(true)).unwrap();
        spend(&mut a, Pulse(2), &v1, Amount(80), None).unwrap();
        assert!(
            spend(&mut a, Pulse(3), &v1, Amount(20), None).is_err(),
            "window total"
        );
        // A new window resets the total.
        spend(&mut a, Pulse(10), &v1, Amount(20), None).unwrap();
        assert_eq!(a.window_start, Pulse(10));
        assert_eq!(a.spent_in_window, Amount(20));
        assert!(
            spend(&mut a, Pulse(1_001), &v1, Amount(1), None).is_err(),
            "expired"
        );
        assert!(spend(&mut a, Pulse(10), &v1, Amount(10_000), None).is_err());
    }

    #[test]
    fn spend_without_caveats_checks_only_the_balance() {
        let mut a = allotment(None);
        spend(&mut a, Pulse(5), &account(b"anyone"), Amount(1_000), None).unwrap();
        assert_eq!(a.remaining, Amount::ZERO);
        assert!(spend(&mut a, Pulse(5), &account(b"anyone"), Amount(1), None).is_err());
    }

    #[test]
    fn huge_windows_never_reset() {
        // §5.13: the reset test saturates, so the window limit becomes a
        // lifetime limit.
        let mut huge = caveats();
        huge.window = u64::MAX;
        huge.evidence_above = None;
        let mut a = allotment(Some(huge));
        a.window_start = Pulse(1);
        spend(&mut a, Pulse(2), &account(b"v1"), Amount(100), None).unwrap();
        spend(&mut a, Pulse(900), &account(b"v1"), Amount(100), None).unwrap();
        assert_eq!(a.window_start, Pulse(1));
        match spend(&mut a, Pulse(950), &account(b"v1"), Amount(51), None) {
            Err(Abort::Op(error)) => assert!(error.0.contains("window total"), "{error}"),
            other => panic!("expected the window limit, got {other:?}"),
        }
        assert_eq!(a.spent_in_window, Amount(200));
    }

    #[test]
    fn the_chain_limit_is_the_specified_sixteen() {
        assert_eq!(MAX_CHARTER_CHAIN, 16);
    }
}
