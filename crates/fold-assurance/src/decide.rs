//! The decision rule.
//!
//! An agent with value `V` at stake accepts claim `c` when
//!
//! ```text
//! min( δ_c · CoC_low(c), CoC(adjudicator_c) ) ≥ κ · V
//! ```
//!
//! where δ ≤ 1 discounts claims whose falsehood is rarely provable and the
//! adjudicator term caps trust at the cost of corrupting the dispute
//! process. With insurance requested, the agent also reserves κ·V of the
//! claim's backing so total reliance stays within what slashing can repay.

use fold_core::hash::{hash_parts, Hash32};
use fold_core::ids::{AccountId, ClaimId, Pulse};
use fold_core::{Amount, Cost};
use serde::{Deserialize, Serialize};

use crate::coc::{atom_cost, coc_interval, CocInterval};
use crate::error::AssuranceError;
use crate::formula::Formula;
use crate::policy::Policy;
use crate::registry::{ClaimStatus, Registry, Reservation, ReservationStatus};
use crate::reserve::{allocate, max_reservable};

/// The outcome of the decision rule, with every intermediate value so an
/// agent can explain its choice.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Decision {
    /// Whether to act on the claim.
    pub accept: bool,
    /// The claim's cost-of-corruption interval.
    pub coc: CocInterval,
    /// `δ · CoC_low`.
    pub detection_adjusted: Cost,
    /// The adjudicator's cost of corruption (∞ if none is named).
    pub adjudicator_cap: Cost,
    /// `min(detection_adjusted, adjudicator_cap)`.
    pub effective: Cost,
    /// `κ · V`.
    pub required: Amount,
    /// Largest reservation the claim's backing can still take.
    pub reservable: Cost,
    /// Human-readable reasons, most important first.
    pub reasons: Vec<String>,
}

/// Applies the decision rule for a consumer with `policy` and value at risk
/// `value_at_risk`. With `insured`, acceptance also requires enough
/// unreserved backing.
pub fn decide(
    registry: &Registry,
    policy: &Policy,
    claim: &ClaimId,
    value_at_risk: Amount,
    insured: bool,
) -> Result<Decision, AssuranceError> {
    let record = registry
        .claim(claim)
        .ok_or(AssuranceError::UnknownClaim(*claim))?;
    let formula = Formula::Claim(*claim);
    let coc = coc_interval(registry, policy, &formula);
    let detection_adjusted = coc.low.scale(
        u128::from(record.detection.num),
        u128::from(record.detection.den),
    );
    let adjudicator_cap = record
        .adjudicator
        .map(|atom| atom_cost(registry, policy, &atom))
        .unwrap_or(Cost::Infinite);
    let effective = detection_adjusted.min(adjudicator_cap);
    let required = policy.kappa.apply(value_at_risk);
    let reservable = max_reservable(registry, policy, &formula)?;

    let mut reasons = Vec::new();
    let mut accept = true;
    if record.status != ClaimStatus::Active {
        accept = false;
        reasons.push(format!("claim is not active ({:?})", record.status));
    }
    if effective < Cost::Finite(required) {
        accept = false;
        if detection_adjusted <= adjudicator_cap {
            reasons.push(format!(
                "cost of corruption {} (after detection discount {}/{}) is below κ·V = {}",
                detection_adjusted, record.detection.num, record.detection.den, required
            ));
        } else {
            reasons.push(format!(
                "the adjudicator can be corrupted for {}, below κ·V = {}",
                adjudicator_cap, required
            ));
        }
    }
    if insured && reservable < Cost::Finite(required) {
        accept = false;
        reasons.push(format!(
            "only {} of backing is unreserved; κ·V = {} is needed",
            reservable, required
        ));
    }
    if accept {
        reasons.push(format!(
            "cost of corruption {effective} covers κ·V = {required}"
        ));
    }
    Ok(Decision {
        accept,
        coc,
        detection_adjusted,
        adjudicator_cap,
        effective,
        required,
        reservable,
        reasons,
    })
}

