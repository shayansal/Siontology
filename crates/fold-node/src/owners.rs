//! What a validator checks before locking owned inputs to a transaction.

use fold_consensus::lanes::canonical_inputs;
use fold_consensus::{LaneRequest, OwnerAuthorization};
use fold_core::encode::decode;
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

impl StateOwners<'_> {
    /// Why a request is not authorized, if it is not.
    pub fn check(&self, request: &LaneRequest) -> Result<SignedTransaction, String> {
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
        let policy = match self
            .state
            .objects
            .get(&well_known::account(&sender))
            .map(|o| &o.data)
        {
            Some(ObjectData::Account(account)) => account.policy.clone(),
            Some(_) => return Err("account object corrupted".into()),
            None => {
                let policy = signed
                    .tx
                    .genesis_policy
                    .clone()
                    .ok_or_else(|| "unknown account and no genesis policy".to_string())?;
                if policy.account_id() != sender {
                    return Err("genesis policy does not hash to the sender".into());
                }
                policy
            }
        };
        policy
            .verify(TX_DOMAIN, &signed.digest().0 .0, &signed.auth)
            .map_err(|e| format!("authorization: {e}"))?;
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
        Ok(signed)
    }
}

impl OwnerAuthorization for StateOwners<'_> {
    fn authorizes(&self, request: &LaneRequest) -> bool {
        self.check(request).is_ok()
    }
}
