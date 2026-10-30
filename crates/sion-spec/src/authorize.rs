//! Authorization (spec §4.3), written from the specification: the fee
//! floor, the authorizing policy and the check. Preparation (§4.1) and the
//! admissibility of forced intents (§6.3) both use it, the first reading
//! with `try_load` and `load`, the second with `try_load` only.
//!
//! The order of reads is the specification's: a keyless sender's charter,
//! then its ancestors up to the nearest keyed one, then that ancestor's
//! account; otherwise the sender's account. Every equality the text checks
//! before a read is checked before it, so a rejected transaction reads only
//! what its checks needed. The check itself reads nothing.

use sion_core::encode::encode;
use sion_core::hash::{hash_domain, Hash32};
use sion_core::ids::{AccountId, ObjectId};
use sion_core::merkle;
use sion_core::Amount;
use sion_crypto::AuthPolicy;
use sion_exec::charter::{Charter, MAX_LINEAGE_DEPTH};
use sion_exec::object::{well_known, Object, ObjectData};
use sion_exec::state::ChainConfig;
use sion_exec::tx::{SignedTransaction, TxBatch, MAX_BATCH, SIGNATURE_SHARE_PERMILLE, TX_DOMAIN};

use crate::prepare::signing_context;

/// The signing context of batches (§4.3): `TX_DOMAIN`, `/batch/` and the
/// configuration digest in lowercase hex.
pub(crate) fn batch_signing_context(config: &ChainConfig) -> String {
    format!("{TX_DOMAIN}/batch/{}", config.digest().to_hex())
}

/// What a batch's authorization signs: `H("sion/tx/batch", enc(batch))`.
pub(crate) fn batch_message(batch: &TxBatch) -> Hash32 {
    hash_domain("sion/tx/batch", &encode(batch))
}

/// The account of a keyless subagent with charter `charter`:
/// `AccountId(H("sion/agent/keyless", charter bytes))`.
pub(crate) fn keyless(charter: &ObjectId) -> AccountId {
    AccountId(hash_domain(
        "sion/agent/keyless",
        charter.as_hash().as_bytes(),
    ))
}

/// `required_fee(signed)` (§4.3), or `None` if computing the share
/// overflows (an error, so the transaction is rejected).
pub(crate) fn required_fee(config: &ChainConfig, signed: &SignedTransaction) -> Option<Amount> {
    let floor = config.min_fee(signed.tx.size());
    match &signed.batch {
        None => Some(floor),
        Some(member) => {
            let share = config
                .base_fee
                .0
                .checked_mul(SIGNATURE_SHARE_PERMILLE)?
                .checked_div(1000)?;
            let members = u128::from(member.batch.size.max(1));
            // ⌈share / members⌉ without overflow: share < 2¹²⁸ and members ≥ 1.
            let part = share / members + u128::from(share % members != 0);
            floor.0.checked_sub(share)?.checked_add(part).map(Amount)
        }
    }
}

/// The policy found, and whose it is.
#[derive(Debug)]
pub(crate) struct Found {
    /// The account whose policy it is (the intent's signer, §6.3).
    pub signer: AccountId,
    /// The policy.
    pub policy: AuthPolicy,
    /// That account object's rotations; 0 for a sender without one.
    pub rotations: u32,
    /// The sender has no account object: the policy is its genesis policy,
    /// and preparation stores the new account object.
    pub new_account: bool,
}

/// The charter in `object`, if it is one.
fn as_charter(object: Option<Object>) -> Option<Charter> {
    match object?.data {
        ObjectData::Charter(charter) => Some(charter),
        _ => None,
    }
}

