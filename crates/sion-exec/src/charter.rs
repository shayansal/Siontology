//! Agent Charters: protocol-enforced limits for AI agents.
//!
//! A charter binds an agent to a principal with caveats the protocol checks
//! on every payment, so a hijacked or prompt-injected agent cannot exceed
//! them: they are not in its code. The agent spends from an allotment only
//! it can write, so its payments stay consensus-free. Delegation only
//! narrows: a sub-charter's caveats must attenuate its parent's.
//!
//! **Agent identities** (spec v1.8). A charter with an [`AgentIdentity`]
//! is an agent's identity: its id is the agent's id, its account signs for
//! the agent, and its card commitment says what the agent is. An agent
//! launches subagents with [`Spawn`]s, each getting its own identity,
//! account, narrower caveats and a budget carved from the parent's
//! allotment, so siblings never contend.
//!
//! **Keyless subagents** (spec v1.13). A subagent spawned without an
//! account of its own gets [`keyless_account`] of its id: no key controls
//! it, and its transactions are authorized only as members of batches its
//! nearest keyed ancestor signs ([`crate::tx::sign_batch`]).
//!
//! **Lineage, buckets and overlays** (spec v1.14).
//!
//! - A spawned charter's id is [`lineage_id`] of its parent and an index
//!   from a counter only the parent's agent writes ([`Allotment::spawned`]),
//!   and every charter keeps its [`Lineage`]: its tree's root and the path
//!   of indices down to it. Anyone holding those recomputes every ancestor's
//!   id by hashing ([`Lineage::ancestry`]), without reading the ancestors.
//!   A tree is at most [`MAX_LINEAGE_DEPTH`] generations deep.
//! - Rate limits are token buckets: each charter allotment has a
//!   [`Bucket`] of capacity `burst` that refills by `rate` each Pulse, and a
//!   spawn carves its children's burst, rate and tokens out of its parent's
//!   bucket. A subtree whose root has `(σ, ρ)` therefore pays at most
//!   `σ + ρ·τ` over any `τ + 1` consecutive Pulses, however it is shaped.
//! - What happens to an agent after it is created (revoked, suspended,
//!   narrowed) is kept in its [`AgentOverlay`], at [`overlay_id`] of its
//!   id. A check reads the overlays of the agent and its ancestors (at most
//!   nine reads, most of them proofs that no overlay exists), so revoking,
//!   suspending or narrowing a subtree is one write at its root.

use std::collections::BTreeSet;

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use sion_assurance::Policy;
use sion_core::encode::encode;
use sion_core::hash::{hash_domain, Hash32};
use sion_core::ids::{AccountId, AssetId, AtomId, ObjectId, Pulse};
use sion_core::Amount;

use crate::error::ExecError;

/// The most generations below a root charter (spec §5.13): a charter's
/// lineage path holds at most this many indices.
pub const MAX_LINEAGE_DEPTH: usize = 8;

/// The most subagents one `SpawnAgents` may launch (spec §5.36).
pub const MAX_SPAWN_BATCH: usize = 256;

/// An agent's identity record on its charter (spec v1.8).
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct AgentIdentity {
    /// Commitment to the agent's card ([`card_commitment`] of its bytes);
    /// all zero for none.
    pub card: Hash32,
    /// Generation in its tree: 0 for a registered agent, and for a spawned
    /// one the number of charters above it (its lineage's depth).
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
    AccountId(hash_domain("sion/agent/keyless", &charter.0 .0))
}

/// Whether `charter`'s agent is keyless: its account is
/// [`keyless_account`] of the charter's own id.
pub fn is_keyless(id: &ObjectId, charter: &Charter) -> bool {
    charter.agent == keyless_account(id)
}

/// The commitment an identity stores for an agent card: a domain-separated
/// SHA3-256 of the card's exact bytes, as published.
pub fn card_commitment(card: &[u8]) -> Hash32 {
    hash_domain("sion/agent/card", card)
}

/// The id of the charter spawned under `parent` with `index` (spec §5.13):
/// `ObjectId(H("sion/id/agent", enc((parent, index))))`.
pub fn lineage_id(parent: &ObjectId, index: u64) -> ObjectId {
    ObjectId(hash_domain("sion/id/agent", &encode(&(*parent, index))))
}

/// The id of the overlay that records what happened to `agent` after it
/// was created (spec §5.13): `ObjectId(H("sion/overlay", agent bytes))`.
pub fn overlay_id(agent: &ObjectId) -> ObjectId {
    ObjectId(hash_domain("sion/overlay", &agent.0 .0))
}

