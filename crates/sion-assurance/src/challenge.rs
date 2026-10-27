//! Challenges, slashing, falsification and compensation.
//!
//! When an atom is proven broken (an attester's bonded statement shown
//! false, an equivocation, or a governance declaration that a primitive is
//! broken), three things happen, in this order:
//!
//! 1. Its bond is slashed into a pool held for compensation.
//! 2. Every active claim is re-evaluated in issue order; a claim whose formula
//!    is now false is falsified, and so is everything resting on it (the
//!    retraction propagation of a truth-maintenance system, after Hotgraph's
//!    `rg-truth-maintenance`).
//! 3. Every live reservation on a falsified claim is compensated from the
//!    pools of its broken atoms, up to its value at risk. Reservations on
//!    claims that still hold but lost an atom become degraded. Pool funds
//!    above outstanding liabilities are released (to the challenger and the
//!    treasury, as the execution layer decides).

use std::collections::{BTreeMap, BTreeSet};

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use sion_core::hash::{hash_parts, Hash32};
use sion_core::ids::{AccountId, AtomId, ClaimId, Pulse};
use sion_core::Amount;

use crate::error::AssuranceError;
use crate::formula::Formula;
use crate::registry::{ClaimStatus, Registry, ReservationStatus};

/// The state of a challenge.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub enum ChallengeStatus {
    /// Awaiting a ruling.
    Open,
    /// The adjudicator found the atom broken.
    Upheld,
    /// The adjudicator found the atom intact; the deposit is forfeited.
    Rejected,
}

/// A challenge against one bonded atom of one claim.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct Challenge {
    /// Identifier.
    pub id: Hash32,
    /// Who filed it.
    pub challenger: AccountId,
    /// The claim whose assumption is challenged.
    pub claim: ClaimId,
    /// The challenged atom.
    pub target: AtomId,
    /// Digest of the counter-evidence (stored in the data layer).
    pub evidence: Hash32,
    /// Deposit at stake against frivolous challenges.
    pub deposit: Amount,
    /// When it was filed.
    pub opened_at: Pulse,
    /// Status.
    pub status: ChallengeStatus,
}

/// A falsified claim and the broken atoms beneath it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Falsified {
    /// The claim.
    pub claim: ClaimId,
    /// Broken atoms among its assumptions.
    pub causes: Vec<AtomId>,
    /// Falsified claims it rests on, if it was falsified through them.
    pub via: Vec<ClaimId>,
}

/// One compensation payment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Compensation {
    /// The reservation compensated.
    pub reservation: Hash32,
    /// Who is paid.
    pub consumer: AccountId,
    /// The falsified claim.
    pub claim: ClaimId,
    /// Total paid.
    pub amount: Amount,
    /// Amount drawn from each broken atom's pool.
    pub from: BTreeMap<AtomId, Amount>,
}

/// Everything breaking an atom caused.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlashOutcome {
    /// The atom broken.
    pub atom: AtomId,
    /// Newly slashed amount (zero for unbonded atoms).
    pub slashed: Amount,
    /// The bond owner who lost it.
    pub slashed_owner: Option<AccountId>,
    /// Claims falsified, in issue order.
    pub falsified: Vec<Falsified>,
    /// Payments to consumers.
    pub compensations: Vec<Compensation>,
    /// Reservations that lost backing but whose claims still hold.
    pub degraded: Vec<Hash32>,
    /// Pool funds no longer needed for any liability, by atom.
    pub released: BTreeMap<AtomId, Amount>,
}

