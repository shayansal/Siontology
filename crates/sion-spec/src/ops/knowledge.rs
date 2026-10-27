//! Knowledge-class operations: IssueClaim (§5.4), RegisterAtom (§5.5),
//! PostBond (§5.6). They only defer work to the end of the Pulse.

use sion_assurance::{AtomClass, Formula};
use sion_core::Amount;
use sion_exec::claim::{claim_id, ClaimBody};
use sion_exec::execute::{bond_atom, registered_atom, Deferred, Event};
use sion_exec::state::ChainConfig;

use crate::error::{fail, Step};
use crate::tx::{usd, TxRun};

/// Longest schema or atom label, in bytes.
const MAX_LABEL_BYTES: usize = 128;
/// Longest claim subject, in bytes.
const MAX_SUBJECT_BYTES: usize = 512;
/// The largest detection factor, in thousandths.
const MAX_DETECTION_PERMILLE: u16 = 1000;

/// The body checks of §5.4 step 1.
pub(super) fn validate_claim_body(config: &ChainConfig, body: &ClaimBody) -> Step {
    if body.schema.is_empty() || body.schema.len() > MAX_LABEL_BYTES {
        return Err(fail("schema must be 1 to 128 bytes"));
    }
    if body.subject.len() > MAX_SUBJECT_BYTES {
        return Err(fail("subject longer than 512 bytes"));
    }
    if body.payload.size() > config.max_payload_bytes {
        return Err(fail("payload too large"));
    }
    if body.formula.size() > config.max_formula_size {
        return Err(fail("formula too large"));
    }
    if body.detection_permille > MAX_DETECTION_PERMILLE {
        return Err(fail("detection factor above one"));
    }
    if body.valid_for == 0 || body.valid_for > config.max_claim_window {
        return Err(fail("validity window out of range"));
    }
    if body.formula == Formula::False {
        return Err(fail("a claim resting on ⊥ never holds"));
    }
    Ok(())
}

/// §5.4 IssueClaim: validates the body and defers it to the knowledge phase.
pub(super) fn issue_claim(run: &mut TxRun<'_>, claim: &ClaimBody) -> Step {
    validate_claim_body(&run.ctx.config, claim)?;
    let sender = run.sender();
    run.defer(Deferred::IssueClaim {
        issuer: sender,
        backer: sender,
        body: claim.clone(),
    });
    run.event(Event::ClaimSubmitted {
        claim: claim_id(&sender, claim),
    });
    Ok(())
}

/// §5.5 RegisterAtom: defers registering an unbonded code or hardware atom
/// in the sender's namespace.
pub(super) fn register_atom(run: &mut TxRun<'_>, class: AtomClass, label: &str) -> Step {
    if !matches!(class, AtomClass::Code | AtomClass::Hardware) {
        return Err(fail("only code and hardware atoms can be registered"));
    }
    if label.is_empty() || label.len() > MAX_LABEL_BYTES {
        return Err(fail("label must be 1 to 128 bytes"));
    }
    let atom = registered_atom(class, &run.sender(), label);
    run.defer(Deferred::RegisterAtom { atom });
    Ok(())
}

/// §5.6 PostBond: draws the bond from the sender's USD coins and defers
/// posting it behind the sender's bonded atom.
pub(super) fn post_bond(
    run: &mut TxRun<'_>,
    class: AtomClass,
    label: &str,
    amount: Amount,
) -> Step {
    if !matches!(
        class,
        AtomClass::Economic | AtomClass::Institutional | AtomClass::Adjudicator
    ) {
        return Err(fail(
            "only economic, institutional and adjudicator atoms are bonded",
        ));
    }
    if amount == Amount::ZERO {
        return Err(fail("zero bond"));
    }
    run.draw(usd(), amount)?;
    let owner = run.sender();
    run.defer(Deferred::PostBond {
        atom: bond_atom(class, &owner, label),
        owner,
        amount,
    });
    Ok(())
}