/// "The authorizing policy" (§4.3). `read` is `try_load`: `None` for a
/// missing object, which fails the search wherever the text says MUST.
/// The outer error is whatever reading raised (a witness gap); the inner
/// one says why no policy authorizes the transaction.
pub(crate) fn authorizing_policy<E>(
    signed: &SignedTransaction,
    read: &mut impl FnMut(&ObjectId) -> Result<Option<Object>, E>,
) -> Result<Result<Found, String>, E> {
    let tx = &signed.tx;
    let member = signed.batch.as_ref();
    match member.and_then(|m| m.charter.map(|c| (m, c))) {
        // 2. The nearest keyed ancestor's.
        Some((member, c)) => {
            if tx.sender != keyless(&c) {
                return Ok(Err("sender is not keyless_account(c)".into()));
            }
            let Some(own) = as_charter(read(&c)?) else {
                return Ok(Err("c is not a charter".into()));
            };
            if own.agent != tx.sender {
                return Ok(Err("c.agent is not the sender".into()));
            }
            let mut parent = own.parent;
            let mut ancestors = 0usize;
            loop {
                let Some(id) = parent else {
                    return Ok(Err("no keyed ancestor: the chain ends".into()));
                };
                if ancestors >= MAX_LINEAGE_DEPTH {
                    return Ok(Err("no keyed ancestor within the chain limit".into()));
                }
                let Some(ancestor) = as_charter(read(&id)?) else {
                    return Ok(Err("an ancestor is not a charter".into()));
                };
                ancestors += 1;
                if ancestor.agent == keyless(&id) {
                    parent = ancestor.parent;
                    continue;
                }
                if ancestor.agent != member.batch.signer {
                    return Ok(Err(
                        "the nearest keyed ancestor did not sign the batch".into()
                    ));
                }
                break;
            }
            let signer = member.batch.signer;
            match read(&well_known::account(&signer))? {
                Some(Object {
                    data: ObjectData::Account(state),
                    ..
                }) => Ok(Ok(Found {
                    signer,
                    policy: state.policy,
                    rotations: state.rotations,
                    new_account: false,
                })),
                _ => Ok(Err("the signer has no account".into())),
            }
        }
        // 1. The sender's own.
        None => {
            if member.is_some_and(|m| m.batch.signer != tx.sender) {
                return Ok(Err("the batch's signer is not the sender".into()));
            }
            match read(&well_known::account(&tx.sender))? {
                Some(object) => match object.data {
                    ObjectData::Account(state) => Ok(Ok(Found {
                        signer: tx.sender,
                        policy: state.policy,
                        rotations: state.rotations,
                        new_account: false,
                    })),
                    _ => Ok(Err("the account object is not an account".into())),
                },
                None => {
                    let Some(policy) = tx.genesis_policy.clone() else {
                        return Ok(Err("unknown account and no genesis policy".into()));
                    };
                    if policy.validate().is_err() {
                        return Ok(Err("the genesis policy is invalid".into()));
                    }
                    if policy.account_id() != tx.sender {
                        return Ok(Err("the genesis policy does not hash to the sender".into()));
                    }
                    Ok(Ok(Found {
                        signer: tx.sender,
                        policy,
                        rotations: 0,
                        new_account: true,
                    }))
                }
            }
        }
    }
}

/// "The check" (§4.3) under `policy`. Reads nothing.
pub(crate) fn check(
    config: &ChainConfig,
    signed: &SignedTransaction,
    policy: &AuthPolicy,
) -> Result<(), String> {
    let digest = signed.digest();
    let Some(member) = &signed.batch else {
        return policy
            .verify(&signing_context(config), &digest.0 .0, &signed.auth)
            .map_err(|e| e.to_string());
    };
    let batch = &member.batch;
    if !(1..=MAX_BATCH).contains(&batch.size) {
        return Err("the batch's size is out of bounds".into());
    }
    merkle::verify_inclusion(
        &batch.root,
        batch.size,
        member.index,
        &merkle::leaf_hash(&digest.0 .0),
        &member.path,
    )
    .map_err(|e| format!("inclusion: {e}"))?;
    let context = batch_signing_context(config);
    let message = batch_message(batch);
    // Every member of a batch carries the same authorization: once one has
    // verified, the others need only their own inclusion. The policy and
    // signatures are compared in full.
    let seen = CHECKED.with(|checked| {
        checked
            .borrow()
            .iter()
            .any(|(c, m, p, a)| *m == message && *c == context && p == policy && *a == signed.auth)
    });
    if seen {
        return Ok(());
    }
    policy
        .verify(&context, message.as_bytes(), &signed.auth)
        .map_err(|e| e.to_string())?;
    CHECKED.with(|checked| {
        let mut checked = checked.borrow_mut();
        if checked.len() == CHECKED_LIMIT {
            checked.remove(0);
        }
        checked.push((context, message, policy.clone(), signed.auth.clone()));
    });
    Ok(())
}

/// How many verified batch authorizations a thread keeps.
const CHECKED_LIMIT: usize = 64;

thread_local! {
    /// Batch authorizations that verified on this thread: context, message,
    /// policy and signatures. Verification is a pure function of these, so
    /// keeping successes changes no result.
    static CHECKED: std::cell::RefCell<Vec<(String, Hash32, AuthPolicy, sion_crypto::Authorization)>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_helpers_match_the_reference_definitions() {
        let config = ChainConfig::devnet(None);
        assert_eq!(
            batch_signing_context(&config),
            sion_exec::tx::batch_signing_context(&config)
        );
        assert_ne!(batch_signing_context(&config), signing_context(&config));
        let batch = TxBatch {
            signer: AccountId::derive(b"signer"),
            size: 3,
            root: Hash32([9; 32]),
        };
        assert_eq!(batch_message(&batch), batch.message());
        let charter = ObjectId::derive(b"charter");
        assert_eq!(
            keyless(&charter),
            sion_exec::charter::keyless_account(&charter)
        );
    }
}
