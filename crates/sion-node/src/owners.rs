//! What a validator checks before locking owned inputs to a transaction.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use sion_consensus::lanes::canonical_inputs;
use sion_consensus::{LaneRequest, OwnerAuthorization};
use sion_core::encode::{decode, encode};
use sion_core::hash::{hash_parts, Hash32};
use sion_crypto::{AuthPolicy, Authorization};
use sion_exec::auth::{check_authorization, check_membership, find_authority, signed_message};
use sion_exec::object::well_known;
use sion_exec::tx::required_fee;
use sion_exec::{ChainConfig, Owner, SignedTransaction, State};

use crate::payload::lane_inputs;

/// Checks lane requests against a validator's executed state: the request
/// carries the signed transaction; its digest and inputs must match the
/// request (a contended transaction locks its owned inputs too, before
/// consensus orders it), its authorization must satisfy the
/// sender's current policy (or its genesis policy, for a new account), and
/// every input must exist at the given version, owned by the sender and
/// unspent.
#[derive(Debug)]
pub struct StateOwners<'a> {
    /// The validator's state.
    pub state: &'a State,
}

/// Whether a logged lock still matters in `state`: its transaction has not
/// expired and every input it locked still exists, unspent, at the locked
/// version (otherwise the transaction executed, or never can).
pub fn lock_is_live(state: &State, request: &LaneRequest) -> bool {
    let Ok(signed) = decode::<SignedTransaction>(&request.owner_proof) else {
        return false;
    };
    signed.digest() == request.tx
        && signed.tx.expires_at > state.pulse
        && request.inputs.iter().all(|input| {
            state
                .objects
                .get(&input.id)
                .map(|o| o.version == input.version && !o.is_tombstone())
                .unwrap_or(false)
        })
}

/// The most owned inputs a transaction may lock (node policy: lane locks
/// cost memory per input).
pub const MAX_INPUTS: usize = 256;

/// Identifies "these exact authorization bytes satisfy this policy for this
/// message in this context", so a validator that checked them once (when
/// locking the inputs) need not check them again when executing. The message
/// is a transaction's digest or, for a batch member, its batch's message
/// (spec §4.3), so one check serves every member of a batch; each member's
/// place in its batch is checked on its own, which costs a few hashes. The
/// key covers the signatures themselves: the same body or batch with other
/// signatures must be checked afresh.
pub fn authorization_key(
    config: &ChainConfig,
    tx: &SignedTransaction,
    policy: &AuthPolicy,
) -> Hash32 {
    let (context, message) = signed_message(config, tx);
    hash_parts(
        "sion/node/authorized",
        &[
            context.as_bytes(),
            &message.0,
            &encode(policy),
            &encode(&tx.auth),
        ],
    )
}

/// The policy a transaction is checked against in the given state (spec
/// §4.3): the sender's current policy, or its genesis policy if the account
/// is new; for a keyless subagent, its nearest keyed ancestor's.
pub fn policy_for(state: &State, signed: &SignedTransaction) -> Result<AuthPolicy, String> {
    find_authority(signed, &mut |id| Ok(state.objects.get(id).cloned()))
        .map_err(|e| e.to_string())?
        .map(|authority| authority.policy)
}

