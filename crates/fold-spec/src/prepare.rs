//! Preparing a transaction (spec §4.1). Any failure here is a rejection:
//! the transaction should never have been certified, so it changes nothing.

use std::collections::BTreeSet;

use fold_core::ids::Version;
use fold_core::Amount;
use fold_exec::account::AccountState;
use fold_exec::object::{well_known, Object, ObjectData, Owner};
use fold_exec::tx::{TxKind, TX_DOMAIN};

use crate::error::{fail, Step};
use crate::tx::{usd, TxRun};

/// Runs the checks of §4.1 in order, stopping at the first failure (so a
/// rejected transaction reads only what its checks needed).
pub(crate) fn prepare(run: &mut TxRun<'_>) -> Step {
    let tx = run.tx;
    let ctx = run.ctx;
    let is_faucet = matches!(tx.kind, TxKind::Faucet { .. });

    // 1. Expiry.
    if ctx.pulse > tx.expires_at {
        return Err(fail("transaction expired"));
    }
    // 2. Inputs present and distinct.
    if tx.inputs.is_empty() {
        return Err(fail("no inputs"));
    }
    let distinct: BTreeSet<_> = tx.inputs.iter().map(|input| input.id).collect();
    if distinct.len() != tx.inputs.len() {
        return Err(fail("duplicate input"));
    }
    // 3. Fees: the faucet pays none; everyone else pays at least the floor.
    if is_faucet {
        if tx.fee != Amount::ZERO {
            return Err(fail("faucet transactions pay no fee"));
        }
    } else if tx.fee < ctx.config.min_fee(tx.size()) {
        return Err(fail("fee below the minimum"));
    }
    // 4. The sender's account and policy.
    let account = well_known::account(&tx.sender);
    let policy = match run.try_load(&account)? {
        Some(object) => match object.data {
            ObjectData::Account(state) => state.policy,
            _ => return Err(fail("the account object is not an account")),
        },
        None => {
            let policy = tx
                .genesis_policy
                .clone()
                .ok_or_else(|| fail("unknown account and no genesis policy"))?;
            policy.validate()?;
            if policy.account_id() != tx.sender {
                return Err(fail("genesis policy does not hash to the sender"));
            }
            run.store(Object {
                id: account,
                version: Version(0),
                owner: Owner::Account(tx.sender),
                data: ObjectData::Account(AccountState {
                    id: tx.sender,
                    policy: policy.clone(),
                    viewing_key: None,
                    created_at: ctx.pulse,
                    rotations: 0,
                }),
            })?;
            policy
        }
    };
    // 5. Authorization.
    policy.verify(TX_DOMAIN, &run.digest.0 .0, &run.signed.auth)?;
    // 6. Inputs.
    for (position, input) in tx.inputs.iter().enumerate() {
        let object = run.load(&input.id)?;
        if object.version != input.version {
            return Err(fail(format!(
                "object {} is at version {}, transaction expects {}",
                input.id, object.version, input.version
            )));
        }
        if object.owner != Owner::Account(tx.sender) {
            return Err(fail(format!("sender does not own object {}", input.id)));
        }
        if object.is_tombstone() {
            return Err(fail(format!("input {} was consumed", input.id)));
        }
        if position == 0 {
            if is_faucet {
                if ctx.config.faucet != Some(tx.sender) {
                    return Err(fail("only the faucet account may mint"));
                }
                if input.id != account {
                    return Err(fail("the faucet's input must be its account object"));
                }
            } else {
                match object.as_coin() {
                    Some(coin) if coin.asset == usd() && coin.amount >= tx.fee => {}
                    _ => return Err(fail("the gas input must be a USD coin covering the fee")),
                }
            }
        }
    }
    Ok(())
}
