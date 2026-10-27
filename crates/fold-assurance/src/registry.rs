//! The assurance registry: atoms, claims, bonds, reservations and broken atoms.

use std::collections::{BTreeMap, BTreeSet};

use borsh::{BorshDeserialize, BorshSerialize};
use fold_core::hash::Hash32;
use fold_core::ids::{AccountId, AtomId, ClaimId, Pulse};
use fold_core::Amount;
use serde::{Deserialize, Serialize};

use crate::atom::{standard, Atom};
use crate::challenge::Challenge;
use crate::error::AssuranceError;
use crate::formula::Formula;
use crate::policy::Ratio;

/// The longest chain of claim references a claim may rest on, itself
/// included. Every traversal of claim references (cost of corruption,
/// reservation, falsification) is bounded by it.
pub const MAX_CLAIM_DEPTH: usize = 16;

/// The deepest nesting of `∧` and `∨` a claim's formula may have.
pub const MAX_FORMULA_DEPTH: usize = 32;

/// Whether a claim can still be relied on.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub enum ClaimStatus {
    /// The claim holds and is within its validity window.
    Active,
    /// The claim's formula became false.
    Falsified {
        /// When.
        at: Pulse,
        /// Broken atoms among the claim's assumptions.
        causes: Vec<AtomId>,
    },
    /// The claim's validity window ended.
    Expired,
}

/// A claim as the Assurance Algebra sees it.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct ClaimRecord {
    /// Identifier (the hash of the claim's canonical body, computed by the issuer layer).
    pub id: ClaimId,
    /// Issuer.
    pub issuer: AccountId,
    /// The trust formula.
    pub formula: Formula,
    /// The atom for the adjudicator that settles disputes, if any.
    pub adjudicator: Option<AtomId>,
    /// Detection factor δ ≤ 1: how reliably a falsehood of this kind is
    /// proven. Claims that are rarely disprovable get δ < 1.
    pub detection: Ratio,
    /// Issue time.
    pub issued_at: Pulse,
    /// End of the validity window.
    pub valid_until: Pulse,
    /// End of the challenge period (≥ `valid_until`).
    pub challenge_until: Pulse,
    /// Current status.
    pub status: ClaimStatus,
    /// Issue order; claims may only reference earlier claims.
    pub sequence: u64,
}

/// A bond backing an atom.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct BondState {
    /// The atom it backs.
    pub atom: AtomId,
    /// Who posted it.
    pub owner: AccountId,
    /// Total posted.
    pub amount: Amount,
    /// Total slashed.
    pub slashed: Amount,
    /// Reserved by consumers' reliance.
    pub reserved: Amount,
    /// Locked until the latest challenge period of any claim resting on it.
    pub locked_until: Pulse,
    /// Slashed funds held back for compensation.
    pub pool: Amount,
}

impl BondState {
    /// Unslashed amount: what an attacker would forfeit.
    pub fn available(&self) -> Amount {
        self.amount.saturating_sub(self.slashed)
    }

    /// Unslashed and unreserved amount: what new reservations may use.
    pub fn free_capacity(&self) -> Amount {
        self.available().saturating_sub(self.reserved)
    }
}

/// The status of a reservation.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub enum ReservationStatus {
    /// Backing is reserved and intact.
    Active,
    /// Ended without loss; capacity returned.
    Released,
    /// The claim was falsified and the consumer was paid.
    Compensated {
        /// Amount paid.
        paid: Amount,
    },
    /// An atom carrying part of the reservation broke but the claim still holds.
    Degraded,
}

/// A consumer's reliance on a claim, reserved against its backing.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct Reservation {
    /// Identifier.
    pub id: Hash32,
    /// The claim relied on.
    pub claim: ClaimId,
    /// Who relies on it.
    pub consumer: AccountId,
    /// Value at risk V.
    pub value_at_risk: Amount,
    /// The consumer's κ.
    pub kappa: Ratio,
    /// Reserved amount κ·V.
    pub amount: Amount,
    /// End of the reliance window.
    pub until: Pulse,
    /// Amount reserved at each bonded atom.
    pub allocations: BTreeMap<AtomId, Amount>,
    /// Digest of the consumer's policy.
    pub policy: Hash32,
    /// Status.
    pub status: ReservationStatus,
}

