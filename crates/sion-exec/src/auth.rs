//! Authorization (spec §4.3): whose policy authorizes a transaction, and
//! whether its signatures satisfy it.
//!
//! A transaction is signed by itself, or is a member of a **batch**: one
//! authorization over the Merkle root of up to [`MAX_BATCH`] transaction
//! digests ([`crate::tx::sign_batch`]). A batch member's sender is its
//! batch's signer, or a keyless subagent ([`crate::charter::keyless_account`])
//! whose nearest keyed ancestor signed the batch.
//!
//! Finding the policy does all the reading; checking it reads nothing, so
//! what a transaction reads never depends on whether its signatures verify.
//! Validators that checked an authorization when they locked its inputs
//! may skip the check when executing, never the reads.

use sion_core::hash::Hash32;
use sion_core::ids::{AccountId, ObjectId};
use sion_core::merkle;
use sion_crypto::AuthPolicy;

use crate::charter::{is_keyless, keyless_account, MAX_CHARTER_CHAIN};
use crate::error::ExecError;
use crate::object::{well_known, Object, ObjectData};
use crate::state::ChainConfig;
use crate::tx::{
    batch_leaf, batch_signing_context, signing_context, BatchMember, SignedTransaction, MAX_BATCH,
};

/// Whose policy authorizes a transaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Authority {
    /// The account whose policy it is: the sender, or for a keyless sender
    /// the nearest keyed ancestor that signed its batch.
    pub signer: AccountId,
    /// The policy.
    pub policy: AuthPolicy,
    /// How many times that account's policy has rotated (0 for a new
    /// account).
    pub rotations: u32,
    /// Whether the sender has no account object yet, so the policy is its
    /// genesis policy and executing the transaction stores the account.
    pub new_account: bool,
}

/// Finds the authorizing policy of `signed` (spec §4.3), reading objects
/// through `read` (`None` for a missing object) in the order the
/// specification gives. The outer error is a failed read; the inner one,
/// why no policy authorizes the transaction.
pub fn find_authority(
    signed: &SignedTransaction,
    read: &mut dyn FnMut(&ObjectId) -> Result<Option<Object>, ExecError>,
) -> Result<Result<Authority, String>, ExecError> {
    let tx = &signed.tx;
    if let Some(BatchMember {
        batch,
        charter: Some(charter_id),
        ..
    }) = &signed.batch
    {
        // A keyless subagent: its nearest keyed ancestor signs for it.
        if tx.sender != keyless_account(charter_id) {
            return Ok(Err(
                "the sender is not the keyless account of the charter it names".into(),
            ));
        }
        let charter = match read(charter_id)? {
            Some(Object {
                data: ObjectData::Charter(charter),
                ..
            }) => charter,
            Some(_) => return Ok(Err(format!("{charter_id} is not a charter"))),
            None => return Ok(Err(format!("charter {charter_id} does not exist"))),
        };
        if charter.agent != tx.sender {
            return Ok(Err("the charter's agent is not the sender".into()));
        }
        let mut next = charter.parent;
        let mut loaded = 0;
        let ancestor = loop {
            let Some(id) = next else {
                return Ok(Err("the subagent has no keyed ancestor".into()));
            };
            if loaded == MAX_CHARTER_CHAIN - 1 {
                return Ok(Err(format!(
                    "no keyed ancestor within {} charters",
                    MAX_CHARTER_CHAIN - 1
                )));
            }
            loaded += 1;
            let parent = match read(&id)? {
                Some(Object {
                    data: ObjectData::Charter(parent),
                    ..
                }) => parent,
                Some(_) => return Ok(Err(format!("{id} is not a charter"))),
                None => return Ok(Err(format!("charter {id} does not exist"))),
            };
            if !is_keyless(&id, &parent) {
                break parent;
            }
            next = parent.parent;
        };
        if ancestor.agent != batch.signer {
            return Ok(Err(
                "the batch is not signed by the subagent's nearest keyed ancestor".into(),
            ));
        }
        return Ok(match read(&well_known::account(&batch.signer))? {
            Some(Object {
                data: ObjectData::Account(state),
                ..
            }) => Ok(Authority {
                signer: batch.signer,
                policy: state.policy,
                rotations: state.rotations,
                new_account: false,
            }),
            Some(_) => Err("the signer's account object is not an account".into()),
            None => Err("the signer has no account yet".into()),
        });
    }
    // The sender's own policy.
    if let Some(member) = &signed.batch {
        if member.batch.signer != tx.sender {
            return Ok(Err("the batch is signed by another account".into()));
        }
    }
    Ok(match read(&well_known::account(&tx.sender))? {
        Some(Object {
            data: ObjectData::Account(state),
            ..
        }) => Ok(Authority {
            signer: tx.sender,
            policy: state.policy,
            rotations: state.rotations,
            new_account: false,
        }),
        Some(_) => Err("account object corrupted".into()),
        None => match &tx.genesis_policy {
            None => Err("unknown account; genesis policy required".into()),
            Some(policy) if policy.validate().is_err() => Err("invalid genesis policy".into()),
            Some(policy) if policy.account_id() != tx.sender => {
                Err("genesis policy does not hash to the sender".into())
            }
            Some(policy) => Ok(Authority {
                signer: tx.sender,
                policy: policy.clone(),
                rotations: 0,
                new_account: true,
            }),
        },
    })
}

/// What `signed.auth` must sign (spec §4.3): the signing context and the
/// message, the transaction's digest or its batch's message.
pub fn signed_message(config: &ChainConfig, signed: &SignedTransaction) -> (String, Hash32) {
    match &signed.batch {
        None => (signing_context(config), signed.digest().0),
        Some(member) => (batch_signing_context(config), member.batch.message()),
    }
}