/// Decides and, if accepted, reserves κ·V of the claim's backing until
/// `until`. Returns the decision and the reservation identifier.
pub fn accept_and_reserve(
    registry: &mut Registry,
    policy: &Policy,
    consumer: AccountId,
    claim: &ClaimId,
    value_at_risk: Amount,
    until: Pulse,
    nonce: u64,
) -> Result<(Decision, Option<Hash32>), AssuranceError> {
    let decision = decide(registry, policy, claim, value_at_risk, true)?;
    if !decision.accept {
        return Ok((decision, None));
    }
    let formula = Formula::Claim(*claim);
    let allocation = allocate(registry, policy, &formula, decision.required)?;
    let id = hash_parts(
        "fold/assurance/reservation",
        &[&claim.0 .0, &consumer.0 .0, &nonce.to_be_bytes()],
    );
    if registry.reservations.contains_key(&id) {
        return Err(AssuranceError::Invalid("duplicate reservation".into()));
    }
    for (atom, amount) in &allocation.per_atom {
        let bond = registry
            .bonds
            .get_mut(atom)
            .ok_or(AssuranceError::NotBonded(*atom))?;
        bond.reserved = bond
            .reserved
            .checked_add(*amount)
            .map_err(|_| AssuranceError::Overflow)?;
        if bond.reserved > bond.available() {
            return Err(AssuranceError::Invalid("bond over-reserved".into()));
        }
    }
    registry.reservations.insert(
        id,
        Reservation {
            id,
            claim: *claim,
            consumer,
            value_at_risk,
            kappa: policy.kappa,
            amount: decision.required,
            until,
            allocations: allocation.per_atom,
            policy: policy.digest(),
            status: ReservationStatus::Active,
        },
    );
    Ok((decision, Some(id)))
}

