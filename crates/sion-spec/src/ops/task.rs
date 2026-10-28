//! Task Charters: PostTask (§5.42), SettleTask (§5.43) and CancelTask
//! (§5.44).
//!
//! Written from the specification. The result formula of §5.43 step 5 is
//! built here from the shared formula constructors; this crate never calls
//! `sion_exec::task::result_formula` or `worker_bond`. Reservation and the
//! decision rule are the shared Assurance Algebra.

use std::collections::BTreeSet;

use sion_assurance::{accept_and_reserve, AtomClass, ClaimStatus, Formula};
use sion_core::encode::decode;
use sion_core::hash::Hash32;
use sion_core::ids::{AccountId, ClaimId, ObjectId, Pulse};
use sion_core::Amount;
use sion_exec::claim::{claim_id, formulas, ClaimBody, Payload};
use sion_exec::execute::{bond_atom, Deferred, Event};
use sion_exec::market::MAX_PERIOD;
use sion_exec::object::{Object, ObjectData, Owner, ScheduledEvent};
use sion_exec::task::{
    Acceptance, Task, TaskResult, TaskState, ATTESTATION_SCHEMA, MAX_REQUIRED, MAX_VERIFIERS,
    RESULT_SCHEMA, WORKER_BOND_LABEL,
};
use sion_exec::tx::object_domain;

use crate::error::{fail, Step};
use crate::ops::assurance::{load_assurance, store_assurance};
use crate::ops::charter::{allotment_input, as_allotment, as_charter, chain_check, spend};
use crate::tx::{usd, TxRun};

/// The terms of §5.42.
pub(super) struct Posting<'a> {
    pub(super) spec: Hash32,
    pub(super) acceptance: &'a Acceptance,
    pub(super) worker: Option<AccountId>,
    pub(super) amount: Amount,
    pub(super) deadline: Pulse,
    pub(super) charter: Option<ObjectId>,
}

/// §5.42 PostTask.
pub(super) fn post(run: &mut TxRun<'_>, terms: Posting<'_>) -> Step {
    let pulse = run.ctx.pulse;
    let sender = run.sender();
    let acceptance = terms.acceptance;
    // Step 1.
    if terms.amount == Amount::ZERO {
        return Err(fail("a task's escrow must be positive"));
    }
    if !(pulse.0 < terms.deadline.0 && terms.deadline.0 <= pulse.0.saturating_add(MAX_PERIOD)) {
        return Err(fail("the deadline is out of range"));
    }
    if acceptance.requires.len() > MAX_REQUIRED || acceptance.verifiers.len() > MAX_VERIFIERS {
        return Err(fail("too many required claims or verifiers"));
    }
    if acceptance.detection_permille > 1000 {
        return Err(fail("the detection factor is above one"));
    }
    if !(1 <= acceptance.reliance && acceptance.reliance <= MAX_PERIOD) {
        return Err(fail("the reliance period is out of range"));
    }
    // Step 2.
    let (funder, refund_to) = match terms.charter {
        None => {
            run.draw(usd(), terms.amount)?;
            (None, sender)
        }
        Some(charter_id) => {
            let allotment_id = allotment_input(run)?;
            let mut allotment_object = run.load(&allotment_id)?;
            let mut allotment = as_allotment(&allotment_object)?;
            let charter = as_charter(&run.load(&charter_id)?)?;
            chain_check(run, charter_id)?;
            if allotment.source != charter_id
                || allotment.writer != sender
                || charter.agent != sender
            {
                return Err(fail("only the charter's agent posts from its allotment"));
            }
            let Some(caveats) = allotment.caveats.clone() else {
                return Err(fail("the allotment has no caveats"));
            };
            if terms.worker.is_none() && caveats.payees.is_some() {
                return Err(fail("an agent with listed payees cannot post an open task"));
            }
            let evidence_ok = caveats.evidence_policy == acceptance.policy;
            let payee = terms.worker.unwrap_or(sender);
            spend(
                &mut allotment,
                pulse,
                &payee,
                terms.amount,
                Some(evidence_ok),
            )?;
            allotment_object.data = ObjectData::Allotment(allotment);
            run.store(allotment_object)?;
            (Some(charter_id), charter.backer)
        }
    };
    // Step 3.
    let task = run.next_created_id();
    run.create(
        Owner::Shared(object_domain("task", &task)),
        ObjectData::Task(Box::new(Task {
            poster: sender,
            funder,
            refund_to,
            spec: terms.spec,
            acceptance: acceptance.clone(),
            worker: terms.worker,
            amount: terms.amount,
            deadline: terms.deadline,
            posted_at: pulse,
            state: TaskState::Open,
        })),
    )?;
    // Step 4.
    run.event(Event::TaskPosted {
        task,
        poster: sender,
        worker: terms.worker,
        amount: terms.amount,
        deadline: terms.deadline,
    });
    Ok(())
}

/// Loads `id`, which must be a task that is still open.
fn open_task(run: &mut TxRun<'_>, id: &ObjectId) -> Step<(Object, Task)> {
    let object = run.load(id)?;
    let task = match &object.data {
        ObjectData::Task(task) => (**task).clone(),
        _ => return Err(fail(format!("{id} is not a task"))),
    };
    if task.state != TaskState::Open {
        return Err(fail("the task is not open"));
    }
    Ok((object, task))
}

