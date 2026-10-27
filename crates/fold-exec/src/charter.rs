//! Agent Charters: protocol-enforced limits for AI agents.
//!
//! A charter binds an agent to a principal with caveats the protocol checks
//! on every payment, so a hijacked or prompt-injected agent cannot exceed
//! them: they are not in its code. The agent spends from an allotment only
//! it can write, so its payments stay consensus-free; the charter itself is
//! a shared object, so revocation is ordered by consensus and takes effect
//! when it commits (payments certified before then still execute).
//! Delegation only narrows: a sub-charter's caveats must attenuate its
//! parent's.

use std::collections::BTreeSet;

use borsh::{BorshDeserialize, BorshSerialize};
use fold_assurance::Policy;
use fold_core::ids::{AccountId, AssetId, AtomId, ObjectId, Pulse};
use fold_core::Amount;
use serde::{Deserialize, Serialize};

use crate::error::ExecError;

/// The most charters a delegation chain may hold, root included
/// (spec §5.13).
pub const MAX_CHARTER_CHAIN: usize = 16;

/// The limits an agent operates within.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct Caveats {
    /// Allowed payees; `None` allows any.
    pub payees: Option<BTreeSet<AccountId>>,
    /// Largest single payment.
    pub max_per_payment: Amount,
    /// Largest total within one window.
    pub max_per_window: Amount,
    /// Window length in Pulses.
    pub window: u64,
    /// Last Pulse in which the agent may act.
    pub expires_at: Pulse,
    /// Payments above this amount must cite an evidence claim whose cost of
    /// corruption covers κ × amount under `evidence_policy`.
    pub evidence_above: Option<Amount>,
    /// The principal's policy for judging evidence.
    pub evidence_policy: Policy,
    /// Whether every payment emits a receipt claim.
    pub receipts: bool,
    /// Whether the agent may delegate to sub-agents.
    pub may_delegate: bool,
}

impl Caveats {
    /// Whether `self` is at least as strict as `parent` (delegation only narrows).
    pub fn attenuates(&self, parent: &Caveats) -> bool {
        let payees_ok = match (&self.payees, &parent.payees) {
            (_, None) => true,
            (None, Some(_)) => false,
            (Some(child), Some(allowed)) => child.is_subset(allowed),
        };
        let evidence_ok = match (self.evidence_above, parent.evidence_above) {
            (_, None) => true,
            (None, Some(_)) => false,
            (Some(child), Some(limit)) => child <= limit,
        };
        payees_ok
            && evidence_ok
            && self.max_per_payment <= parent.max_per_payment
            && self.max_per_window <= parent.max_per_window
            && self.window >= parent.window
            && self.expires_at <= parent.expires_at
            && (parent.receipts <= self.receipts)
            && (self.may_delegate <= parent.may_delegate)
    }
}

/// An Agent Charter (a shared object in its own contention domain).
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct Charter {
    /// Who answers for the agent.
    pub principal: AccountId,
    /// The agent.
    pub agent: AccountId,
    /// The caveats.
    pub caveats: Caveats,
    /// The economic atom backing the agent's receipts.
    pub agent_atom: AtomId,
    /// Whose bond the agent atom is: the root principal, also for
    /// delegated charters.
    pub backer: AccountId,
    /// When the charter was revoked, if it was.
    pub revoked_at: Option<Pulse>,
    /// The parent charter, for delegated charters.
    pub parent: Option<ObjectId>,
    /// Allotments funded under this charter.
    pub allotments: BTreeSet<ObjectId>,
    /// Creation Pulse.
    pub created_at: Pulse,
}

/// A single-writer allotment: part of a charter's budget or of a bounded
/// total, spendable by its writer alone.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct Allotment {
    /// The charter or bounded total it comes from.
    pub source: ObjectId,
    /// The only account that may spend it.
    pub writer: AccountId,
    /// Asset.
    pub asset: AssetId,
    /// Remaining amount.
    pub remaining: Amount,
    /// The charter's caveats, copied at funding time (immutable).
    pub caveats: Option<Caveats>,
    /// Start of the current spending window.
    pub window_start: Pulse,
    /// Spent in the current window.
    pub spent_in_window: Amount,
}

