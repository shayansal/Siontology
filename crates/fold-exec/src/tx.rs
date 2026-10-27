//! Transactions and their coordination classes.
//!
//! The coordination class of a transaction is derived from what it does,
//! never declared: knowledge-only effects need no ordering, owned inputs need
//! a lane certificate, and operations on shared objects need consensus in
//! that object's contention domain. Every transaction has at least one owned
//! input (its gas coin), which also gives replay protection: each object
//! version can be consumed by one transaction only.

use borsh::{BorshDeserialize, BorshSerialize};
use fold_assurance::{AtomClass, Policy};
use fold_clearing::ClearingResult;
use fold_core::encode::encode;
use fold_core::hash::Hash32;
use fold_core::ids::{AccountId, AssetId, AtomId, ClaimId, DomainId, ObjectId, Pulse, TxDigest};
use fold_core::Amount;
use fold_crypto::{AuthPolicy, Authorization, CryptoError, DualKeypair, KemPublicKey, KeyShare};
use serde::{Deserialize, Serialize};

use crate::charter::Caveats;
use crate::claim::ClaimBody;
use crate::market::Submission;
use crate::object::{well_known, CoordinationClass, ObjectRef};

/// The signing domain for transactions.
pub const TX_DOMAIN: &str = "fold/tx/v1";

