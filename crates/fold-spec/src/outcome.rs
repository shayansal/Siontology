//! A transaction's outcome (spec §4.2): success, failure with fee, or
//! rejection.

use fold_exec::execute::{ExecStatus, TxEffects};
use fold_exec::tx::SignedTransaction;

use crate::context::SpecContext;
use crate::error::{Abort, SpecError, Step};
use crate::ops;
use crate::prepare::prepare;
use crate::tx::{rejected_effects, TxRun};
use crate::view::ChunkView;

/// Executes one transaction against `view`, keeping its writes there as
/// §4.2 says, and returns its effects. An error returned here rejects the
/// chunk; a transaction's own errors only decide its outcome.
pub(crate) fn run_transaction(
    ctx: &SpecContext,
    view: &mut ChunkView,
    signed: &SignedTransaction,
) -> Result<TxEffects, SpecError> {
    let digest = signed.digest();

    // Steps 1–2: prepare, then apply, charge the fee, release the inputs.
    let mut run = TxRun::new(ctx, view, signed);
    let prepared = prepare(&mut run);
    let rejected_in_prepare = prepared.is_err();
    let result = prepared.and_then(|()| full_run(&mut run));
    let outcome = run.finish();
    let reason = match result {
        Ok(()) => {
            let effects = outcome.effects(signed, digest, ExecStatus::Success);
            view.note_reads(outcome.parts.reads);
            view.keep(outcome.parts.writes);
            return Ok(effects);
        }
        Err(Abort::Chunk(error)) => return Err(error),
        Err(Abort::Op(error)) => error.0,
    };
    // A discarded run still read what it read.
    view.note_reads(outcome.parts.reads);
    if rejected_in_prepare {
        // Step 1: a rejection writes nothing.
        return Ok(rejected_effects(signed, digest, reason));
    }

    // Step 3: the operation failed. Discard every write and start again with
    // fresh bookkeeping, only charging the fee and releasing the inputs.
    let mut retry = TxRun::new(ctx, view, signed);
    let result = prepare(&mut retry)
        .and_then(|()| retry.charge_fee_from_gas())
        .and_then(|()| retry.finalize_inputs());
    let outcome = retry.finish();
    view.note_reads(outcome.parts.reads.clone());
    match result {
        Ok(()) => {
            let mut effects = outcome.effects(signed, digest, ExecStatus::Failed(reason));
            effects.events.clear();
            view.keep(outcome.parts.writes);
            Ok(effects)
        }
        Err(Abort::Chunk(error)) => Err(error),
        Err(Abort::Op(error)) => Ok(rejected_effects(signed, digest, error.0)),
    }
}

/// Step 2 of §4.2, after a successful prepare.
fn full_run(run: &mut TxRun<'_>) -> Step {
    ops::apply(run)?;
    run.charge_fee_from_gas()?;
    run.finalize_inputs()
}