impl Registry {
    /// Marks `atom` broken at `at` and carries out slashing, falsification
    /// and compensation.
    pub fn break_atom(&mut self, atom: AtomId, at: Pulse) -> Result<SlashOutcome, AssuranceError> {
        if !self.atoms.contains_key(&atom) {
            return Err(AssuranceError::UnknownAtom(atom));
        }
        if self.broken.contains_key(&atom) {
            return Err(AssuranceError::Challenge("atom already broken".into()));
        }
        self.broken.insert(atom, at);

        // 1. Slash.
        let mut slashed = Amount::ZERO;
        let mut slashed_owner = None;
        if let Some(bond) = self.bonds.get_mut(&atom) {
            slashed = bond.available();
            bond.slashed = bond
                .slashed
                .checked_add(slashed)
                .map_err(|_| AssuranceError::Overflow)?;
            bond.pool = bond
                .pool
                .checked_add(slashed)
                .map_err(|_| AssuranceError::Overflow)?;
            slashed_owner = Some(bond.owner);
        }

        // 2. Re-evaluate claims in issue order (references point backward,
        //    so one pass propagates falsification through dependents).
        let broken: BTreeSet<AtomId> = self.broken.keys().copied().collect();
        let mut falsified = Vec::new();
        let order = self.claim_order.clone();
        for id in order {
            let Some(claim) = self.claims.get(&id) else {
                continue;
            };
            if claim.status != ClaimStatus::Active {
                continue;
            }
            let formula = claim.formula.clone();
            let holds = {
                let is_broken = |a: &AtomId| broken.contains(a);
                let claim_holds = |c: &ClaimId| self.claim_is_active(c);
                formula.holds(&is_broken, &claim_holds)
            };
            if !holds {
                let causes: Vec<AtomId> = self
                    .transitive_atoms(&id)
                    .intersection(&broken)
                    .copied()
                    .collect();
                let via: Vec<ClaimId> = formula
                    .claim_refs()
                    .into_iter()
                    .filter(|c| !self.claim_is_active(c))
                    .collect();
                if let Some(claim) = self.claims.get_mut(&id) {
                    claim.status = ClaimStatus::Falsified {
                        at,
                        causes: causes.clone(),
                    };
                }
                falsified.push(Falsified {
                    claim: id,
                    causes,
                    via,
                });
            }
        }

        // 3. Compensate or degrade reservations.
        let mut compensations = Vec::new();
        let mut degraded = Vec::new();
        let reservation_ids: Vec<Hash32> = self.reservations.keys().copied().collect();
        for id in reservation_ids {
            let reservation = self.reservations[&id].clone();
            if !reservation.is_live() {
                continue;
            }
            let claim_falsified = matches!(
                self.claims.get(&reservation.claim).map(|c| &c.status),
                Some(ClaimStatus::Falsified { .. })
            );
            if claim_falsified {
                let mut owed = reservation.value_at_risk;
                let mut from = BTreeMap::new();
                for alloc_atom in reservation.allocations.keys() {
                    if owed == Amount::ZERO {
                        break;
                    }
                    if !broken.contains(alloc_atom) {
                        continue;
                    }
                    let entitled = reservation.liability_at(alloc_atom);
                    let Some(bond) = self.bonds.get_mut(alloc_atom) else {
                        continue;
                    };
                    let pay = entitled.min(owed).min(bond.pool);
                    if pay > Amount::ZERO {
                        bond.pool = bond.pool.saturating_sub(pay);
                        owed = owed.saturating_sub(pay);
                        from.insert(*alloc_atom, pay);
                    }
                }
                let paid = reservation.value_at_risk.saturating_sub(owed);
                // Free the capacity this reservation held on intact bonds.
                for (alloc_atom, amount) in &reservation.allocations {
                    if let Some(bond) = self.bonds.get_mut(alloc_atom) {
                        bond.reserved = bond.reserved.saturating_sub(*amount);
                    }
                }
                if let Some(record) = self.reservations.get_mut(&id) {
                    record.status = ReservationStatus::Compensated { paid };
                }
                compensations.push(Compensation {
                    reservation: id,
                    consumer: reservation.consumer,
                    claim: reservation.claim,
                    amount: paid,
                    from,
                });
            } else if reservation.allocations.contains_key(&atom) {
                if let Some(record) = self.reservations.get_mut(&id) {
                    record.status = ReservationStatus::Degraded;
                }
                degraded.push(id);
            }
        }

        let released = self.release_excess_pools();
        Ok(SlashOutcome {
            atom,
            slashed,
            slashed_owner,
            falsified,
            compensations,
            degraded,
            released,
        })
    }