/// What a transaction does.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub enum TxKind {
    /// Pays outputs from the sender's coins of `asset`; change returns to the sender.
    Transfer {
        /// Asset transferred.
        asset: AssetId,
        /// Recipients and amounts.
        outputs: Vec<(AccountId, Amount)>,
    },
    /// Devnet faucet: mints each grant. Only the configured faucet account
    /// may send it; its input is the faucet's account object.
    Faucet {
        /// Recipients, assets and amounts (at most 256).
        grants: Vec<(AccountId, AssetId, Amount)>,
    },
    /// Rotates the sender's authorization policy (the account ID is unchanged).
    RotatePolicy {
        /// New policy.
        policy: AuthPolicy,
        /// New viewing key.
        viewing_key: Option<KemPublicKey>,
    },
    /// Issues a claim.
    IssueClaim {
        /// The claim.
        claim: ClaimBody,
    },
    /// Registers an unbonded atom (for example a circuit). Attack costs are
    /// governance-set at genesis and cannot be registered this way.
    RegisterAtom {
        /// Class.
        class: AtomClass,
        /// Label.
        label: String,
    },
    /// Posts (or tops up) a bond behind the sender's bonded atom
    /// `bond:<sender>:<label>`.
    PostBond {
        /// Atom class (economic, institutional or adjudicator).
        class: AtomClass,
        /// Label within the sender's namespace.
        label: String,
        /// Amount in USD.
        amount: Amount,
    },
    /// Accepts a claim under a policy and reserves κ·V of its backing.
    Accept {
        /// The claim.
        claim: ClaimId,
        /// Value at risk.
        value_at_risk: Amount,
        /// The consumer's policy.
        policy: Policy,
        /// End of reliance.
        until: Pulse,
        /// Distinguishes repeated acceptances.
        nonce: u64,
    },
    /// Releases a reservation.
    Release {
        /// The reservation.
        reservation: Hash32,
    },
    /// Challenges a bonded atom of a claim.
    Challenge {
        /// The claim.
        claim: ClaimId,
        /// The challenged atom.
        target: AtomId,
        /// Digest of the counter-evidence.
        evidence: Hash32,
        /// Deposit in USD.
        deposit: Amount,
    },
    /// The claim's adjudicator rules on a challenge.
    Rule {
        /// The challenge.
        challenge: Hash32,
        /// Whether the atom is broken.
        upheld: bool,
    },
    /// Withdraws the free part of a bond after its lock.
    WithdrawBond {
        /// The bonded atom.
        atom: AtomId,
    },
    /// Creates an Agent Charter and funds the agent's allotment.
    CreateCharter {
        /// The agent.
        agent: AccountId,
        /// The caveats.
        caveats: Caveats,
        /// Budget in USD.
        funding: Amount,
        /// Bond behind the agent's receipts, in USD.
        agent_bond: Amount,
    },
    /// The agent delegates part of its allotment under narrower caveats.
    Delegate {
        /// The parent charter.
        charter: ObjectId,
        /// The sub-agent.
        sub_agent: AccountId,
        /// Narrower caveats.
        caveats: Caveats,
        /// Amount moved from the agent's allotment.
        amount: Amount,
    },
    /// The agent pays from its allotment.
    AgentPay {
        /// The charter.
        charter: ObjectId,
        /// Payee.
        payee: AccountId,
        /// Amount.
        amount: Amount,
        /// Purpose (recorded in the receipt).
        purpose: String,
        /// Evidence claim for large payments.
        evidence: Option<ClaimId>,
    },
    /// The principal revokes a charter.
    RevokeCharter {
        /// The charter.
        charter: ObjectId,
    },
    /// Creates a bounded total split into allotments.
    CreateBounded {
        /// Label.
        label: String,
        /// Asset escrowed.
        asset: AssetId,
        /// Total.
        total: Amount,
        /// Initial allotments.
        allotments: Vec<(AccountId, Amount)>,
    },
    /// The writer spends from its allotment.
    SpendAllotment {
        /// Recipient.
        to: AccountId,
        /// Amount.
        amount: Amount,
    },
    /// The writer returns the rest of its allotment to the bounded total.
    ReturnAllotment,
    /// The manager issues a new allotment from the unallocated amount.
    Rebalance {
        /// The bounded total.
        bounded: ObjectId,
        /// The new allotment's single writer.
        holder: AccountId,
        /// Amount.
        amount: Amount,
    },
    /// Creates a market.
    CreateMarket {
        /// Name.
        label: String,
        /// The good.
        good: AssetId,
        /// The numeraire.
        numeraire: AssetId,
        /// Pulses per batch.
        batch_length: u64,
        /// Pulses allowed for clearing after sealing.
        clear_deadline: u64,
        /// Committee for sealed orders.
        committee: Vec<KemPublicKey>,
        /// Reveal threshold.
        threshold: u8,
    },
    /// Submits an order (escrow is drawn from the sender's coins).
    SubmitOrder {
        /// The market.
        market: ObjectId,
        /// The order.
        submission: Submission,
    },
    /// Reveals sealed orders of a sealed batch with committee key shares.
    RevealBatch {
        /// The market.
        market: ObjectId,
        /// Batch index.
        batch: u64,
        /// Shares per sealed order.
        shares: Vec<(Hash32, Vec<KeyShare>)>,
    },
    /// A solver submits a certified clearing for a sealed batch.
    ClearBatch {
        /// The market.
        market: ObjectId,
        /// Batch index.
        batch: u64,
        /// The clearing with its optimality certificate.
        result: ClearingResult,
    },
}

/// A transaction body (what is signed).
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct Transaction {
    /// The sender.
    pub sender: AccountId,
    /// The sender's genesis policy, required on its first transaction.
    pub genesis_policy: Option<AuthPolicy>,
    /// Owned inputs; `inputs[0]` is the gas coin (or the faucet's account object).
    pub inputs: Vec<ObjectRef>,
    /// Fee in USD.
    pub fee: Amount,
    /// What it does.
    pub kind: TxKind,
    /// Last Pulse in which it may execute.
    pub expires_at: Pulse,
    /// Makes otherwise identical transactions distinct.
    pub nonce: u64,
}

/// A transaction with its authorization.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct SignedTransaction {
    /// The body.
    pub tx: Transaction,
    /// Signatures satisfying the sender's policy.
    pub auth: Authorization,
}

impl Transaction {
    /// The digest (also the signed message).
    pub fn digest(&self) -> TxDigest {
        TxDigest::derive(&encode(self))
    }