impl StateOwners<'_> {
    /// Why a request is not authorized, if it is not. On success, returns
    /// the transaction and its [`authorization_key`].
    pub fn check(&self, request: &LaneRequest) -> Result<(SignedTransaction, Hash32), String> {
        let signed = self.decode_request(request)?;
        // Cheap checks first; the signature last.
        self.check_unsigned(&signed)?;
        let policy = policy_for(self.state, &signed)?;
        check_authorization(&self.state.config, &signed, &policy)
            .map_err(|e| format!("authorization: {e}"))?;
        let key = authorization_key(&self.state.config, &signed, &policy);
        Ok((signed, key))
    }

    /// [`StateOwners::check`], skipping signatures `authorized` holds as
    /// verified and remembering those it verifies: a batch's members after
    /// the first cost their own cheap checks and a few hashes each.
    pub fn check_cached(
        &self,
        request: &LaneRequest,
        authorized: &mut Authorized,
    ) -> Result<SignedTransaction, String> {
        let signed = self.decode_request(request)?;
        self.check_unsigned(&signed)?;
        let policy = policy_for(self.state, &signed)?;
        authorized.check(&self.state.config, &signed, &policy)?;
        Ok(signed)
    }

    /// The signed transaction a request carries, if it is the request's.
    fn decode_request(&self, request: &LaneRequest) -> Result<SignedTransaction, String> {
        let signed: SignedTransaction = decode(&request.owner_proof)
            .map_err(|_| "owner proof is not a signed transaction".to_string())?;
        if signed.digest() != request.tx {
            return Err("owner proof is for another transaction".into());
        }
        if canonical_inputs(&lane_inputs(&signed.tx.inputs)) != request.canonical_inputs() {
            return Err("request inputs differ from the transaction's".into());
        }
        Ok(signed)
    }

    /// Everything [`StateOwners::check`] checks except the authorization:
    /// the checks execution's `prepare` makes before charging anything (not
    /// expired; one to [`MAX_INPUTS`] distinct inputs; the fee floor; a gas
    /// coin that covers the fee, or the faucet's own rules), and that every
    /// input exists at the given version, owned by the sender and unspent.
    /// A transaction that passes is certified only if it will pay its fee,
    /// so nobody gets data published for free.
    pub fn check_unsigned(&self, signed: &SignedTransaction) -> Result<(), String> {
        let tx = &signed.tx;
        if tx.expires_at <= self.state.pulse {
            return Err("the transaction has expired".into());
        }
        if tx.inputs.is_empty() || tx.inputs.len() > MAX_INPUTS {
            return Err(format!("a transaction needs 1 to {MAX_INPUTS} inputs"));
        }
        let distinct: BTreeSet<_> = tx.inputs.iter().map(|r| r.id).collect();
        if distinct.len() != tx.inputs.len() {
            return Err("duplicate inputs".into());
        }
        let config = &self.state.config;
        if matches!(tx.kind, sion_exec::TxKind::Faucet { .. }) {
            if config.faucet != Some(tx.sender)
                || tx.fee != sion_core::Amount::ZERO
                || tx.inputs[0].id != well_known::account(&tx.sender)
            {
                return Err("not a valid faucet transaction".into());
            }
        } else {
            let floor = required_fee(config, signed).ok_or("the fee floor overflows")?;
            if tx.fee < floor {
                return Err(format!("fee {} below minimum {floor}", tx.fee));
            }
            let gas = self
                .state
                .objects
                .get(&tx.inputs[0].id)
                .and_then(|o| o.as_coin());
            match gas {
                Some(coin)
                    if coin.asset == sion_core::ids::AssetId::usd() && coin.amount >= tx.fee => {}
                _ => return Err("the first input must be a USD coin covering the fee".into()),
            }
        }
        let sender = tx.sender;
        for input in &tx.inputs {
            let object = self
                .state
                .objects
                .get(&input.id)
                .ok_or_else(|| format!("input {} does not exist (yet)", input.id.short()))?;
            if object.version != input.version {
                return Err(format!(
                    "input {} is at version {}, not {}",
                    input.id.short(),
                    object.version,
                    input.version
                ));
            }
            if object.owner != Owner::Account(sender) {
                return Err(format!("input {} is not the sender's", input.id.short()));
            }
            if object.is_tombstone() {
                return Err(format!("input {} is spent", input.id.short()));
            }
        }
        Ok(())
    }

    /// Whether a request failed only because this validator is behind: an
    /// input does not exist yet, or exists at an older version than the
    /// transaction names. Such requests are worth retrying after the next
    /// Pulse instead of refusing.
    pub fn is_behind(&self, signed: &SignedTransaction) -> bool {
        signed
            .tx
            .inputs
            .iter()
            .any(|input| match self.state.objects.get(&input.id) {
                None => true,
                Some(object) => object.version < input.version,
            })
    }

    /// Whether a transaction would be approved now: [`StateOwners::check`]
    /// without its encoding checks, consulting `authorized` before
    /// verifying a signature (and remembering what verifies).
    pub fn admissible(
        &self,
        signed: &SignedTransaction,
        authorized: &mut Authorized,
    ) -> Result<(), String> {
        self.check_unsigned(signed)?;
        let policy = policy_for(self.state, signed)?;
        authorized.check(&self.state.config, signed, &policy)
    }
}

