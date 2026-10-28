//! Payment streams: OpenStream (§5.31), SettleStream (§5.32), TopUpStream
//! (§5.33) and CloseStream (§5.34).
//!
//! Written from the specification: the chain step is `H("sion/stream/step",
//! x)` computed with the shared hashing primitive, a checkpoint is checked
//! with the shared Merkle tree hash, and the constants are those §5.31 names
//! in `sion_exec::stream`.

use sion_core::hash::{hash_domain, Hash32};
use sion_core::ids::{AccountId, AssetId, ObjectId, Pulse};
use sion_core::{merkle, Amount};
use sion_exec::execute::Event;
use sion_exec::market::MAX_PERIOD;
use sion_exec::object::{Object, ObjectData, Owner};
use sion_exec::stream::{
    Checkpoints, Jump, Stream, CHECKPOINT_TAG, MAX_CHECKPOINTS, MAX_CHECKPOINT_DEPTH,
    MAX_SETTLE_STEPS, MAX_STREAM_LENGTH, STEP_DOMAIN, STREAM_STEP_FEE,
};
use sion_exec::tx::object_domain;

use crate::error::{fail, Step};
use crate::tx::TxRun;

/// The terms of §5.31.
pub(super) struct OpenTerms {
    pub(super) payee: AccountId,
    pub(super) asset: AssetId,
    pub(super) unit: Amount,
    pub(super) length: u64,
    pub(super) anchor: Hash32,
    pub(super) deposit: Amount,
    pub(super) until: Pulse,
    pub(super) checkpoints: Option<Checkpoints>,
}

/// The latest Pulse a stream may last to, opened or extended now:
/// `ctx.pulse + MAX_PERIOD`, saturating.
fn latest(run: &TxRun<'_>) -> u64 {
    run.ctx.pulse.0.saturating_add(MAX_PERIOD)
}

/// Loads `id`, which must be a stream that is still open.
fn open_stream(run: &mut TxRun<'_>, id: &ObjectId) -> Step<(Object, Stream)> {
    let object = run.load(id)?;
    let stream = match &object.data {
        ObjectData::Stream(stream) => stream.clone(),
        _ => return Err(fail(format!("{id} is not a stream"))),
    };
    if !stream.open {
        return Err(fail("the stream is closed"));
    }
    Ok((object, stream))
}

/// Writes `stream` back as `object`'s data.
fn put(run: &mut TxRun<'_>, mut object: Object, stream: Stream) -> Step {
    object.data = ObjectData::Stream(stream);
    run.store(object)
}

/// §5.31 OpenStream.
pub(super) fn open(run: &mut TxRun<'_>, terms: OpenTerms) -> Step {
    // Step 1.
    if terms.unit == Amount::ZERO {
        return Err(fail("the unit must be positive"));
    }
    if terms.length < 1 || terms.length > MAX_STREAM_LENGTH {
        return Err(fail(format!(
            "the length must be between 1 and {MAX_STREAM_LENGTH}"
        )));
    }
    if terms.deposit == Amount::ZERO {
        return Err(fail("the deposit must be positive"));
    }
    if !(run.ctx.pulse.0 < terms.until.0 && terms.until.0 <= latest(run)) {
        return Err(fail("until is out of range"));
    }
    if let Some(checkpoints) = &terms.checkpoints {
        let spacing = checkpoints.spacing;
        if !(1 <= spacing && spacing <= terms.length && terms.length / spacing < MAX_CHECKPOINTS) {
            return Err(fail("the checkpoint spacing is out of range"));
        }
    }
    // Step 2.
    run.draw(terms.asset, terms.deposit)?;
    // Step 3.
    let stream_id = run.next_created_id();
    let payer = run.sender();
    run.create(
        Owner::Shared(object_domain("stream", &stream_id)),
        ObjectData::Stream(Stream {
            payer,
            payee: terms.payee,
            asset: terms.asset,
            unit: terms.unit,
            length: terms.length,
            position: 0,
            head: terms.anchor,
            escrow: terms.deposit,
            until: terms.until,
            open: true,
            spacing: terms.checkpoints.map_or(0, |c| c.spacing),
            checkpoints: terms.checkpoints.map_or(Hash32::ZERO, |c| c.root),
        }),
    )?;
    // Step 4.
    run.event(Event::StreamOpened {
        stream: stream_id,
        payer,
        payee: terms.payee,
        deposit: terms.deposit,
    });
    Ok(())
}

/// `k` chain steps from `x`.
fn steps_from(mut x: Hash32, k: u64) -> Hash32 {
    for _ in 0..k {
        x = hash_domain(STEP_DOMAIN, x.as_bytes());
    }
    x
}