    /// Signs with a dual keypair under its default policy.
    pub fn sign(self, keys: &DualKeypair) -> Result<SignedTransaction, CryptoError> {
        let auth = keys.authorize(TX_DOMAIN, &self.digest().0 .0)?;
        Ok(SignedTransaction { tx: self, auth })
    }

    /// The contention domain a transaction must be ordered in, if any.
    pub fn domain(&self) -> Option<DomainId> {
        match &self.kind {
            TxKind::Accept { .. }
            | TxKind::Release { .. }
            | TxKind::Challenge { .. }
            | TxKind::Rule { .. }
            | TxKind::WithdrawBond { .. } => Some(well_known::assurance_domain()),
            TxKind::RevokeCharter { charter } => Some(object_domain("charter", charter)),
            TxKind::Rebalance { bounded, .. } => Some(object_domain("bounded", bounded)),
            TxKind::RevealBatch { market, .. } | TxKind::ClearBatch { market, .. } => {
                Some(object_domain("market", market))
            }
            _ => None,
        }
    }

    /// The coordination class, derived from the operation.
    pub fn class(&self) -> CoordinationClass {
        if self.domain().is_some() {
            return CoordinationClass::Contended;
        }
        match self.kind {
            TxKind::IssueClaim { .. } | TxKind::RegisterAtom { .. } | TxKind::PostBond { .. } => {
                CoordinationClass::Knowledge
            }
            TxKind::AgentPay { .. }
            | TxKind::SpendAllotment { .. }
            | TxKind::ReturnAllotment
            | TxKind::Delegate { .. } => CoordinationClass::Bounded,
            _ => CoordinationClass::OwnedLinear,
        }
    }

    /// Encoded size, for fees and bandwidth accounting.
    pub fn size(&self) -> usize {
        encode(self).len()
    }
}

impl SignedTransaction {
    /// The digest.
    pub fn digest(&self) -> TxDigest {
        self.tx.digest()
    }

    /// Total encoded size including signatures.
    pub fn size(&self) -> usize {
        encode(self).len()
    }
}

/// The domain of a shared object created by kind `kind`.
pub fn object_domain(kind: &str, object: &ObjectId) -> DomainId {
    DomainId::derive_parts(&[kind.as_bytes(), &object.0 .0])
}

#[cfg(test)]
mod tests {
    use super::*;
    use fold_core::ids::Version;

    fn tx(kind: TxKind) -> Transaction {
        Transaction {
            sender: AccountId::derive(b"s"),
            genesis_policy: None,
            inputs: vec![ObjectRef {
                id: ObjectId::derive(b"gas"),
                version: Version(0),
            }],
            fee: Amount::units(1),
            kind,
            expires_at: Pulse(10),
            nonce: 0,
        }
    }

    #[test]
    fn classes_are_derived() {
        let transfer = tx(TxKind::Transfer {
            asset: AssetId::usd(),
            outputs: vec![],
        });
        assert_eq!(transfer.class(), CoordinationClass::OwnedLinear);
        let atom = tx(TxKind::RegisterAtom {
            class: AtomClass::Code,
            label: "circuit".into(),
        });
        assert_eq!(atom.class(), CoordinationClass::Knowledge);
        let accept = tx(TxKind::Release {
            reservation: Hash32::ZERO,
        });
        assert_eq!(accept.class(), CoordinationClass::Contended);
        assert_eq!(accept.domain(), Some(well_known::assurance_domain()));
        let pay = tx(TxKind::SpendAllotment {
            to: AccountId::derive(b"x"),
            amount: Amount::units(1),
        });
        assert_eq!(pay.class(), CoordinationClass::Bounded);
    }

    #[test]
    fn digests_change_with_any_field() {
        let a = tx(TxKind::ReturnAllotment);
        let mut b = a.clone();
        b.nonce = 1;
        assert_ne!(a.digest(), b.digest());
    }
}