/// A charter's place in its tree (spec §5.13).
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct Lineage {
    /// The tree's root charter.
    pub root: ObjectId,
    /// The indices from the root down to the charter: empty for the root.
    pub path: Vec<u64>,
}

impl Lineage {
    /// The lineage of a root charter.
    pub fn root(id: ObjectId) -> Self {
        Lineage {
            root: id,
            path: Vec::new(),
        }
    }

    /// The lineage of the child with `index`.
    pub fn child(&self, index: u64) -> Self {
        let mut path = self.path.clone();
        path.push(index);
        Lineage {
            root: self.root,
            path,
        }
    }

    /// The number of charters above this one.
    pub fn depth(&self) -> usize {
        self.path.len()
    }

    /// The ids from the root down to the charter itself, computed by
    /// hashing (spec §5.13 **ancestry**): `a₀ = root`, `aᵢ₊₁ =
    /// lineage_id(aᵢ, path[i])`. The last is the charter's own id.
    pub fn ancestry(&self) -> Vec<ObjectId> {
        let mut ids = Vec::with_capacity(self.path.len() + 1);
        let mut current = self.root;
        ids.push(current);
        for index in &self.path {
            current = lineage_id(&current, *index);
            ids.push(current);
        }
        ids
    }
}

/// The limits an agent operates within.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct Caveats {
    /// Allowed payees; `None` allows any.
    pub payees: Option<BTreeSet<AccountId>>,
    /// Largest single payment.
    pub max_per_payment: Amount,
    /// The most the agent may spend at once: its bucket's capacity, and
    /// what the bucket holds when the agent starts (spec v1.14).
    pub burst: Amount,
    /// What refills the agent's bucket each Pulse.
    pub rate: Amount,
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
    /// Whether `self` is at least as strict as `parent` (delegation only
    /// narrows). Where the parent requires evidence, the child requires it
    /// at no higher threshold and judges it by the same policy (spec v1.14):
    /// a laxer policy would pass evidence the parent's rejects.
    pub fn attenuates(&self, parent: &Caveats) -> bool {
        let payees_ok = match (&self.payees, &parent.payees) {
            (_, None) => true,
            (None, Some(_)) => false,
            (Some(child), Some(allowed)) => child.is_subset(allowed),
        };
        let evidence_ok = match (self.evidence_above, parent.evidence_above) {
            (_, None) => true,
            (None, Some(_)) => false,
            (Some(child), Some(limit)) => {
                child <= limit && self.evidence_policy == parent.evidence_policy
            }
        };
        payees_ok
            && evidence_ok
            && self.max_per_payment <= parent.max_per_payment
            && self.burst <= parent.burst
            && self.rate <= parent.rate
            && self.expires_at <= parent.expires_at
            && (parent.receipts <= self.receipts)
            && (self.may_delegate <= parent.may_delegate)
    }
}

/// A token bucket (spec §5.13): `tokens` as of `refilled_at`, a capacity of
/// `burst`, refilled by `rate` each Pulse. A charter allotment's bucket
/// keeps what its children's were not carved from.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize,
)]
pub struct Bucket {
    /// What it holds, as of `refilled_at`.
    pub tokens: Amount,
    /// When it was last refilled.
    pub refilled_at: Pulse,
    /// Its capacity.
    pub burst: Amount,
    /// What refills it each Pulse.
    pub rate: Amount,
}

impl Bucket {
    /// A full bucket of `burst` and `rate`, as of `at`.
    pub fn full(burst: Amount, rate: Amount, at: Pulse) -> Self {
        Bucket {
            tokens: burst,
            refilled_at: at,
            burst,
            rate,
        }
    }

    /// A bucket that holds and refills nothing: a bounded total's
    /// allotments have one, unused.
    pub fn none(at: Pulse) -> Self {
        Bucket::full(Amount::ZERO, Amount::ZERO, at)
    }

    /// What the bucket holds at `pulse`, refilled but not changed.
    pub fn level(&self, pulse: Pulse) -> Amount {
        let mut copy = *self;
        copy.refill(pulse);
        copy.tokens
    }

    /// **refill** (spec §5.13): adds `rate` for each Pulse since the last
    /// refill, up to `burst` (saturating), and moves `refilled_at` to
    /// `pulse`. An earlier Pulse changes nothing.
    pub fn refill(&mut self, pulse: Pulse) {
        if pulse > self.refilled_at {
            let elapsed = u128::from(pulse.0 - self.refilled_at.0);
            let gained = self.rate.0.saturating_mul(elapsed);
            self.tokens = Amount(self.tokens.0.saturating_add(gained).min(self.burst.0));
            self.refilled_at = pulse;
        }
    }