/// §5.32 SettleStream.
pub(super) fn settle(
    run: &mut TxRun<'_>,
    id: ObjectId,
    position: u64,
    value: Hash32,
    jump: Option<&Jump>,
) -> Step {
    // Step 1.
    let (object, mut stream) = open_stream(run, &id)?;
    if stream.payee != run.sender() {
        return Err(fail("only the payee settles"));
    }
    if run.ctx.pulse > stream.until {
        return Err(fail("the stream is past its last Pulse"));
    }
    // Step 2.
    if !(stream.position < position && position <= stream.length) {
        return Err(fail("the position is out of range"));
    }
    let steps = position - stream.position;
    // Step 3: the hashes the check costs.
    let hashes = match jump {
        None => {
            if steps > MAX_SETTLE_STEPS {
                return Err(fail(format!("more than {MAX_SETTLE_STEPS} steps")));
            }
            steps
        }
        Some(jump) => {
            if stream.spacing == 0 {
                return Err(fail("the stream has no checkpoints"));
            }
            if jump.path.len() > MAX_CHECKPOINT_DEPTH {
                return Err(fail("the checkpoint path is too long"));
            }
            let base = jump
                .index
                .checked_mul(stream.spacing)
                .ok_or_else(|| fail("the checkpoint is out of range"))?;
            if !(base <= position && position - base < stream.spacing) {
                return Err(fail("the checkpoint is not just below the position"));
            }
            (position - base) + 1 + jump.path.len() as u64
        }
    };
    // Step 4.
    let floor = run
        .ctx
        .config
        .min_fee(run.tx.size())
        .0
        .saturating_add(STREAM_STEP_FEE.0.saturating_mul(u128::from(hashes)));
    if run.tx.fee.0 < floor {
        return Err(fail("the fee does not cover the hashes"));
    }
    // Step 5.
    match jump {
        None => {
            if steps_from(value, steps) != stream.head {
                return Err(fail("the value does not hash down to the head"));
            }
        }
        Some(jump) => {
            let base = jump.index * stream.spacing;
            let checkpoint = steps_from(value, position - base);
            let mut data = CHECKPOINT_TAG.to_vec();
            data.extend_from_slice(&jump.index.to_be_bytes());
            data.extend_from_slice(checkpoint.as_bytes());
            let leaf = merkle::leaf_hash(&data);
            let count = stream.length / stream.spacing + 1;
            if merkle::verify_inclusion(&stream.checkpoints, count, jump.index, &leaf, &jump.path)
                .is_err()
            {
                return Err(fail("the checkpoint is not in the stream's tree"));
            }
        }
    }
    // Step 6.
    let amount = stream
        .unit
        .0
        .checked_mul(u128::from(steps))
        .map(Amount)
        .ok_or_else(|| fail("unit × steps overflows"))?;
    if amount > stream.escrow {
        return Err(fail("the escrow does not cover the steps"));
    }
    // Step 7.
    stream.escrow = stream.escrow.checked_sub(amount)?;
    stream.position = position;
    stream.head = value;
    let (payee, asset) = (stream.payee, stream.asset);
    put(run, object, stream)?;
    run.mint(payee, asset, amount)?;
    // Step 8.
    run.event(Event::StreamSettled {
        stream: id,
        position,
        head: value,
        amount,
    });
    Ok(())
}

/// §5.33 TopUpStream.
pub(super) fn top_up(run: &mut TxRun<'_>, id: ObjectId, deposit: Amount, until: Pulse) -> Step {
    // Step 1.
    let (object, mut stream) = open_stream(run, &id)?;
    if stream.payer != run.sender() {
        return Err(fail("only the payer tops up"));
    }
    if run.ctx.pulse > stream.until {
        return Err(fail("the stream is past its last Pulse"));
    }
    // Step 2.
    if !(stream.until <= until && until.0 <= latest(run)) {
        return Err(fail("until is out of range"));
    }
    if deposit == Amount::ZERO && until <= stream.until {
        return Err(fail("nothing to add"));
    }
    // Step 3.
    if deposit > Amount::ZERO {
        run.draw(stream.asset, deposit)?;
        stream.escrow = stream.escrow.checked_add(deposit)?;
    }
    // Step 4.
    stream.until = until;
    put(run, object, stream)?;
    run.event(Event::StreamToppedUp {
        stream: id,
        deposit,
        until,
    });
    Ok(())
}

/// §5.34 CloseStream.
pub(super) fn close(run: &mut TxRun<'_>, id: ObjectId) -> Step {
    // Step 1.
    let (object, mut stream) = open_stream(run, &id)?;
    let sender = run.sender();
    let allowed =
        sender == stream.payee || (sender == stream.payer && run.ctx.pulse > stream.until);
    if !allowed {
        return Err(fail(
            "only the payee, or the payer after the last Pulse, closes",
        ));
    }
    // Step 2.
    let refund = stream.escrow;
    let (payer, asset) = (stream.payer, stream.asset);
    stream.escrow = Amount::ZERO;
    stream.open = false;
    put(run, object, stream)?;
    run.mint(payer, asset, refund)?;
    // Step 3.
    run.event(Event::StreamClosed { stream: id, refund });
    Ok(())
}