impl OwnerAuthorization for StateOwners<'_> {
    fn authorizes(&self, request: &LaneRequest) -> bool {
        self.check(request).is_ok()
    }
}

/// Where a batch's authorization is remembered: its signing context and
/// message.
fn batch_key(config: &ChainConfig, tx: &SignedTransaction) -> Hash32 {
    let (context, message) = signed_message(config, tx);
    hash_parts(
        "sion/node/authorized-batch",
        &[context.as_bytes(), &message.0],
    )
}

/// Authorizations a validator has verified; the oldest are forgotten
/// first. A transaction signed by itself is remembered by its
/// [`authorization_key`]. A batch's authorization (spec §4.3) is remembered
/// by its batch with the policy and signatures that verified, compared byte
/// for byte, so each of its members costs its own place in the batch (a few
/// hashes), not a hash over 20 KB of signatures.
#[derive(Debug, Default)]
pub struct Authorized {
    keys: BTreeSet<Hash32>,
    order: VecDeque<Hash32>,
    batches: BTreeMap<Hash32, Vec<(AuthPolicy, Authorization)>>,
    batch_order: VecDeque<Hash32>,
}

impl Authorized {
    /// How many transactions' authorizations are remembered.
    pub const LIMIT: usize = 200_000;

    /// How many batches' authorizations are remembered.
    pub const BATCH_LIMIT: usize = 4_096;

    /// Whether this authorization was verified (a transaction signed by
    /// itself, by its [`authorization_key`]).
    pub fn contains(&self, key: &Hash32) -> bool {
        self.keys.contains(key)
    }

    /// Remembers a verified authorization of a transaction signed by itself.
    pub fn insert(&mut self, key: Hash32) {
        if self.keys.insert(key) {
            self.order.push_back(key);
            while self.order.len() > Self::LIMIT {
                if let Some(old) = self.order.pop_front() {
                    self.keys.remove(&old);
                }
            }
        }
    }

    /// Whether `tx`'s exact signatures were verified under `policy`. For a
    /// batch member this says nothing of its place in the batch.
    pub fn verified(
        &self,
        config: &ChainConfig,
        tx: &SignedTransaction,
        policy: &AuthPolicy,
    ) -> bool {
        if tx.batch.is_none() {
            return self.contains(&authorization_key(config, tx, policy));
        }
        self.batches
            .get(&batch_key(config, tx))
            .is_some_and(|seen| seen.iter().any(|(p, auth)| p == policy && *auth == tx.auth))
    }

    /// Remembers that `tx`'s signatures verified under `policy`.
    pub fn remember(&mut self, config: &ChainConfig, tx: &SignedTransaction, policy: &AuthPolicy) {
        if tx.batch.is_none() {
            self.insert(authorization_key(config, tx, policy));
            return;
        }
        if self.verified(config, tx, policy) {
            return;
        }
        let key = batch_key(config, tx);
        let seen = self.batches.entry(key).or_default();
        if seen.is_empty() {
            self.batch_order.push_back(key);
        }
        seen.push((policy.clone(), tx.auth.clone()));
        while self.batch_order.len() > Self::BATCH_LIMIT {
            if let Some(old) = self.batch_order.pop_front() {
                self.batches.remove(&old);
            }
        }
    }

    /// The check of spec §4.3 for `tx` under `policy`: its place in its
    /// batch always, its signatures unless they verified before.
    /// Remembers what verifies.
    pub fn check(
        &mut self,
        config: &ChainConfig,
        tx: &SignedTransaction,
        policy: &AuthPolicy,
    ) -> Result<(), String> {
        if self.verified(config, tx, policy) {
            return check_membership(tx).map_err(|e| format!("authorization: {e}"));
        }
        check_authorization(config, tx, policy).map_err(|e| format!("authorization: {e}"))?;
        self.remember(config, tx, policy);
        Ok(())
    }