impl Reservation {
    /// What this reservation can claim from atom `atom`'s slashed pool:
    /// `allocation / κ`, rounded up.
    pub fn liability_at(&self, atom: &AtomId) -> Amount {
        self.allocations
            .get(atom)
            .map(|allocated| {
                self.kappa
                    .apply_inverse_ceil(*allocated)
                    .min(self.value_at_risk)
            })
            .unwrap_or(Amount::ZERO)
    }

    /// Whether the reservation still holds backing.
    pub fn is_live(&self) -> bool {
        matches!(
            self.status,
            ReservationStatus::Active | ReservationStatus::Degraded
        )
    }
}

/// Everything that expired or was released by [`Registry::advance_to`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Expiries {
    /// Claims whose validity window ended.
    pub claims: Vec<ClaimId>,
    /// Reservations released.
    pub reservations: Vec<Hash32>,
    /// Pool funds no longer needed for any liability, by atom.
    pub released_pools: BTreeMap<AtomId, Amount>,
}

/// The assurance registry.
#[derive(Clone, Debug, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Registry {
    pub(crate) atoms: BTreeMap<AtomId, Atom>,
    pub(crate) claims: BTreeMap<ClaimId, ClaimRecord>,
    pub(crate) claim_order: Vec<ClaimId>,
    pub(crate) bonds: BTreeMap<AtomId, BondState>,
    pub(crate) reservations: BTreeMap<Hash32, Reservation>,
    pub(crate) broken: BTreeMap<AtomId, Pulse>,
    pub(crate) challenges: BTreeMap<Hash32, Challenge>,
    pub(crate) pulse: Pulse,
}