    /// Files a challenge against a bonded atom a claim rests on.
    pub fn open_challenge(
        &mut self,
        challenger: AccountId,
        claim: ClaimId,
        target: AtomId,
        evidence: Hash32,
        deposit: Amount,
        at: Pulse,
    ) -> Result<Hash32, AssuranceError> {
        let record = self
            .claims
            .get(&claim)
            .ok_or(AssuranceError::UnknownClaim(claim))?;
        if at > record.challenge_until {
            return Err(AssuranceError::Challenge(
                "challenge period has ended".into(),
            ));
        }
        if record.adjudicator.is_none() {
            return Err(AssuranceError::Challenge(
                "claim names no adjudicator; only cryptographic evidence can break its atoms"
                    .into(),
            ));
        }
        // Only an atom the claim rests on directly can be challenged through
        // it: the claim's adjudicator was chosen by the party that put that
        // atom at stake. To dispute an atom a referenced claim rests on,
        // challenge that claim, under its own adjudicator.
        if !record.formula.direct_atoms().contains(&target) {
            return Err(AssuranceError::Challenge(
                "target is not a direct assumption of the claim (challenge the claim that rests on it)"
                    .into(),
            ));
        }
        if self.broken.contains_key(&target) {
            return Err(AssuranceError::Challenge("target already broken".into()));
        }
        if !self.atoms.get(&target).map(|a| a.bonded).unwrap_or(false) {
            return Err(AssuranceError::Challenge(
                "only bonded atoms can be challenged".into(),
            ));
        }
        let id = hash_parts(
            "sion/assurance/challenge",
            &[&claim.0 .0, &target.0 .0, &evidence.0, &challenger.0 .0],
        );
        if self.challenges.contains_key(&id) {
            return Err(AssuranceError::Challenge("duplicate challenge".into()));
        }
        self.challenges.insert(
            id,
            Challenge {
                id,
                challenger,
                claim,
                target,
                evidence,
                deposit,
                opened_at: at,
                status: ChallengeStatus::Open,
            },
        );
        Ok(id)
    }

    /// Looks up a challenge.
    pub fn challenge(&self, id: &Hash32) -> Option<&Challenge> {
        self.challenges.get(id)
    }

    /// The claim's adjudicator rules on a challenge. An upheld challenge
    /// breaks the target atom.
    pub fn rule(
        &mut self,
        challenge: &Hash32,
        adjudicator: &AtomId,
        upheld: bool,
        at: Pulse,
    ) -> Result<Option<SlashOutcome>, AssuranceError> {
        let record = self
            .challenges
            .get(challenge)
            .ok_or_else(|| AssuranceError::Challenge("unknown challenge".into()))?
            .clone();
        if record.status != ChallengeStatus::Open {
            return Err(AssuranceError::Challenge(
                "challenge already decided".into(),
            ));
        }
        let claim = self
            .claims
            .get(&record.claim)
            .ok_or(AssuranceError::UnknownClaim(record.claim))?;
        if claim.adjudicator.as_ref() != Some(adjudicator) {
            return Err(AssuranceError::Challenge(
                "not this claim's adjudicator".into(),
            ));
        }
        if self.broken.contains_key(adjudicator) {
            return Err(AssuranceError::Challenge(
                "the adjudicator itself is broken".into(),
            ));
        }
        let status = if upheld {
            ChallengeStatus::Upheld
        } else {
            ChallengeStatus::Rejected
        };
        if let Some(entry) = self.challenges.get_mut(challenge) {
            entry.status = status;
        }
        if upheld && !self.broken.contains_key(&record.target) {
            return self.break_atom(record.target, at).map(Some);
        }
        Ok(None)
    }

    /// Declares an unbonded assumption broken (for example a hash function
    /// after a published collision). Governance action; no slashing.
    pub fn declare_broken(
        &mut self,
        atom: AtomId,
        at: Pulse,
    ) -> Result<SlashOutcome, AssuranceError> {
        self.break_atom(atom, at)
    }