    /// Whether every transaction of a Pulse is already known to be
    /// authorized under the policy it will execute under, verifying the
    /// ones not seen before in parallel. Then the Pulse may execute without
    /// re-verifying signatures, with exactly the same result.
    ///
    /// Returns false (verify during execution) if anything in the Pulse
    /// rotates a policy, if a policy cannot be determined before execution,
    /// or if any authorization is invalid: execution then rejects it.
    pub fn preverify<'t>(
        &mut self,
        state: &State,
        txs: impl IntoIterator<Item = &'t SignedTransaction>,
    ) -> bool {
        let txs: Vec<&SignedTransaction> = txs.into_iter().collect();
        if txs
            .iter()
            .any(|t| matches!(t.tx.kind, sion_exec::TxKind::RotatePolicy { .. }))
        {
            return false;
        }
        let config = &state.config;
        // Each batch member's place in its batch is checked here, every
        // time (a few hashes); the signatures of each distinct transaction
        // or batch at most once.
        let mut misses: Vec<(AuthPolicy, &SignedTransaction)> = Vec::new();
        // A batch's members after the first, compared byte for byte.
        let mut pending: BTreeMap<Hash32, Vec<usize>> = BTreeMap::new();
        for tx in txs {
            if check_membership(tx).is_err() {
                return false;
            }
            let Ok(policy) = policy_for(state, tx) else {
                return false;
            };
            if self.verified(config, tx, &policy) {
                continue;
            }
            if tx.batch.is_none() {
                misses.push((policy, tx));
                continue;
            }
            let same = pending.entry(batch_key(config, tx)).or_default();
            if same
                .iter()
                .any(|&i| misses[i].0 == policy && misses[i].1.auth == tx.auth)
            {
                continue;
            }
            same.push(misses.len());
            misses.push((policy, tx));
        }
        if misses.is_empty() {
            return true;
        }
        let threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
            .clamp(1, misses.len());
        let size = misses.len().div_ceil(threads);
        let valid = std::thread::scope(|scope| {
            let handles: Vec<_> = misses
                .chunks(size)
                .map(|chunk| {
                    scope.spawn(move || {
                        chunk
                            .iter()
                            .all(|(policy, tx)| check_authorization(config, tx, policy).is_ok())
                    })
                })
                .collect();
            handles
                .into_iter()
                .all(|handle| handle.join().unwrap_or(false))
        });
        if valid {
            for (policy, tx) in misses {
                self.remember(config, tx, &policy);
            }
        }
        valid
    }
}

/// For a request the validator has just checked with [`StateOwners::check`]
/// (so the signature is not verified twice).
#[derive(Debug, Clone, Copy)]
pub struct AlreadyChecked;

impl OwnerAuthorization for AlreadyChecked {
    fn authorizes(&self, _: &LaneRequest) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use sion_core::ids::AssetId;
    use sion_core::Amount;
    use sion_exec::sim::{Chain, Sender};
    use sion_exec::TxKind;

    use super::*;

    fn transfer(chain: &mut Chain, from: &str, to: &str) -> SignedTransaction {
        let to = chain.id(to);
        chain
            .tx(
                from,
                TxKind::Transfer {
                    asset: AssetId::usd(),
                    outputs: vec![(to, Amount::units(1))],
                },
            )
            .unwrap()
    }

