//! Bounded totals: CreateBounded (§5.18), SpendAllotment (§5.19),
//! ReturnAllotment (§5.20) and Rebalance (§5.21).

use std::collections::BTreeMap;

use fold_core::ids::{AccountId, AssetId, ObjectId};
use fold_core::Amount;
use fold_exec::charter::Allotment;
use fold_exec::escrow::{BoundedTotal, MAX_ALLOTMENTS};
use fold_exec::execute::{Deferred, Event};
use fold_exec::object::{ObjectData, Owner};
use fold_exec::tx::object_domain;

use crate::error::{fail, Step};
use crate::ops::spend;
use crate::tx::TxRun;

/// A fresh allotment of a bounded total (no caveats).
fn bounded_allotment(
    source: ObjectId,
    writer: AccountId,
    asset: AssetId,
    remaining: Amount,
    run: &TxRun<'_>,
) -> ObjectData {
    ObjectData::Allotment(Allotment {
        source,
        writer,
        asset,
        remaining,
        caveats: None,
        window_start: run.ctx.pulse,
        spent_in_window: Amount::ZERO,
    })
}

/// The caveat-free allotment `tx.inputs[1]`, loaded.
fn plain_allotment_input(run: &mut TxRun<'_>) -> Step<(fold_exec::object::Object, Allotment)> {
    let id = run
        .tx
        .inputs
        .get(1)
        .map(|input| input.id)
        .ok_or_else(|| fail("the allotment must be the second input"))?;
    let object = run.load(&id)?;
    let allotment = match &object.data {
        ObjectData::Allotment(allotment) if allotment.caveats.is_none() => allotment.clone(),
        ObjectData::Allotment(_) => return Err(fail("charter allotments are spent with AgentPay")),
        _ => return Err(fail(format!("{id} is not an allotment"))),
    };
    Ok((object, allotment))
}

/// §5.18 CreateBounded: escrows `total` and splits it into single-writer
/// allotments.
pub(super) fn create_bounded(
    run: &mut TxRun<'_>,
    label: &str,
    asset: AssetId,
    total: Amount,
    allotments: &[(AccountId, Amount)],
) -> Step {
    if allotments.len() > MAX_ALLOTMENTS {
        return Err(fail(format!("at most {MAX_ALLOTMENTS} allotments")));
    }
    let allotted = Amount::sum(allotments.iter().map(|(_, amount)| amount))?;
    if allotted > total {
        return Err(fail("allotments exceed the total"));
    }
    run.draw(asset, total)?;
    let bounded_id = run.next_created_id();
    run.create(
        Owner::Shared(object_domain("bounded", &bounded_id)),
        ObjectData::Bounded(BoundedTotal {
            label: label.to_owned(),
            asset,
            manager: run.sender(),
            total,
            unallocated: total.checked_sub(allotted)?,
            allotments: BTreeMap::new(),
            returned: Amount::ZERO,
        }),
    )?;
    let mut record = BTreeMap::new();
    let mut ids = Vec::with_capacity(allotments.len());
    for (writer, amount) in allotments {
        let data = bounded_allotment(bounded_id, *writer, asset, *amount, run);
        let id = run.create(Owner::Account(*writer), data)?;
        record.insert(id, (*writer, *amount));
        ids.push(id);
    }
    let mut object = run.load(&bounded_id)?;
    let ObjectData::Bounded(state) = &mut object.data else {
        return Err(fail("the new bounded total is not a bounded total"));
    };
    state.allotments = record;
    run.store(object)?;
    run.event(Event::BoundedCreated {
        bounded: bounded_id,
        allotments: ids,
    });
    Ok(())
}

/// §5.19 SpendAllotment: the writer pays from its allotment.
pub(super) fn spend_allotment(run: &mut TxRun<'_>, to: AccountId, amount: Amount) -> Step {
    let (mut object, mut allotment) = plain_allotment_input(run)?;
    if allotment.writer != run.sender() {
        return Err(fail("only the allotment's writer may spend it"));
    }
    spend(&mut allotment, run.ctx.pulse, &to, amount, None)?;
    let asset = allotment.asset;
    let id = object.id;
    object.data = ObjectData::Allotment(allotment);
    run.store(object)?;
    run.mint(to, asset, amount)?;
    run.event(Event::AllotmentUsed {
        allotment: id,
        amount,
    });
    Ok(())
}

/// §5.20 ReturnAllotment: consumes the allotment and defers returning its
/// remainder to the bounded total.
pub(super) fn return_allotment(run: &mut TxRun<'_>) -> Step {
    let (object, allotment) = plain_allotment_input(run)?;
    run.consume(&object.id)?;
    run.defer(Deferred::ReturnToBounded {
        bounded: allotment.source,
        amount: allotment.remaining,
    });
    run.event(Event::AllotmentUsed {
        allotment: object.id,
        amount: allotment.remaining,
    });
    Ok(())
}

/// §5.21 Rebalance: the manager issues a new allotment from the
/// unallocated amount.
pub(super) fn rebalance(
    run: &mut TxRun<'_>,
    bounded: ObjectId,
    holder: AccountId,
    amount: Amount,
) -> Step {
    let mut object = run.load(&bounded)?;
    let ObjectData::Bounded(state) = &mut object.data else {
        return Err(fail(format!("{bounded} is not a bounded total")));
    };
    if state.manager != run.sender() {
        return Err(fail("only the manager may rebalance"));
    }
    state.unallocated = state
        .unallocated
        .checked_sub(amount)
        .map_err(|_| fail("insufficient unallocated amount"))?;
    let allotment = run.next_created_id();
    state.allotments.insert(allotment, (holder, amount));
    let asset = state.asset;
    run.store(object)?;
    let data = bounded_allotment(bounded, holder, asset, amount, run);
    run.create(Owner::Account(holder), data)?;
    run.event(Event::AllotmentUsed { allotment, amount });
    Ok(())
}
