//! What a validator checks before locking owned inputs to a transaction.

use sion_consensus::lanes::canonical_inputs;
use sion_consensus::{LaneRequest, OwnerAuthorization};
use sion_core::encode::{decode, encode};
use sion_core::hash::{hash_parts, Hash32};
use sion_crypto::AuthPolicy;
use sion_exec::object::well_known;
use sion_exec::tx::signing_context;
use sion_exec::{ObjectData, Owner, SignedTransaction, State};

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
/// transaction", so a validator that checked them once (when locking the
/// inputs) need not check them again when executing. The key covers the
/// signatures themselves: the transaction digest does not, and the same
/// body with other signatures must be checked afresh.
pub fn authorization_key(tx: &SignedTransaction, policy: &AuthPolicy) -> Hash32 {
    hash_parts(
        "sion/node/authorized",
        &[&tx.digest().0 .0, &encode(policy), &encode(&tx.auth)],
    )
}

/// The policy a transaction is checked against in the given state: the
/// sender's current policy, or its genesis policy if the account is new.
pub fn policy_for(state: &State, signed: &SignedTransaction) -> Result<AuthPolicy, String> {
    let sender = signed.tx.sender;
    match state
        .objects
        .get(&well_known::account(&sender))
        .map(|o| &o.data)
    {
        Some(ObjectData::Account(account)) => Ok(account.policy.clone()),
        Some(_) => Err("account object corrupted".into()),
        None => {
            let policy = signed
                .tx
                .genesis_policy
                .clone()
                .ok_or_else(|| "unknown account and no genesis policy".to_string())?;
            if policy.account_id() != sender {
                return Err("genesis policy does not hash to the sender".into());
            }
            Ok(policy)
        }
    }
}

impl StateOwners<'_> {
    /// Why a request is not authorized, if it is not. On success, returns
    /// the transaction and its [`authorization_key`].
    pub fn check(&self, request: &LaneRequest) -> Result<(SignedTransaction, Hash32), String> {
        let signed: SignedTransaction = decode(&request.owner_proof)
            .map_err(|_| "owner proof is not a signed transaction".to_string())?;
        if signed.digest() != request.tx {
            return Err("owner proof is for another transaction".into());
        }
        if canonical_inputs(&lane_inputs(&signed.tx.inputs)) != request.canonical_inputs() {
            return Err("request inputs differ from the transaction's".into());
        }
        // Cheap checks first; the signature last.
        self.check_unsigned(&signed)?;
        let policy = policy_for(self.state, &signed)?;
        policy
            .verify(
                &signing_context(&self.state.config),
                &signed.digest().0 .0,
                &signed.auth,
            )
            .map_err(|e| format!("authorization: {e}"))?;
        let key = authorization_key(&signed, &policy);
        Ok((signed, key))
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
        let distinct: std::collections::BTreeSet<_> = tx.inputs.iter().map(|r| r.id).collect();
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
            let floor = config.min_fee(tx.size());
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
        let key = authorization_key(signed, &policy);
        if !authorized.contains(&key) {
            policy
                .verify(
                    &signing_context(&self.state.config),
                    &signed.digest().0 .0,
                    &signed.auth,
                )
                .map_err(|e| format!("authorization: {e}"))?;
            authorized.insert(key);
        }
        Ok(())
    }
}

impl OwnerAuthorization for StateOwners<'_> {
    fn authorizes(&self, request: &LaneRequest) -> bool {
        self.check(request).is_ok()
    }
}

/// Authorizations a validator has verified, by [`authorization_key`]; the
/// oldest are forgotten first.
#[derive(Debug, Default)]
pub struct Authorized {
    keys: std::collections::BTreeSet<Hash32>,
    order: std::collections::VecDeque<Hash32>,
}

impl Authorized {
    /// How many authorizations are remembered.
    pub const LIMIT: usize = 200_000;

    /// Whether this authorization was verified.
    pub fn contains(&self, key: &Hash32) -> bool {
        self.keys.contains(key)
    }

    /// Remembers a verified authorization.
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
        let mut misses = Vec::new();
        for tx in txs {
            let Ok(policy) = policy_for(state, tx) else {
                return false;
            };
            let key = authorization_key(tx, &policy);
            if !self.contains(&key) {
                misses.push((key, policy, tx));
            }
        }
        if misses.is_empty() {
            return true;
        }
        let threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
            .clamp(1, misses.len());
        let size = misses.len().div_ceil(threads);
        let context = signing_context(&state.config);
        let context = context.as_str();
        let valid = std::thread::scope(|scope| {
            let handles: Vec<_> = misses
                .chunks(size)
                .map(|chunk| {
                    scope.spawn(move || {
                        chunk.iter().all(|(_, policy, tx)| {
                            policy.verify(context, &tx.digest().0 .0, &tx.auth).is_ok()
                        })
                    })
                })
                .collect();
            handles
                .into_iter()
                .all(|handle| handle.join().unwrap_or(false))
        });
        if valid {
            for (key, _, _) in misses {
                self.insert(key);
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
    use sion_exec::sim::Chain;
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
}