    /// **carve** (spec §5.13): moves `burst` of the capacity, `rate` of the
    /// refill and `take` of the tokens out to a child, failing if the
    /// bucket keeps less of any. Refill first.
    pub fn carve(&mut self, burst: Amount, rate: Amount, take: Amount) -> Result<(), ExecError> {
        if burst > self.burst {
            return Err(ExecError::Caveat(format!(
                "burst left to carve is {}, below {burst}",
                self.burst
            )));
        }
        if rate > self.rate {
            return Err(ExecError::Caveat(format!(
                "rate left to carve is {}, below {rate}",
                self.rate
            )));
        }
        if take > self.tokens {
            return Err(ExecError::Caveat(format!(
                "the bucket holds {}, below the {take} a child starts with",
                self.tokens
            )));
        }
        self.burst = self.burst.checked_sub(burst)?;
        self.rate = self.rate.checked_sub(rate)?;
        self.tokens = self.tokens.checked_sub(take)?.min(self.burst);
        Ok(())
    }

    /// Takes `amount` of the tokens, failing if it holds less. Refill first.
    pub fn take(&mut self, amount: Amount) -> Result<(), ExecError> {
        if amount > self.tokens {
            return Err(ExecError::Caveat(format!(
                "the bucket holds {}, below {amount}",
                self.tokens
            )));
        }
        self.tokens = self.tokens.checked_sub(amount)?;
        Ok(())
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
    /// The parent charter, for delegated charters.
    pub parent: Option<ObjectId>,
    /// Its place in its tree (spec v1.14).
    pub lineage: Lineage,
    /// Allotments funded under this charter.
    pub allotments: BTreeSet<ObjectId>,
    /// Creation Pulse.
    pub created_at: Pulse,
    /// Its identity record, if it is an agent identity (spec v1.8).
    pub identity: Option<AgentIdentity>,
}

/// Limits an overseer adds to an agent and everything below it after the
/// fact (spec §5.57): every payment in the subtree is also checked against
/// them, and none of it may spawn unless `may_delegate`.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct Narrowing {
    /// Allowed payees; `None` adds no limit.
    pub payees: Option<BTreeSet<AccountId>>,
    /// Largest single payment.
    pub max_per_payment: Amount,
    /// Last Pulse in which the subtree may pay.
    pub expires_at: Pulse,
    /// Whether the subtree may still spawn.
    pub may_delegate: bool,
}

impl Narrowing {
    /// Whether `self` is at least as strict as `previous` (spec §5.13
    /// **narrows**).
    pub fn attenuates(&self, previous: &Narrowing) -> bool {
        let payees_ok = match (&self.payees, &previous.payees) {
            (_, None) => true,
            (None, Some(_)) => false,
            (Some(new), Some(old)) => new.is_subset(old),
        };
        payees_ok
            && self.max_per_payment <= previous.max_per_payment
            && self.expires_at <= previous.expires_at
            && (self.may_delegate <= previous.may_delegate)
    }

    /// Checks one payment against the narrowing.
    pub fn permits(
        &self,
        payee: &AccountId,
        amount: Amount,
        pulse: Pulse,
    ) -> Result<(), ExecError> {
        if pulse > self.expires_at {
            return Err(ExecError::Caveat("narrowed: expired".into()));
        }
        if let Some(payees) = &self.payees {
            if !payees.contains(payee) {
                return Err(ExecError::Caveat("narrowed: payee not allowed".into()));
            }
        }
        if amount > self.max_per_payment {
            return Err(ExecError::Caveat(format!(
                "narrowed: payment {amount} above {}",
                self.max_per_payment
            )));
        }
        Ok(())
    }
}

/// What happened to an agent after it was created (spec §5.13): a shared
/// object at [`overlay_id`] of the agent's id, in the agent's charter
/// domain, written only by `RevokeCharter`, `SuspendAgent`, `ResumeAgent`
/// and `NarrowAgent`.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct AgentOverlay {
    /// The agent.
    pub agent: ObjectId,
    /// When it was revoked, if it was.
    pub revoked_at: Option<Pulse>,
    /// Who suspended it, while it is suspended.
    pub suspended_by: Option<AccountId>,
    /// Limits added after the fact.
    pub narrowing: Option<Narrowing>,
}

