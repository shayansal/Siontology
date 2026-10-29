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
//!
//! **Agent identities** (spec v1.8). A charter with an [`AgentIdentity`]
//! is an agent's identity: its id is the agent's id, its account signs for
//! the agent, and its card commitment says what the agent is. An agent
//! launches subagents with [`Spawn`]s, each getting its own identity,
//! account, narrower caveats and a budget carved from the parent's
//! allotment, so siblings never contend. Revoking or suspending any
//! charter stops everything below it, because every check walks the chain
//! up to the root.
//!
//! **Keyless subagents** (spec v1.13). A subagent spawned without an
//! account of its own gets [`keyless_account`] of its id: no key controls
//! it, and its transactions are authorized only as members of batches its
//! nearest keyed ancestor signs ([`crate::tx::sign_batch`]).

use std::collections::BTreeSet;

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use sion_assurance::Policy;
use sion_core::hash::Hash32;
use sion_core::ids::{AccountId, AssetId, AtomId, ObjectId, Pulse};
use sion_core::Amount;

use crate::error::ExecError;

/// The most charters a delegation chain may hold, root included
/// (spec §5.13).
pub const MAX_CHARTER_CHAIN: usize = 16;

/// The most subagents one `SpawnAgents` may launch (spec §5.36).
pub const MAX_SPAWN_BATCH: usize = 256;

/// An agent's identity record on its charter (spec v1.8).
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct AgentIdentity {
    /// Commitment to the agent's card ([`card_commitment`] of its bytes);
    /// all zero for none.
    pub card: Hash32,
    /// Generation in its tree: 0 for a registered agent, and for a spawned
    /// one the number of charters above it.
    pub depth: u8,
}

/// One subagent to launch (spec §5.36).
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct Spawn {
    /// The account that acts for the subagent, or `None` for a keyless
    /// subagent, whose account is [`keyless_account`] of its id.
    pub agent: Option<AccountId>,
    /// Commitment to its card.
    pub card: Hash32,
    /// Its caveats, which must attenuate the parent's.
    pub caveats: Caveats,
    /// Carved from the parent's allotment into the subagent's own.
    pub budget: Amount,
}

/// The account of the keyless subagent whose charter is `charter`
/// (spec §4.3): `AccountId(H("sion/agent/keyless", charter bytes))`. Account
/// ids of keyed accounts hash policies in another domain, so no key ever
/// controls this one.
pub fn keyless_account(charter: &ObjectId) -> AccountId {
    AccountId(sion_core::hash::hash_domain(
        "sion/agent/keyless",
        &charter.0 .0,
    ))
}

/// Whether `charter`'s agent is keyless: its account is
/// [`keyless_account`] of the charter's own id.
pub fn is_keyless(id: &ObjectId, charter: &Charter) -> bool {
    charter.agent == keyless_account(id)
}

/// The commitment an identity stores for an agent card: a domain-separated
/// SHA3-256 of the card's exact bytes, as published.
pub fn card_commitment(card: &[u8]) -> Hash32 {
    sion_core::hash::hash_domain("sion/agent/card", card)
}

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
    /// Who suspended it, while it is suspended (spec v1.8).
    pub suspended_by: Option<AccountId>,
    /// Its identity record, if it is an agent identity (spec v1.8).
    pub identity: Option<AgentIdentity>,
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
    /// Window allowance carved out for subagents (spec v1.8): the writer's
    /// own window limit is `max_per_window − carved_window`, so a subtree's
    /// limits never add up to more than its root's.
    pub carved_window: Amount,
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
            let own_limit = caveats.max_per_window.checked_sub(self.carved_window)?;
            if window_total > own_limit {
                return Err(ExecError::Caveat(format!(
                    "window total {} above limit {}",
                    window_total, own_limit
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
            carved_window: Amount::ZERO,
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