/// Ends a reservation early (the consumer no longer relies on the claim).
pub fn release(
    registry: &mut Registry,
    reservation: &Hash32,
    consumer: &AccountId,
) -> Result<(), AssuranceError> {
    let record = registry
        .reservations
        .get(reservation)
        .ok_or_else(|| AssuranceError::Invalid("unknown reservation".into()))?;
    if record.consumer != *consumer {
        return Err(AssuranceError::Invalid(
            "not the reservation's consumer".into(),
        ));
    }
    registry.release_reservation(reservation);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atom::{Atom, AtomClass};
    use crate::policy::Ratio;
    use crate::registry::ClaimRecord;
    use fold_core::ids::AtomId;

    fn bonded(registry: &mut Registry, class: AtomClass, label: &str, units: u64) -> AtomId {
        let id = registry.register_atom(Atom::bonded(class, label)).unwrap();
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
        detection: Ratio,
    ) -> ClaimId {
        let id = ClaimId::derive(name.as_bytes());
        registry
            .issue_claim(ClaimRecord {
                id,
                issuer: AccountId::derive(b"issuer"),
                formula,
                adjudicator,
                detection,
                issued_at: Pulse(0),
                valid_until: Pulse(90),
                challenge_until: Pulse(104),
                status: ClaimStatus::Active,
                sequence: 0,
            })
            .unwrap();
        id
    }

    /// The architecture's worked example, end to end.
    #[test]
    fn invoice_1042_decisions_and_reservations() {
        let mut registry = Registry::with_standard_atoms();
        let witnesses: Vec<AtomId> = (1..=3)
            .map(|i| {
                bonded(
                    &mut registry,
                    AtomClass::Economic,
                    &format!("witness{i}"),
                    20_000,
                )
            })
            .collect();
        let bank = bonded(&mut registry, AtomClass::Institutional, "bank", 1_000_000);
        let stack_a = crate::atom::standard::proof_stack("A").id;
        let stack_b = crate::atom::standard::proof_stack("B").id;
        let delivery = issue(
            &mut registry,
            "delivery",
            Formula::or(witnesses.iter().map(|a| Formula::atom(*a))),
            None,
            Ratio::ONE,
        );
        let arithmetic = issue(
            &mut registry,
            "arithmetic",
            Formula::or(vec![Formula::atom(stack_a), Formula::atom(stack_b)]),
            None,
            Ratio::ONE,
        );
        let supplier = issue(
            &mut registry,
            "supplier",
            Formula::atom(bank),
            None,
            Ratio::ONE,
        );
        let payable = issue(
            &mut registry,
            "invoice-1042-payable",
            Formula::and(vec![
                Formula::claim(delivery),
                Formula::claim(arithmetic),
                Formula::claim(supplier),
            ]),
            None,
            Ratio::ONE,
        );
        let policy = Policy::standard().trusting([stack_a, stack_b]);

        // 25,000 at κ = 2 needs 50,000 ≤ 60,000: accepted.
        let decision = decide(&registry, &policy, &payable, Amount::units(25_000), true).unwrap();
        assert!(decision.accept, "{:?}", decision.reasons);
        assert_eq!(decision.effective, Cost::Finite(Amount::units(60_000)));
        // Up to 30,000 clears automatically; 30,001 does not.
        assert!(
            decide(&registry, &policy, &payable, Amount::units(30_000), false)
                .unwrap()
                .accept
        );
        assert!(
            !decide(&registry, &policy, &payable, Amount::units(30_001), false)
                .unwrap()
                .accept
        );

        // The first financier reserves 50,000 of the witnesses' 60,000.
        let first = AccountId::derive(b"financier-1");
        let (_, reservation) = accept_and_reserve(
            &mut registry,
            &policy,
            first,
            &payable,
            Amount::units(25_000),
            Pulse(90),
            1,
        )
        .unwrap();
        assert!(reservation.is_some());
        let reserved: u128 = witnesses
            .iter()
            .map(|a| registry.bond(a).unwrap().reserved.0)
            .sum();
        assert_eq!(reserved, Amount::units(50_000).0);
        // The second financier is refused: only 10,000 of backing is left.
        let second = AccountId::derive(b"financier-2");
        let (decision, reservation) = accept_and_reserve(
            &mut registry,
            &policy,
            second,
            &payable,
            Amount::units(25_000),
            Pulse(90),
            2,
        )
        .unwrap();
        assert!(!decision.accept);
        assert!(reservation.is_none());
        assert_eq!(decision.reservable, Cost::Finite(Amount::units(10_000)));
        // A 5,000 loan still fits (κ·V = 10,000).
        let (decision, _) = accept_and_reserve(
            &mut registry,
            &policy,
            second,
            &payable,
            Amount::units(5_000),
            Pulse(90),
            3,
        )
        .unwrap();
        assert!(decision.accept);
    }

    #[test]
    fn detection_discount_and_adjudicator_cap() {
        let mut registry = Registry::new();
        let attester = bonded(&mut registry, AtomClass::Economic, "attester", 100_000);
        let judge = bonded(&mut registry, AtomClass::Adjudicator, "judge", 10_000);
        let weak = issue(
            &mut registry,
            "weak",
            Formula::atom(attester),
            None,
            Ratio::permille(250),
        );
        let decision = decide(
            &registry,
            &Policy::paranoid(),
            &weak,
            Amount::units(10_000),
            false,
        )
        .unwrap();
        assert_eq!(
            decision.detection_adjusted,
            Cost::Finite(Amount::units(25_000))
        );
        assert!(decision.accept);
        let judged = issue(
            &mut registry,
            "judged",
            Formula::atom(attester),
            Some(judge),
            Ratio::ONE,
        );
        let decision = decide(
            &registry,
            &Policy::paranoid(),
            &judged,
            Amount::units(10_000),
            false,
        )
        .unwrap();
        assert_eq!(decision.effective, Cost::Finite(Amount::units(10_000)));
        assert!(
            !decision.accept,
            "a 10,000 judge cannot secure 20,000 of reliance"
        );
        assert!(decision.reasons[0].contains("adjudicator"));
    }

    #[test]
    fn release_frees_capacity() {
        let mut registry = Registry::new();
        let attester = bonded(&mut registry, AtomClass::Economic, "attester", 100);
        let claim = issue(
            &mut registry,
            "c",
            Formula::atom(attester),
            None,
            Ratio::ONE,
        );
        let consumer = AccountId::derive(b"consumer");
        let policy = Policy::paranoid();
        let (_, id) = accept_and_reserve(
            &mut registry,
            &policy,
            consumer,
            &claim,
            Amount::units(50),
            Pulse(10),
            0,
        )
        .unwrap();
        let id = id.unwrap();
        assert_eq!(
            registry.bond(&attester).unwrap().reserved,
            Amount::units(100)
        );
        assert!(release(&mut registry, &id, &AccountId::derive(b"someone-else")).is_err());
        release(&mut registry, &id, &consumer).unwrap();
        assert_eq!(registry.bond(&attester).unwrap().reserved, Amount::ZERO);
    }
}