impl AgentOverlay {
    /// An overlay that records nothing yet.
    pub fn new(agent: ObjectId) -> Self {
        AgentOverlay {
            agent,
            revoked_at: None,
            suspended_by: None,
            narrowing: None,
        }
    }
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
    /// The bucket a charter allotment spends from (spec v1.14); unused for
    /// a bounded total's.
    pub bucket: Bucket,
    /// How many subagents were ever spawned from it: the next lineage
    /// index (spec v1.14).
    pub spawned: u64,
}

impl Allotment {
    /// Checks a payment against the caveats, the narrowings above the
    /// agent and the balance, and records it (spec §5.13 **spend**).
    /// `evidence_ok` reports whether a required evidence claim passed.
    pub fn spend(
        &mut self,
        payee: &AccountId,
        amount: Amount,
        pulse: Pulse,
        evidence_ok: Option<bool>,
        narrowings: &[Narrowing],
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
            for narrowing in narrowings {
                narrowing.permits(payee, amount, pulse)?;
            }
            self.bucket.refill(pulse);
            if amount > self.bucket.tokens {
                return Err(ExecError::Caveat(format!(
                    "the bucket holds {}, below the payment {amount}",
                    self.bucket.tokens
                )));
            }
            if let Some(threshold) = caveats.evidence_above {
                if amount > threshold && evidence_ok != Some(true) {
                    return Err(ExecError::Caveat(format!(
                        "payments above {threshold} need evidence whose cost of corruption covers κ × amount"
                    )));
                }
            }
            self.bucket.take(amount)?;
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
            burst: Amount::units(2_000),
            rate: Amount::units(100),
            expires_at: Pulse(100),
            evidence_above: Some(Amount::units(500)),
            evidence_policy: Policy::standard(),
            receipts: true,
            may_delegate: true,
        }
    }

    fn allotment() -> Allotment {
        let caveats = caveats();
        Allotment {
            source: ObjectId::derive(b"charter"),
            writer: AccountId::derive(b"agent"),
            asset: AssetId::usd(),
            remaining: Amount::units(10_000),
            bucket: Bucket::full(caveats.burst, caveats.rate, Pulse(0)),
            caveats: Some(caveats),
            spawned: 0,
        }
    }

    #[test]
    fn caveats_bound_the_worst_case() {
        let supplier = AccountId::derive(b"supplier");
        let mut a = allotment();
        a.spend(&supplier, Amount::units(400), Pulse(1), None, &[])
            .unwrap();
        assert!(a
            .spend(
                &AccountId::derive(b"attacker"),
                Amount::units(1),
                Pulse(1),
                None,
                &[]
            )
            .is_err());
        assert!(a
            .spend(&supplier, Amount::units(2_001), Pulse(1), Some(true), &[])
            .is_err());
        assert!(
            a.spend(&supplier, Amount::units(600), Pulse(1), None, &[])
                .is_err(),
            "needs evidence"
        );
        a.spend(&supplier, Amount::units(600), Pulse(1), Some(true), &[])
            .unwrap();
        // 1,000 left in the bucket, 100 more a Pulse later.
        assert!(
            a.spend(&supplier, Amount::units(1_101), Pulse(2), Some(true), &[])
                .is_err(),
            "the bucket holds 1,100"
        );
        a.spend(&supplier, Amount::units(1_100), Pulse(2), Some(true), &[])
            .unwrap();
        // Empty; ten Pulses refill 1,000.
        a.spend(&supplier, Amount::units(1_000), Pulse(12), Some(true), &[])
            .unwrap();
        assert!(
            a.spend(&supplier, Amount::units(1), Pulse(101), None, &[])
                .is_err(),
            "expired"
        );
        assert_eq!(a.remaining, Amount::units(6_900));
    }

    #[test]
    fn delegation_only_narrows() {
        let parent = caveats();
        let mut child = parent.clone();
        child.max_per_payment = Amount::units(100);
        child.burst = Amount::units(10);
        child.rate = Amount::ZERO;
        assert!(child.attenuates(&parent));
        child.payees = None;
        assert!(!child.attenuates(&parent));
        let mut wider = parent.clone();
        wider.expires_at = Pulse(200);
        assert!(!wider.attenuates(&parent));
        let mut no_evidence = parent.clone();
        no_evidence.evidence_above = None;
        assert!(!no_evidence.attenuates(&parent));
        let mut faster = parent.clone();
        faster.rate = Amount::units(101);
        assert!(!faster.attenuates(&parent));
        let mut bigger = parent.clone();
        bigger.burst = Amount::units(2_001);
        assert!(!bigger.attenuates(&caveats()));
        // Evidence the parent requires is judged by the parent's policy.
        let mut lax = parent;
        lax.evidence_policy
            .trusted_classes
            .insert(sion_assurance::AtomClass::Economic);
        assert!(!lax.attenuates(&caveats()));
    }

    #[test]
    fn a_bucket_refills_up_to_its_burst_and_never_overflows() {
        let mut b = Bucket::full(Amount(10), Amount(3), Pulse(5));
        b.take(Amount(10)).unwrap();
        assert!(b.take(Amount(1)).is_err());
        b.refill(Pulse(7));
        assert_eq!(b.tokens, Amount(6));
        assert_eq!(b.level(Pulse(100)), Amount(10));
        // An earlier Pulse changes nothing.
        b.refill(Pulse(6));
        assert_eq!((b.tokens, b.refilled_at), (Amount(6), Pulse(7)));
        let mut huge = Bucket::full(Amount(u128::MAX), Amount(u128::MAX), Pulse(0));
        huge.take(Amount(u128::MAX)).unwrap();
        huge.refill(Pulse(u64::MAX));
        assert_eq!(huge.tokens, Amount(u128::MAX));
        assert_eq!(Bucket::none(Pulse(3)).level(Pulse(1_000)), Amount::ZERO);
    }

    #[test]
    fn carving_moves_burst_rate_and_tokens_down() {
        let mut b = Bucket::full(Amount(100), Amount(10), Pulse(0));
        b.take(Amount(70)).unwrap();
        // 30 tokens: a child cannot start with 40.
        assert!(b.carve(Amount(40), Amount(1), Amount(40)).is_err());
        assert!(b.carve(Amount(101), Amount(1), Amount(0)).is_err());
        assert!(b.carve(Amount(10), Amount(11), Amount(0)).is_err());
        b.carve(Amount(20), Amount(4), Amount(20)).unwrap();
        assert_eq!(
            (b.burst, b.rate, b.tokens),
            (Amount(80), Amount(6), Amount(10))
        );
        // A child that starts empty takes no tokens, but the parent's tokens
        // never exceed what it keeps.
        let mut c = Bucket::full(Amount(100), Amount(10), Pulse(0));
        c.carve(Amount(90), Amount(0), Amount(0)).unwrap();
        assert_eq!((c.burst, c.tokens), (Amount(10), Amount(10)));
    }

    #[test]
    fn a_lineage_names_every_ancestor() {
        let root = ObjectId::derive(b"root");
        let child = Lineage::root(root).child(3);
        let grandchild = child.child(0);
        let ids = grandchild.ancestry();
        assert_eq!(ids.len(), 3);
        assert_eq!(ids[0], root);
        assert_eq!(ids[1], lineage_id(&root, 3));
        assert_eq!(ids[2], lineage_id(&ids[1], 0));
        assert_eq!(grandchild.depth(), 2);
        // Different parents or indices give different ids, and none is an
        // overlay's or a keyless account's.
        assert_ne!(lineage_id(&root, 0), lineage_id(&root, 1));
        assert_ne!(lineage_id(&root, 0), lineage_id(&ids[1], 0));
        assert_ne!(overlay_id(&root), lineage_id(&root, 0));
        assert_ne!(overlay_id(&root).0, keyless_account(&root).0);
    }

    #[test]
    fn narrowings_narrow_and_check_payments() {
        let supplier = AccountId::derive(b"supplier");
        let n = Narrowing {
            payees: Some([supplier].into_iter().collect()),
            max_per_payment: Amount::units(5),
            expires_at: Pulse(50),
            may_delegate: false,
        };
        assert!(n.permits(&supplier, Amount::units(5), Pulse(50)).is_ok());
        assert!(n.permits(&supplier, Amount::units(6), Pulse(1)).is_err());
        assert!(n.permits(&supplier, Amount::units(1), Pulse(51)).is_err());
        assert!(n
            .permits(&AccountId::derive(b"other"), Amount::units(1), Pulse(1))
            .is_err());
        let mut wider = n.clone();
        wider.payees = None;
        assert!(!wider.attenuates(&n));
        assert!(n.attenuates(&wider));
        let mut spawning = n.clone();
        spawning.may_delegate = true;
        assert!(!spawning.attenuates(&n));
        // A spend checks every narrowing above the agent.
        let mut a = allotment();
        let err = a.spend(
            &supplier,
            Amount::units(6),
            Pulse(1),
            None,
            std::slice::from_ref(&n),
        );
        assert!(err.is_err());
        a.spend(&supplier, Amount::units(5), Pulse(1), None, &[n])
            .unwrap();
    }
}