/// A batch member's place in its batch (spec §4.3): the batch holds 1 to
/// [`MAX_BATCH`] transactions and the member's path leads from its leaf to
/// the batch's root. Nothing to check for a transaction signed by itself.
pub fn check_membership(signed: &SignedTransaction) -> Result<(), String> {
    let Some(member) = &signed.batch else {
        return Ok(());
    };
    let batch = &member.batch;
    if batch.size == 0 || batch.size > MAX_BATCH {
        return Err(format!(
            "a batch holds 1 to {MAX_BATCH} transactions, not {}",
            batch.size
        ));
    }
    merkle::verify_inclusion(
        &batch.root,
        batch.size,
        member.index,
        &batch_leaf(&signed.digest()),
        &member.path,
    )
    .map_err(|e| format!("not in its batch: {e}"))
}

/// The check of spec §4.3 under `policy`: the member's place in its batch,
/// then the signatures. Reads nothing.
///
/// A batch's members carry the same signatures, so the signatures of a
/// batch that verified are remembered on this thread, by batch, with the
/// policy and signatures compared byte for byte: a batch of 256 costs one
/// signature check, not 256. Checking signatures is a pure function, so
/// remembering a success changes no outcome.
pub fn check_authorization(
    config: &ChainConfig,
    signed: &SignedTransaction,
    policy: &AuthPolicy,
) -> Result<(), String> {
    check_membership(signed)?;
    let (context, message) = signed_message(config, signed);
    let batched = signed.batch.is_some();
    let key = batched.then(|| {
        sion_core::hash::hash_parts(
            "sion/exec/verified-batch",
            &[context.as_bytes(), &message.0],
        )
    });
    if let Some(key) = &key {
        if VERIFIED.with(|memo| memo.borrow().holds(key, policy, &signed.auth)) {
            return Ok(());
        }
    }
    policy
        .verify(&context, &message.0, &signed.auth)
        .map_err(|e| e.to_string())?;
    if let Some(key) = key {
        VERIFIED.with(|memo| memo.borrow_mut().remember(key, policy, &signed.auth));
    }
    Ok(())
}

/// Batches whose signatures verified on this thread.
#[derive(Default)]
struct Verified {
    entries: std::collections::BTreeMap<Hash32, Vec<(AuthPolicy, sion_crypto::Authorization)>>,
    order: std::collections::VecDeque<Hash32>,
}

/// How many batches a thread remembers (each entry holds about 21 KB).
const VERIFIED_LIMIT: usize = 256;

impl Verified {
    fn holds(&self, key: &Hash32, policy: &AuthPolicy, auth: &sion_crypto::Authorization) -> bool {
        self.entries
            .get(key)
            .is_some_and(|seen| seen.iter().any(|(p, a)| p == policy && a == auth))
    }

    fn remember(&mut self, key: Hash32, policy: &AuthPolicy, auth: &sion_crypto::Authorization) {
        if self.holds(&key, policy, auth) {
            return;
        }
        let seen = self.entries.entry(key).or_default();
        if seen.is_empty() {
            self.order.push_back(key);
        }
        seen.push((policy.clone(), auth.clone()));
        while self.order.len() > VERIFIED_LIMIT {
            if let Some(old) = self.order.pop_front() {
                self.entries.remove(&old);
            }
        }
    }
}

thread_local! {
    static VERIFIED: std::cell::RefCell<Verified> = std::cell::RefCell::new(Verified::default());
}

#[cfg(test)]
mod tests {
    use sion_core::ids::{AssetId, Pulse, Version};
    use sion_core::Amount;
    use sion_crypto::DualKeypair;

    use super::*;
    use crate::object::ObjectRef;
    use crate::tx::{sign_batch, Transaction, TxKind};

    fn body(sender: AccountId, nonce: u64) -> Transaction {
        Transaction {
            sender,
            genesis_policy: None,
            inputs: vec![ObjectRef {
                id: ObjectId::derive(&nonce.to_be_bytes()),
                version: Version(0),
            }],
            fee: Amount(1_000),
            kind: TxKind::Transfer {
                asset: AssetId::usd(),
                outputs: vec![(sender, Amount(1))],
            },
            expires_at: Pulse(10),
            nonce,
        }
    }

    #[test]
    fn a_batch_signature_covers_every_member_and_nothing_else() {
        let keys = DualKeypair::from_seed(&[7; 32], sion_crypto::Scheme::SlhDsaSha2_128f);
        let signer = keys.policy().account_id();
        let config = ChainConfig::devnet(None);
        let members: Vec<_> = (0..5).map(|n| (body(signer, n), None)).collect();
        let signed = sign_batch(&keys, signer, &config, members).unwrap();
        for member in &signed {
            check_authorization(&config, member, &keys.policy()).unwrap();
        }
        // Another body on the same authorization and path is not a member.
        let mut forged = signed[2].clone();
        forged.tx.nonce = 99;
        assert!(check_membership(&forged).is_err());
        // The batch's signature does not verify as a transaction's.
        let mut alone = signed[0].clone();
        alone.batch = None;
        assert!(check_authorization(&config, &alone, &keys.policy()).is_err());
        // Nor on another chain.
        let other = ChainConfig {
            chain_id: "other".into(),
            ..config.clone()
        };
        assert!(check_authorization(&other, &signed[0], &keys.policy()).is_err());
        // A size of zero, or above the limit, is refused before any hashing.
        let mut empty = signed[0].clone();
        empty.batch.as_mut().unwrap().batch.size = 0;
        assert!(check_membership(&empty).is_err());
        let mut huge = signed[0].clone();
        huge.batch.as_mut().unwrap().batch.size = MAX_BATCH + 1;
        assert!(check_membership(&huge).is_err());
    }
}