/// The formula of §5.43 step 5.
fn required_formula(
    bond: sion_core::ids::AtomId,
    attestations: &[ClaimId],
    requires: &BTreeSet<ClaimId>,
) -> Formula {
    let alternatives: Vec<Formula> = std::iter::once(Formula::atom(bond))
        .chain(attestations.iter().copied().map(Formula::claim))
        .collect();
    let mut parts = Vec::with_capacity(requires.len() + 2);
    parts.push(formulas::dual_signature());
    parts.push(Formula::or(alternatives));
    parts.extend(requires.iter().copied().map(Formula::claim));
    Formula::and(parts)
}

/// §5.43 SettleTask.
pub(super) fn settle(
    run: &mut TxRun<'_>,
    task_id: ObjectId,
    result: &ClaimBody,
    attestations: &[(AccountId, ClaimBody)],
) -> Step {
    let pulse = run.ctx.pulse;
    let sender = run.sender();
    // Step 1.
    let (mut object, mut task) = open_task(run, &task_id)?;
    if pulse > task.deadline {
        return Err(fail("the task's deadline has passed"));
    }
    if let Some(worker) = task.worker {
        if worker != sender {
            return Err(fail("only the named worker settles"));
        }
    }
    // Step 2.
    let r = claim_id(&sender, result);
    let mut loaded = load_assurance(run)?;
    let Some(record) = loaded.domain.registry.claim(&r).cloned() else {
        return Err(fail("the result claim is unknown"));
    };
    if record.status != ClaimStatus::Active {
        return Err(fail("the result claim is not active"));
    }
    // Step 3.
    if result.schema != RESULT_SCHEMA {
        return Err(fail("the result has the wrong schema"));
    }
    let Payload::Public(bytes) = &result.payload else {
        return Err(fail("the result's payload is not public"));
    };
    match decode::<TaskResult>(bytes) {
        Ok(committed) if committed.task == task_id => {}
        _ => return Err(fail("the result commits to another task")),
    }
    // Step 4.
    if attestations.len() > MAX_VERIFIERS {
        return Err(fail("too many attestations"));
    }
    let mut seen = BTreeSet::new();
    let mut attested = Vec::new();
    for (verifier, body) in attestations {
        if !seen.insert(*verifier) {
            return Err(fail("two attestations by one verifier"));
        }
        if !task.acceptance.verifiers.contains(verifier) {
            return Err(fail("the verifier is not named by the task"));
        }
        let a = claim_id(verifier, body);
        match loaded.domain.registry.claim(&a) {
            Some(record) if record.status == ClaimStatus::Active => {}
            _ => return Err(fail("an attestation is not an active claim")),
        }
        if body.schema != ATTESTATION_SCHEMA {
            return Err(fail("an attestation has the wrong schema"));
        }
        if body.payload != result.payload {
            return Err(fail("an attestation commits to something else"));
        }
        attested.push(a);
    }
    // Step 5.
    let bond = bond_atom(AtomClass::Economic, &sender, WORKER_BOND_LABEL).id;
    let expected = required_formula(bond, &attested, &task.acceptance.requires);
    if record.formula != expected {
        return Err(fail("the result's formula is not the required one"));
    }
    if record.adjudicator != task.acceptance.adjudicator {
        return Err(fail("the result names another adjudicator"));
    }
    if result.detection_permille > task.acceptance.detection_permille {
        return Err(fail("the result's detection factor is too high"));
    }
    // Step 6.
    let until = Pulse(pulse.0.saturating_add(task.acceptance.reliance));
    let (_, reservation) = accept_and_reserve(
        &mut loaded.domain.registry,
        &task.acceptance.policy,
        task.refund_to,
        &r,
        task.amount,
        until,
        task_id.0.prefix_u64(),
    )?;
    let reservation = reservation.ok_or_else(|| fail("the result is not accepted"))?;
    // Step 7.
    store_assurance(run, loaded)?;
    run.defer(Deferred::Schedule {
        at: until,
        event: ScheduledEvent::RegistryTick,
    });
    // Step 8.
    run.mint(sender, usd(), task.amount)?;
    let amount = task.amount;
    task.state = TaskState::Settled {
        worker: sender,
        result: r,
        reservation,
        at: pulse,
    };
    object.data = ObjectData::Task(Box::new(task));
    run.store(object)?;
    // Step 9.
    run.event(Event::TaskSettled {
        task: task_id,
        worker: sender,
        result: r,
        amount,
        reservation,
    });
    Ok(())
}

/// §5.44 CancelTask.
pub(super) fn cancel(run: &mut TxRun<'_>, task_id: ObjectId) -> Step {
    let pulse = run.ctx.pulse;
    let sender = run.sender();
    // Step 1.
    let (mut object, mut task) = open_task(run, &task_id)?;
    // Step 2.
    let worker_releases = task.worker == Some(sender);
    let poster_withdraws = (sender == task.poster || sender == task.refund_to)
        && (task.worker.is_none() || pulse > task.deadline);
    if !worker_releases && !poster_withdraws {
        return Err(fail("not allowed to cancel this task now"));
    }
    // Step 3.
    let (refund, to) = (task.amount, task.refund_to);
    run.mint(to, usd(), refund)?;
    task.state = TaskState::Cancelled {
        by: sender,
        at: pulse,
    };
    object.data = ObjectData::Task(Box::new(task));
    run.store(object)?;
    // Step 4.
    run.event(Event::TaskCancelled {
        task: task_id,
        refund,
        to,
    });
    Ok(())
}