    /// Found by the security review: lane validation did not run the
    /// checks execution makes before charging a fee, so a zero-fee
    /// transaction was certified, rejected at execution for free, and its
    /// body published anyway.
    #[test]
    fn lane_checks_refuse_what_would_not_pay() {
        let mut chain = Chain::new(&["a", "b"], 100, 2);
        let owners_of =
            |state: &State, tx: &SignedTransaction| StateOwners { state }.check_unsigned(tx);
        let good = transfer(&mut chain, "a", "b");
        assert!(owners_of(&chain.state, &good).is_ok());
        let mut free = good.clone();
        free.tx.fee = sion_core::Amount::ZERO;
        assert!(owners_of(&chain.state, &free)
            .unwrap_err()
            .contains("below minimum"));
        let mut duplicated = good.clone();
        duplicated.tx.inputs.push(duplicated.tx.inputs[0]);
        assert!(owners_of(&chain.state, &duplicated)
            .unwrap_err()
            .contains("duplicate"));
        let mut too_many = good.clone();
        too_many.tx.inputs = vec![good.tx.inputs[0]; MAX_INPUTS + 1];
        assert!(owners_of(&chain.state, &too_many).is_err());
        let mut rich = good;
        rich.tx.fee = sion_core::Amount::units(1_000_000);
        assert!(
            owners_of(&chain.state, &rich).is_err(),
            "the gas coin must cover the fee"
        );
    }

    #[test]
    fn preverification_matches_what_execution_would_check() {
        let mut chain = Chain::new(&["a", "b"], 100, 4);
        let txs = vec![
            transfer(&mut chain, "a", "b"),
            transfer(&mut chain, "b", "a"),
        ];
        let mut authorized = Authorized::default();
        assert!(authorized.preverify(&chain.state, &txs));
        // Now cached: a second pass verifies nothing.
        let keys = authorized.order.len();
        assert!(authorized.preverify(&chain.state, &txs));
        assert_eq!(authorized.order.len(), keys);

        // A bad signature means execution must check (and reject) it.
        let mut forged = transfer(&mut chain, "a", "b");
        forged.tx.nonce += 1;
        assert!(!Authorized::default().preverify(&chain.state, &[forged]));

        // Found by the security review: the same body with its signatures
        // stripped must not ride on the cached check of the original.
        let mut stripped = txs[0].clone();
        stripped.auth = Default::default();
        assert!(!authorized.preverify(&chain.state, &[stripped]));

        // A policy rotation in the Pulse changes what later transactions
        // are checked against, so nothing is skipped.
        let policy = chain.wallet("a").policy.clone();
        let rotate = chain
            .tx(
                "a",
                TxKind::RotatePolicy {
                    policy,
                    viewing_key: None,
                },
            )
            .unwrap();
        assert!(!Authorized::default().preverify(&chain.state, &[rotate]));
    }

    #[test]
    fn a_batch_is_preverified_with_one_signature_check() {
        let mut chain = Chain::new(&["a", "b"], 100, 8);
        let b = chain.id("b");
        let members = chain
            .batch(
                "a",
                (1..=4)
                    .map(|n| {
                        (
                            Sender::Named("a"),
                            TxKind::Transfer {
                                asset: AssetId::usd(),
                                outputs: vec![(b, Amount::units(n))],
                            },
                            Vec::new(),
                        )
                    })
                    .collect(),
            )
            .unwrap();
        let mut authorized = Authorized::default();
        assert!(authorized.preverify(&chain.state, &members));
        assert_eq!(authorized.batch_order.len(), 1, "one batch remembered");
        assert!(authorized.keys.is_empty());
        let policy = chain.wallet("a").policy.clone();
        for member in &members {
            assert!(authorized.verified(&chain.state.config, member, &policy));
        }
        // A remembered batch does not excuse a member's place in it.
        let mut moved = members[0].clone();
        moved.batch.as_mut().unwrap().index = 1;
        assert!(!authorized.preverify(&chain.state, &[moved]));
        // Nor other signatures over the same batch.
        let mut stripped = members[1].clone();
        stripped.auth = Default::default();
        assert!(!authorized.verified(&chain.state.config, &stripped, &policy));
        assert!(!authorized.preverify(&chain.state, &[stripped]));
        // The fee floor of a member is the batch's: lower than alone.
        let owners = StateOwners {
            state: &chain.state,
        };
        assert!(owners.check_unsigned(&members[2]).is_ok());
        let mut alone = members[2].clone();
        alone.batch = None;
        assert!(owners.check_unsigned(&alone).is_err());
    }
}
