//! Coins and accounts: Transfer (§5.1), Faucet (§5.2), RotatePolicy (§5.3).

use fold_core::ids::{AccountId, AssetId};
use fold_core::Amount;
use fold_crypto::{AuthPolicy, KemPublicKey};
use fold_exec::execute::{Deferred, Event};
use fold_exec::object::{well_known, ObjectData};
use fold_exec::tx::MAX_GRANT;

use crate::error::{fail, Step};
use crate::tx::TxRun;

/// Most outputs or grants one transaction may carry.
const MAX_OUTPUTS: usize = 256;

/// §5.1 Transfer: pays every output from the sender's coins of `asset`.
pub(super) fn transfer(
    run: &mut TxRun<'_>,
    asset: AssetId,
    outputs: &[(AccountId, Amount)],
) -> Step {
    if outputs.is_empty() || outputs.len() > MAX_OUTPUTS {
        return Err(fail("a transfer needs 1 to 256 outputs"));
    }
    if outputs.iter().any(|(_, amount)| *amount == Amount::ZERO) {
        return Err(fail("zero output"));
    }
    let total = Amount::sum(outputs.iter().map(|(_, amount)| amount))?;
    run.draw(asset, total)?;
    for (to, amount) in outputs {
        run.mint(*to, asset, *amount)?;
    }
    run.event(Event::Transferred {
        asset,
        outputs: outputs.to_vec(),
    });
    Ok(())
}

/// §5.2 Faucet: mints every grant (each between one micro-unit and
/// `MAX_GRANT`) and records the new supply.
pub(super) fn faucet(run: &mut TxRun<'_>, grants: &[(AccountId, AssetId, Amount)]) -> Step {
    if grants.is_empty() || grants.len() > MAX_OUTPUTS {
        return Err(fail("a faucet transaction needs 1 to 256 grants"));
    }
    for (to, asset, amount) in grants {
        if *amount == Amount::ZERO {
            return Err(fail("zero grant"));
        }
        if *amount > MAX_GRANT {
            return Err(fail(format!("a grant may mint at most {MAX_GRANT}")));
        }
        run.mint(*to, *asset, *amount)?;
        run.defer(Deferred::Minted {
            asset: *asset,
            amount: *amount,
        });
        run.event(Event::Minted {
            to: *to,
            asset: *asset,
            amount: *amount,
        });
    }
    Ok(())
}

/// §5.3 RotatePolicy: replaces the account's policy and viewing key; the
/// account identifier stays.
pub(super) fn rotate_policy(
    run: &mut TxRun<'_>,
    policy: &AuthPolicy,
    viewing_key: &Option<KemPublicKey>,
) -> Step {
    policy.validate()?;
    let sender = run.sender();
    let account = well_known::account(&sender);
    if !run.tx.inputs.iter().any(|input| input.id == account) {
        return Err(fail("the account object must be an input"));
    }
    let mut object = run.load(&account)?;
    let ObjectData::Account(state) = &mut object.data else {
        return Err(fail("the account object is not an account"));
    };
    state.policy = policy.clone();
    state.viewing_key = viewing_key.clone();
    state.rotations = state
        .rotations
        .checked_add(1)
        .ok_or_else(|| fail("rotation counter overflow"))?;
    run.store(object)?;
    run.event(Event::PolicyRotated { account: sender });
    Ok(())
}