impl Registry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// A registry with the standard atoms registered.
    pub fn with_standard_atoms() -> Self {
        let mut registry = Self::new();
        for atom in [
            standard::sha3(),
            standard::sha2(),
            standard::module_lattice(),
            standard::specification(),
            standard::proof_stack("A"),
            standard::proof_stack("B"),
        ] {
            registry
                .register_atom(atom)
                .expect("standard atoms are consistent");
        }
        registry
    }

    /// Current ledger time.
    pub fn pulse(&self) -> Pulse {
        self.pulse
    }

    /// Registers an atom; idempotent for an identical atom.
    pub fn register_atom(&mut self, atom: Atom) -> Result<AtomId, AssuranceError> {
        if atom.id != Atom::id_for(atom.class, &atom.label) {
            return Err(AssuranceError::Invalid(
                "atom id does not match class and label".into(),
            ));
        }
        match self.atoms.get(&atom.id) {
            Some(existing) if *existing == atom => Ok(atom.id),
            Some(_) => Err(AssuranceError::ConflictingAtom(atom.id)),
            None => {
                let id = atom.id;
                self.atoms.insert(id, atom);
                Ok(id)
            }
        }
    }

    /// Looks up an atom.
    pub fn atom(&self, id: &AtomId) -> Option<&Atom> {
        self.atoms.get(id)
    }

    /// All atoms.
    pub fn atoms(&self) -> impl Iterator<Item = &Atom> {
        self.atoms.values()
    }

    /// Posts (or tops up) the bond behind a bonded atom.
    pub fn post_bond(
        &mut self,
        atom: AtomId,
        owner: AccountId,
        amount: Amount,
        locked_until: Pulse,
    ) -> Result<(), AssuranceError> {
        let info = self
            .atoms
            .get(&atom)
            .ok_or(AssuranceError::UnknownAtom(atom))?;
        if !info.bonded {
            return Err(AssuranceError::NotBonded(atom));
        }
        if self.broken.contains_key(&atom) {
            return Err(AssuranceError::Invalid("cannot bond a broken atom".into()));
        }
        match self.bonds.get_mut(&atom) {
            Some(bond) => {
                if bond.owner != owner {
                    return Err(AssuranceError::Invalid(
                        "bond belongs to another account".into(),
                    ));
                }
                bond.amount = bond
                    .amount
                    .checked_add(amount)
                    .map_err(|_| AssuranceError::Overflow)?;
                bond.locked_until = bond.locked_until.max(locked_until);
            }
            None => {
                self.bonds.insert(
                    atom,
                    BondState {
                        atom,
                        owner,
                        amount,
                        slashed: Amount::ZERO,
                        reserved: Amount::ZERO,
                        locked_until,
                        pool: Amount::ZERO,
                    },
                );
            }
        }
        Ok(())
    }

    /// The bond behind an atom.
    pub fn bond(&self, atom: &AtomId) -> Option<&BondState> {
        self.bonds.get(atom)
    }

    /// All bonds.
    pub fn bonds(&self) -> impl Iterator<Item = &BondState> {
        self.bonds.values()
    }

    /// Withdraws the unslashed, unreserved part of a bond after its lock
    /// ends (`now` is the current Pulse).
    pub fn withdraw_bond(
        &mut self,
        atom: &AtomId,
        owner: &AccountId,
        now: Pulse,
    ) -> Result<Amount, AssuranceError> {
        let pulse = now.max(self.pulse);
        let bond = self
            .bonds
            .get_mut(atom)
            .ok_or(AssuranceError::NotBonded(*atom))?;
        if bond.owner != *owner {
            return Err(AssuranceError::Invalid("not the bond owner".into()));
        }
        if pulse <= bond.locked_until {
            return Err(AssuranceError::Invalid(format!(
                "bond locked until pulse {}",
                bond.locked_until
            )));
        }
        let free = bond.free_capacity();
        bond.amount = bond.amount.saturating_sub(free);
        Ok(free)
    }

    /// Whether an atom has been proven broken.
    pub fn is_broken(&self, atom: &AtomId) -> bool {
        self.broken.contains_key(atom)
    }

    /// Broken atoms and when they broke.
    pub fn broken_atoms(&self) -> &BTreeMap<AtomId, Pulse> {
        &self.broken
    }

    /// Issues a claim. Its atoms must be registered, every referenced claim
    /// must be active and earlier, and bonds behind its assumptions are
    /// locked through its challenge period (so a claim's backing cannot
    /// silently fall after someone relies on it).
    pub fn issue_claim(&mut self, mut record: ClaimRecord) -> Result<(), AssuranceError> {
        if self.claims.contains_key(&record.id) {
            return Err(AssuranceError::DuplicateClaim(record.id));
        }
        if record.formula.depth() > MAX_FORMULA_DEPTH {
            return Err(AssuranceError::Invalid(format!(
                "the formula nests deeper than {MAX_FORMULA_DEPTH}"
            )));
        }
        match self.reference_depth(&record.formula) {
            Some(depth) if depth < MAX_CLAIM_DEPTH => {}
            _ => {
                return Err(AssuranceError::Invalid(format!(
                    "claim references nest deeper than {MAX_CLAIM_DEPTH}"
                )))
            }
        }
        for atom in record.formula.direct_atoms() {
            if !self.atoms.contains_key(&atom) {
                return Err(AssuranceError::UnknownAtom(atom));
            }
        }
        for referenced in record.formula.claim_refs() {
            match self.claims.get(&referenced) {
                None => return Err(AssuranceError::UnknownClaim(referenced)),
                Some(claim) if claim.status != ClaimStatus::Active => {
                    return Err(AssuranceError::InactiveClaim(referenced))
                }
                Some(_) => {}
            }
        }
        if let Some(adjudicator) = record.adjudicator {
            if !self.atoms.contains_key(&adjudicator) {
                return Err(AssuranceError::UnknownAtom(adjudicator));
            }
        }
        if !record.detection.at_most_one() {
            return Err(AssuranceError::Invalid("detection factor above one".into()));
        }
        if record.valid_until < record.issued_at || record.challenge_until < record.valid_until {
            return Err(AssuranceError::Invalid(
                "window must satisfy issued ≤ valid ≤ challenge".into(),
            ));
        }
        record.status = ClaimStatus::Active;
        record.sequence = self.claim_order.len() as u64;
        let id = record.id;
        let challenge_until = record.challenge_until;
        let mut atoms = record.formula.direct_atoms();
        for referenced in record.formula.claim_refs() {
            atoms.extend(self.transitive_atoms(&referenced));
        }
        if let Some(adjudicator) = record.adjudicator {
            atoms.insert(adjudicator);
        }
        for atom in atoms {
            if let Some(bond) = self.bonds.get_mut(&atom) {
                bond.locked_until = bond.locked_until.max(challenge_until);
            }
        }
        self.claims.insert(id, record);
        self.claim_order.push(id);
        Ok(())
    }

    /// Looks up a claim.
    pub fn claim(&self, id: &ClaimId) -> Option<&ClaimRecord> {
        self.claims.get(id)
    }

    /// Claims in issue order.
    pub fn claims_in_order(&self) -> impl Iterator<Item = &ClaimRecord> {
        self.claim_order.iter().filter_map(|id| self.claims.get(id))
    }

    /// Whether a claim is active.
    pub fn claim_is_active(&self, id: &ClaimId) -> bool {
        matches!(
            self.claims.get(id).map(|c| &c.status),
            Some(ClaimStatus::Active)
        )
    }

    /// The longest chain of claim references below `formula` (0 if it
    /// references no claims), or `None` if it exceeds [`MAX_CLAIM_DEPTH`].
    /// Iterative and memoized, so it costs at most one visit per reachable
    /// claim and never recurses.
    pub fn reference_depth(&self, formula: &Formula) -> Option<usize> {
        let mut memo: BTreeMap<ClaimId, usize> = BTreeMap::new();
        let mut deepest = 0;
        for root in formula.claim_refs() {
            // Depth-first, with an explicit stack of (claim, children left,
            // deepest child so far).
            let mut stack: Vec<(ClaimId, Vec<ClaimId>, usize)> = Vec::new();
            if !memo.contains_key(&root) {
                stack.push((root, self.children(&root), 0));
            }
            while let Some((_, children, best)) = stack.last_mut() {
                if let Some(child) = children.pop() {
                    match memo.get(&child) {
                        Some(depth) => *best = (*best).max(*depth),
                        None => {
                            if stack.len() >= MAX_CLAIM_DEPTH {
                                return None;
                            }
                            let grandchildren = self.children(&child);
                            stack.push((child, grandchildren, 0));
                        }
                    }
                } else {
                    let (id, _, best) = stack.pop().expect("non-empty");
                    let depth = best + 1;
                    memo.insert(id, depth);
                    if let Some((_, _, parent)) = stack.last_mut() {
                        *parent = (*parent).max(depth);
                    }
                }
            }
            deepest = deepest.max(memo.get(&root).copied().unwrap_or(1));
        }
        (deepest <= MAX_CLAIM_DEPTH).then_some(deepest)
    }

    fn children(&self, id: &ClaimId) -> Vec<ClaimId> {
        self.claims
            .get(id)
            .map(|c| c.formula.claim_refs().into_iter().collect())
            .unwrap_or_default()
    }

    /// Every atom a claim rests on, through its references.
    pub fn transitive_atoms(&self, id: &ClaimId) -> BTreeSet<AtomId> {
        let mut out = BTreeSet::new();
        let mut seen = BTreeSet::new();
        self.collect_transitive(id, &mut out, &mut seen);
        out
    }

    fn collect_transitive(
        &self,
        id: &ClaimId,
        out: &mut BTreeSet<AtomId>,
        seen: &mut BTreeSet<ClaimId>,
    ) {
        if !seen.insert(*id) {
            return;
        }
        if let Some(claim) = self.claims.get(id) {
            out.extend(claim.formula.direct_atoms());
            for referenced in claim.formula.claim_refs() {
                self.collect_transitive(&referenced, out, seen);
            }
        }
    }

    /// Every atom a formula rests on, through claim references.
    pub fn formula_atoms(&self, formula: &Formula) -> BTreeSet<AtomId> {
        let mut out = formula.direct_atoms();
        let mut seen = BTreeSet::new();
        for referenced in formula.claim_refs() {
            self.collect_transitive(&referenced, &mut out, &mut seen);
        }
        out
    }

    /// Reservations.
    pub fn reservations(&self) -> impl Iterator<Item = &Reservation> {
        self.reservations.values()
    }

    /// Looks up a reservation.
    pub fn reservation(&self, id: &Hash32) -> Option<&Reservation> {
        self.reservations.get(id)
    }

    /// Outstanding compensation liability against an atom's pool.
    pub fn liability(&self, atom: &AtomId) -> Amount {
        self.reservations
            .values()
            .filter(|r| r.is_live())
            .map(|r| r.liability_at(atom))
            .fold(Amount::ZERO, |total, next| {
                Amount(total.0.saturating_add(next.0))
            })
    }

    /// Advances ledger time: expires claims, releases ended reservations and
    /// frees pool funds no longer needed.
    pub fn advance_to(&mut self, pulse: Pulse) -> Expiries {
        let mut expiries = Expiries::default();
        if pulse < self.pulse {
            return expiries;
        }
        self.pulse = pulse;
        for id in &self.claim_order {
            if let Some(claim) = self.claims.get_mut(id) {
                if claim.status == ClaimStatus::Active && claim.valid_until < pulse {
                    claim.status = ClaimStatus::Expired;
                    expiries.claims.push(*id);
                }
            }
        }
        let ended: Vec<Hash32> = self
            .reservations
            .values()
            .filter(|r| r.is_live() && r.until < pulse)
            .map(|r| r.id)
            .collect();
        for id in ended {
            self.release_reservation(&id);
            expiries.reservations.push(id);
        }
        expiries.released_pools = self.release_excess_pools();
        expiries
    }

    /// Releases a live reservation's capacity.
    pub(crate) fn release_reservation(&mut self, id: &Hash32) {
        let Some(reservation) = self.reservations.get_mut(id) else {
            return;
        };
        if !reservation.is_live() {
            return;
        }
        reservation.status = ReservationStatus::Released;
        let allocations = reservation.allocations.clone();
        for (atom, amount) in allocations {
            if let Some(bond) = self.bonds.get_mut(&atom) {
                bond.reserved = bond.reserved.saturating_sub(amount);
            }
        }
    }

    /// Moves pool funds above outstanding liabilities out of each pool.
    pub(crate) fn release_excess_pools(&mut self) -> BTreeMap<AtomId, Amount> {
        let atoms: Vec<AtomId> = self.bonds.keys().copied().collect();
        let mut released = BTreeMap::new();
        for atom in atoms {
            let liability = self.liability(&atom);
            if let Some(bond) = self.bonds.get_mut(&atom) {
                let excess = bond.pool.saturating_sub(liability);
                if excess > Amount::ZERO {
                    bond.pool = bond.pool.saturating_sub(excess);
                    released.insert(atom, excess);
                }
            }
        }
        released
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atom::AtomClass;

    fn account(n: u8) -> AccountId {
        AccountId::derive(&[n])
    }

    fn claim(id: &str, formula: Formula, until: u64) -> ClaimRecord {
        ClaimRecord {
            id: ClaimId::derive(id.as_bytes()),
            issuer: account(0),
            formula,
            adjudicator: None,
            detection: Ratio::ONE,
            issued_at: Pulse(0),
            valid_until: Pulse(until),
            challenge_until: Pulse(until + 14),
            status: ClaimStatus::Active,
            sequence: 0,
        }
    }

    #[test]
    fn bonds_lock_through_dependent_challenge_periods() {
        let mut registry = Registry::new();
        let witness = Atom::bonded(AtomClass::Economic, "w1");
        registry.register_atom(witness.clone()).unwrap();
        registry
            .post_bond(witness.id, account(1), Amount::units(20_000), Pulse(10))
            .unwrap();
        let base = claim("base", Formula::atom(witness.id), 90);
        registry.issue_claim(base.clone()).unwrap();
        assert_eq!(registry.bond(&witness.id).unwrap().locked_until, Pulse(104));
        let derived = claim("derived", Formula::claim(base.id), 200);
        registry.issue_claim(derived).unwrap();
        assert_eq!(registry.bond(&witness.id).unwrap().locked_until, Pulse(214));
        assert!(registry
            .withdraw_bond(&witness.id, &account(1), Pulse(214))
            .is_err());
        assert_eq!(
            registry
                .withdraw_bond(&witness.id, &account(1), Pulse(215))
                .unwrap(),
            Amount::units(20_000)
        );
    }

    /// Found by the consensus audit: a long chain of claim references made
    /// every traversal recurse once per link, so a deep enough chain
    /// overflowed a validator's stack. Depth is now bounded at issuance.
    #[test]
    fn claim_chains_and_formulas_have_bounded_depth() {
        let mut registry = Registry::new();
        let witness = Atom::bonded(AtomClass::Economic, "w");
        registry.register_atom(witness.clone()).unwrap();
        let mut previous = claim("c0", Formula::atom(witness.id), 1_000);
        registry.issue_claim(previous.clone()).unwrap();
        for i in 1..MAX_CLAIM_DEPTH {
            let next = claim(&format!("c{i}"), Formula::claim(previous.id), 1_000);
            registry.issue_claim(next.clone()).unwrap();
            previous = next;
        }
        assert_eq!(
            registry.reference_depth(&Formula::claim(previous.id)),
            Some(MAX_CLAIM_DEPTH)
        );
        let too_deep = claim("deep", Formula::claim(previous.id), 1_000);
        assert!(matches!(
            registry.issue_claim(too_deep),
            Err(AssuranceError::Invalid(_))
        ));
        // A wide diamond is cheap: every claim is visited once.
        let mut layer = vec![registry.claim_order[0]];
        for depth in 1..6 {
            let mut next = Vec::new();
            for k in 0..4 {
                let mut parts: Vec<Formula> = layer.iter().map(|id| Formula::claim(*id)).collect();
                parts.push(Formula::atom(witness.id));
                let record = claim(&format!("d{depth}-{k}"), Formula::Or(parts), 1_000);
                next.push(record.id);
                registry.issue_claim(record).unwrap();
            }
            layer = next;
        }
        let parts: Vec<Formula> = layer.iter().map(|id| Formula::claim(*id)).collect();
        assert_eq!(registry.reference_depth(&Formula::and(parts)), Some(6));

        // Nesting is bounded too (raw, unnormalized nesting, as a
        // transaction may carry it).
        let mut nested = Formula::atom(witness.id);
        for _ in 0..MAX_FORMULA_DEPTH {
            nested = Formula::And(vec![nested, Formula::atom(witness.id)]);
        }
        assert_eq!(nested.depth(), MAX_FORMULA_DEPTH + 1);
        assert!(matches!(
            registry.issue_claim(claim("nested", nested, 1_000)),
            Err(AssuranceError::Invalid(_))
        ));
    }

    #[test]
    fn claims_must_reference_known_active_claims_and_atoms() {
        let mut registry = Registry::new();
        let missing = claim("x", Formula::atom(AtomId::derive(b"nope")), 5);
        assert!(matches!(
            registry.issue_claim(missing),
            Err(AssuranceError::UnknownAtom(_))
        ));
        let dangling = claim("y", Formula::claim(ClaimId::derive(b"nope")), 5);
        assert!(matches!(
            registry.issue_claim(dangling),
            Err(AssuranceError::UnknownClaim(_))
        ));
        let ok = claim("z", Formula::True, 5);
        registry.issue_claim(ok.clone()).unwrap();
        assert!(matches!(
            registry.issue_claim(ok),
            Err(AssuranceError::DuplicateClaim(_))
        ));
    }

    #[test]
    fn claims_expire() {
        let mut registry = Registry::new();
        let record = claim("short", Formula::True, 3);
        registry.issue_claim(record.clone()).unwrap();
        let expiries = registry.advance_to(Pulse(4));
        assert_eq!(expiries.claims, vec![record.id]);
        assert_eq!(
            registry.claim(&record.id).unwrap().status,
            ClaimStatus::Expired
        );
    }

    #[test]
    fn atoms_cannot_be_redefined() {
        let mut registry = Registry::new();
        let atom = Atom::new(AtomClass::Hardware, "tee");
        registry.register_atom(atom.clone()).unwrap();
        registry.register_atom(atom.clone()).unwrap();
        let priced = atom.with_attack_cost(Amount::units(5));
        assert!(matches!(
            registry.register_atom(priced),
            Err(AssuranceError::ConflictingAtom(_))
        ));
    }
}