    /// Re-evaluates whether a formula holds given the current broken atoms
    /// and claim statuses.
    pub fn formula_holds(&self, formula: &Formula) -> bool {
        let is_broken = |a: &AtomId| self.broken.contains_key(a);
        let claim_holds = |c: &ClaimId| self.claim_is_active(c);
        formula.holds(&is_broken, &claim_holds)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atom::{Atom, AtomClass};
    use crate::decide::accept_and_reserve;
    use crate::policy::{Policy, Ratio};
    use crate::registry::ClaimRecord;
    use proptest::prelude::*;

    fn bonded(registry: &mut Registry, label: &str, units: u64) -> AtomId {
        let id = registry
            .register_atom(Atom::bonded(AtomClass::Economic, label))
            .unwrap();
        registry
            .post_bond(
                id,
                AccountId::derive(label.as_bytes()),
                Amount::units(units),
                Pulse(0),
            )
            .unwrap();
        id
    }

    fn issue(
        registry: &mut Registry,
        name: &str,
        formula: Formula,
        adjudicator: Option<AtomId>,
    ) -> ClaimId {
        let id = ClaimId::derive(name.as_bytes());
        registry
            .issue_claim(ClaimRecord {
                id,
                issuer: AccountId::derive(b"issuer"),
                formula,
                adjudicator,
                detection: Ratio::ONE,
                issued_at: Pulse(0),
                valid_until: Pulse(90),
                challenge_until: Pulse(104),
                status: ClaimStatus::Active,
                sequence: 0,
            })
            .unwrap();
        id
    }

    fn invoice() -> (Registry, Vec<AtomId>, AtomId, ClaimId, ClaimId) {
        let mut registry = Registry::new();
        let judge = bonded(&mut registry, "judge", 5_000_000);
        let witnesses: Vec<AtomId> = (1..=3)
            .map(|i| bonded(&mut registry, &format!("w{i}"), 20_000))
            .collect();
        let delivery = issue(
            &mut registry,
            "delivery",
            Formula::or(witnesses.iter().map(|a| Formula::atom(*a))),
            Some(judge),
        );
        let payable = issue(
            &mut registry,
            "payable",
            Formula::claim(delivery),
            Some(judge),
        );
        (registry, witnesses, judge, delivery, payable)
    }

    #[test]
    fn one_lying_witness_does_not_falsify_the_quorum() {
        let (mut registry, witnesses, judge, delivery, payable) = invoice();
        let consumer = AccountId::derive(b"agent");
        let (_, reservation) = accept_and_reserve(
            &mut registry,
            &Policy::paranoid(),
            consumer,
            &payable,
            Amount::units(25_000),
            Pulse(90),
            0,
        )
        .unwrap();
        let reservation = reservation.unwrap();
        let challenge = registry
            .open_challenge(
                AccountId::derive(b"c"),
                delivery,
                witnesses[1],
                Hash32::ZERO,
                Amount::units(100),
                Pulse(5),
            )
            .unwrap();
        let outcome = registry
            .rule(&challenge, &judge, true, Pulse(6))
            .unwrap()
            .unwrap();
        assert_eq!(outcome.slashed, Amount::units(20_000));
        assert!(outcome.falsified.is_empty());
        assert!(registry.claim_is_active(&payable));
        assert_eq!(outcome.degraded, vec![reservation]);
        // The pool keeps what the degraded reservation could still claim.
        let pool = registry.bond(&witnesses[1]).unwrap().pool;
        assert!(pool > Amount::ZERO && pool <= Amount::units(20_000));
    }

    #[test]
    fn all_witnesses_lying_falsifies_dependents_and_pays_victims_in_full() {
        let (mut registry, witnesses, judge, delivery, payable) = invoice();
        let consumer = AccountId::derive(b"agent");
        accept_and_reserve(
            &mut registry,
            &Policy::paranoid(),
            consumer,
            &payable,
            Amount::units(25_000),
            Pulse(90),
            0,
        )
        .unwrap();
        let mut last = None;
        for (i, witness) in witnesses.iter().enumerate() {
            let challenge = registry
                .open_challenge(
                    AccountId::derive(&[i as u8]),
                    delivery,
                    *witness,
                    Hash32::ZERO,
                    Amount::ZERO,
                    Pulse(5),
                )
                .unwrap();
            last = registry
                .rule(&challenge, &judge, true, Pulse(6 + i as u64))
                .unwrap();
        }
        let outcome = last.unwrap();
        let falsified: Vec<ClaimId> = outcome.falsified.iter().map(|f| f.claim).collect();
        assert_eq!(falsified, vec![delivery, payable]);
        assert_eq!(outcome.falsified[1].via, vec![delivery]);
        assert_eq!(outcome.compensations.len(), 1);
        assert_eq!(outcome.compensations[0].amount, Amount::units(25_000));
        assert_eq!(outcome.compensations[0].consumer, consumer);
    }

    #[test]
    fn challenges_need_the_right_adjudicator_and_window() {
        let (mut registry, witnesses, _judge, delivery, _) = invoice();
        let late = registry.open_challenge(
            AccountId::derive(b"c"),
            delivery,
            witnesses[0],
            Hash32::ZERO,
            Amount::ZERO,
            Pulse(105),
        );
        assert!(late.is_err());
        let challenge = registry
            .open_challenge(
                AccountId::derive(b"c"),
                delivery,
                witnesses[0],
                Hash32::ZERO,
                Amount::ZERO,
                Pulse(5),
            )
            .unwrap();
        let impostor = witnesses[2];
        assert!(registry
            .rule(&challenge, &impostor, true, Pulse(6))
            .is_err());
    }

    #[test]
    fn atoms_are_challenged_through_the_claim_that_rests_on_them() {
        let (mut registry, witnesses, _judge, _delivery, payable) = invoice();
        // `payable` rests on the witnesses only through `delivery`.
        let indirect = registry.open_challenge(
            AccountId::derive(b"c"),
            payable,
            witnesses[0],
            Hash32::ZERO,
            Amount::ZERO,
            Pulse(5),
        );
        assert!(matches!(indirect, Err(AssuranceError::Challenge(_))));
    }

    /// The reservation guarantee: whatever set of atoms breaks, every
    /// consumer relying on a falsified claim is paid its value at risk in
    /// full, provided it trusted nothing that broke.
    fn guarantee_holds(
        costs: Vec<u64>,
        breaks: Vec<bool>,
        values: Vec<u64>,
    ) -> Result<(), TestCaseError> {
        let mut registry = Registry::new();
        let judge = bonded(&mut registry, "judge", 1_000_000_000);
        let atoms: Vec<AtomId> = costs
            .iter()
            .enumerate()
            .map(|(i, c)| bonded(&mut registry, &format!("a{i}"), *c))
            .collect();
        // Claims of different shapes over overlapping atoms.
        let f = |i: usize| Formula::atom(atoms[i % atoms.len()]);
        let c1 = issue(
            &mut registry,
            "c1",
            Formula::or(vec![f(0), f(1), f(2)]),
            Some(judge),
        );
        let c2 = issue(
            &mut registry,
            "c2",
            Formula::and(vec![f(1), f(3)]),
            Some(judge),
        );
        let c3 = issue(
            &mut registry,
            "c3",
            Formula::and(vec![Formula::claim(c1), Formula::or(vec![f(3), f(4)])]),
            Some(judge),
        );
        let c4 = issue(
            &mut registry,
            "c4",
            Formula::or(vec![Formula::claim(c2), Formula::claim(c3)]),
            Some(judge),
        );
        let claims = [c1, c2, c3, c4];
        let mut accepted: Vec<(ClaimId, Amount)> = Vec::new();
        for (i, value) in values.iter().enumerate() {
            let claim = claims[i % claims.len()];
            let (decision, reservation) = accept_and_reserve(
                &mut registry,
                &Policy::paranoid(),
                AccountId::derive(&[i as u8, 7]),
                &claim,
                Amount::units(*value),
                Pulse(90),
                i as u64,
            )
            .unwrap();
            if reservation.is_some() {
                prop_assert!(decision.accept);
                accepted.push((claim, Amount::units(*value)));
            }
        }
        let mut paid_total = Amount::ZERO;
        for (i, atom) in atoms.iter().enumerate() {
            if breaks[i] {
                let outcome = registry.break_atom(*atom, Pulse(10)).unwrap();
                for compensation in outcome.compensations {
                    paid_total = Amount(paid_total.0 + compensation.amount.0);
                }
            }
        }
        // Everyone relying on a falsified claim was paid in full.
        let owed: u128 = accepted
            .iter()
            .filter(|(claim, _)| !registry.claim_is_active(claim))
            .map(|(_, value)| value.0)
            .sum();
        prop_assert_eq!(paid_total.0, owed);
        for reservation in registry.reservations() {
            if let ReservationStatus::Compensated { paid } = reservation.status {
                prop_assert_eq!(paid, reservation.value_at_risk);
            }
        }
        Ok(())
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(300))]
        #[test]
        fn compensation_is_always_full(
            costs in proptest::collection::vec(1u64..200, 5),
            breaks in proptest::collection::vec(any::<bool>(), 5),
            values in proptest::collection::vec(1u64..120, 1..8),
        ) {
            guarantee_holds(costs, breaks, values)?;
        }
    }
}
