//! What a validator checks before locking owned inputs to a transaction.

use fold_consensus::lanes::canonical_inputs;
use fold_consensus::{LaneRequest, OwnerAuthorization};
use fold_core::encode::{decode, encode};
use fold_core::hash::{hash_parts, Hash32};
use fold_core::ids::TxDigest;
use fold_crypto::AuthPolicy;
use fold_exec::object::well_known;
use fold_exec::tx::TX_DOMAIN;
use fold_exec::{ObjectData, Owner, SignedTransaction, State};

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

/// Identifies "this transaction's authorization satisfies this policy", so a
/// validator that checked it once (when locking the inputs) need not check
/// it again when executing.
pub fn authorization_key(tx: &TxDigest, policy: &AuthPolicy) -> Hash32 {
    hash_parts("fold/node/authorized", &[&tx.0 .0, &encode(policy)])
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
        if signed.tx.expires_at <= self.state.pulse {
            return Err("the transaction has expired".into());
        }
        let sender = signed.tx.sender;
        let policy = policy_for(self.state, &signed)?;
        policy
            .verify(TX_DOMAIN, &signed.digest().0 .0, &signed.auth)
            .map_err(|e| format!("authorization: {e}"))?;
        let key = authorization_key(&signed.digest(), &policy);
        for input in &signed.tx.inputs {
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
        Ok((signed, key))
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
            .any(|t| matches!(t.tx.kind, fold_exec::TxKind::RotatePolicy { .. }))
        {
            return false;
        }
        let mut misses = Vec::new();
        for tx in txs {
            let Ok(policy) = policy_for(state, tx) else {
                return false;
            };
            let key = authorization_key(&tx.digest(), &policy);
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
        let valid = std::thread::scope(|scope| {
            let handles: Vec<_> = misses
                .chunks(size)
                .map(|chunk| {
                    scope.spawn(move || {
                        chunk.iter().all(|(_, policy, tx)| {
                            policy
                                .verify(TX_DOMAIN, &tx.digest().0 .0, &tx.auth)
                                .is_ok()
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
    use fold_core::ids::AssetId;
    use fold_core::Amount;
    use fold_exec::sim::Chain;
    use fold_exec::TxKind;

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