impl Allotment {
    /// Checks a payment against the caveats and the balance, and records it.
    /// `evidence_ok` reports whether a required evidence claim passed.
    pub fn spend(
        &mut self,
        payee: &AccountId,
        amount: Amount,
        pulse: Pulse,
        evidence_ok: Option<bool>,
    ) -> Result<(), ExecError> {
        if amount > self.remaining {
            return Err(ExecError::InsufficientFunds {
                need: amount.to_string(),
                have: self.remaining.to_string(),
            });
        }
        if let Some(caveats) = &self.caveats {
            if pulse > caveats.expires_at {
                return Err(ExecError::Caveat("charter expired".into()));
            }
            if let Some(payees) = &caveats.payees {
                if !payees.contains(payee) {
                    return Err(ExecError::Caveat("payee not allowed".into()));
                }
            }
            if amount > caveats.max_per_payment {
                return Err(ExecError::Caveat(format!(
                    "payment {} above per-payment limit {}",
                    amount, caveats.max_per_payment
                )));
            }
            if pulse.0 >= self.window_start.0.saturating_add(caveats.window) {
                self.window_start = pulse;
                self.spent_in_window = Amount::ZERO;
            }
            let window_total = self.spent_in_window.checked_add(amount)?;
            if window_total > caveats.max_per_window {
                return Err(ExecError::Caveat(format!(
                    "window total {} above limit {}",
                    window_total, caveats.max_per_window
                )));
            }
            if let Some(threshold) = caveats.evidence_above {
                if amount > threshold && evidence_ok != Some(true) {
                    return Err(ExecError::Caveat(format!(
                        "payments above {threshold} need evidence whose cost of corruption covers κ × amount"
                    )));
                }
            }
            self.spent_in_window = window_total;
        }
        self.remaining = self.remaining.checked_sub(amount)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caveats() -> Caveats {
        Caveats {
            payees: Some([AccountId::derive(b"supplier")].into_iter().collect()),
            max_per_payment: Amount::units(2_000),
            max_per_window: Amount::units(2_000),
            window: 10,
            expires_at: Pulse(100),
            evidence_above: Some(Amount::units(500)),
            evidence_policy: Policy::standard(),
            receipts: true,
            may_delegate: true,
        }
    }

    fn allotment() -> Allotment {
        Allotment {
            source: ObjectId::derive(b"charter"),
            writer: AccountId::derive(b"agent"),
            asset: AssetId::usd(),
            remaining: Amount::units(10_000),
            caveats: Some(caveats()),
            window_start: Pulse(0),
            spent_in_window: Amount::ZERO,
        }
    }

    #[test]
    fn caveats_bound_the_worst_case() {
        let supplier = AccountId::derive(b"supplier");
        let mut a = allotment();
        a.spend(&supplier, Amount::units(400), Pulse(1), None)
            .unwrap();
        assert!(a
            .spend(
                &AccountId::derive(b"attacker"),
                Amount::units(1),
                Pulse(1),
                None
            )
            .is_err());
        assert!(a
            .spend(&supplier, Amount::units(2_001), Pulse(1), Some(true))
            .is_err());
        assert!(
            a.spend(&supplier, Amount::units(600), Pulse(1), None)
                .is_err(),
            "needs evidence"
        );
        a.spend(&supplier, Amount::units(600), Pulse(1), Some(true))
            .unwrap();
        assert!(
            a.spend(&supplier, Amount::units(1_001), Pulse(2), Some(true))
                .is_err(),
            "window full"
        );
        a.spend(&supplier, Amount::units(1_000), Pulse(11), Some(true))
            .unwrap();
        assert!(
            a.spend(&supplier, Amount::units(1), Pulse(101), None)
                .is_err(),
            "expired"
        );
        assert_eq!(a.remaining, Amount::units(8_000));
    }

    #[test]
    fn delegation_only_narrows() {
        let parent = caveats();
        let mut child = parent.clone();
        child.max_per_payment = Amount::units(100);
        assert!(child.attenuates(&parent));
        child.payees = None;
        assert!(!child.attenuates(&parent));
        let mut wider = parent.clone();
        wider.expires_at = Pulse(200);
        assert!(!wider.attenuates(&parent));
        let mut no_evidence = parent.clone();
        no_evidence.evidence_above = None;
        assert!(!no_evidence.attenuates(&parent));
    }
}
