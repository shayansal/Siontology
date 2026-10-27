//! Failure probabilities for pricing insurance.
//!
//! For independent atoms with failure probabilities `p`,
//! `P(φ₁ ∧ φ₂ fails) = 1 − (1 − p₁)(1 − p₂)` and
//! `P(φ₁ ∨ φ₂ fails) = p₁ · p₂`. Atoms shared between parts are handled
//! exactly by conditioning on each shared atom. Correlated atoms need an
//! explicit correlation model; this module assumes independence. Floating
//! point is acceptable here: premiums are reports, not consensus values.

use std::collections::BTreeMap;

use fold_core::ids::{AtomId, ClaimId};

use crate::error::AssuranceError;
use crate::formula::Formula;
use crate::registry::{ClaimStatus, Registry};

/// Probability that `formula` fails, given independent atom failure
/// probabilities (missing atoms are treated as never failing; broken atoms
/// always fail). Conditions on at most `max_shared` shared atoms.
pub fn failure_probability(
    registry: &Registry,
    formula: &Formula,
    probabilities: &BTreeMap<AtomId, f64>,
    max_shared: usize,
) -> Result<f64, AssuranceError> {
    let mut counts = BTreeMap::new();
    count(registry, formula, &mut counts);
    let shared: Vec<AtomId> = counts
        .into_iter()
        .filter(|(_, c)| *c > 1)
        .map(|(a, _)| a)
        .collect();
    if shared.len() > max_shared || shared.len() >= 31 {
        return Err(AssuranceError::TooLarge(shared.len()));
    }
    let p = |atom: &AtomId| -> f64 {
        if registry.is_broken(atom) {
            1.0
        } else {
            probabilities
                .get(atom)
                .copied()
                .unwrap_or(0.0)
                .clamp(0.0, 1.0)
        }
    };
    let mut total = 0.0;
    for mask in 0u32..(1u32 << shared.len()) {
        let mut weight = 1.0;
        let mut fixed = BTreeMap::new();
        for (i, atom) in shared.iter().enumerate() {
            let fails = mask & (1 << i) != 0;
            weight *= if fails { p(atom) } else { 1.0 - p(atom) };
            fixed.insert(*atom, fails);
        }
        if weight == 0.0 {
            continue;
        }
        total += weight * fail(registry, formula, &fixed, &p);
    }
    Ok(total)
}

fn count(registry: &Registry, formula: &Formula, counts: &mut BTreeMap<AtomId, u32>) {
    match formula {
        Formula::Atom(atom) => *counts.entry(*atom).or_insert(0) += 1,
        Formula::Claim(id) => {
            if let Some(claim) = registry.claim(id) {
                if claim.status == ClaimStatus::Active {
                    count(registry, &claim.formula, counts);
                }
            }
        }
        Formula::And(parts) | Formula::Or(parts) => {
            parts.iter().for_each(|p| count(registry, p, counts))
        }
        Formula::True | Formula::False => {}
    }
}

fn fail(
    registry: &Registry,
    formula: &Formula,
    fixed: &BTreeMap<AtomId, bool>,
    p: &dyn Fn(&AtomId) -> f64,
) -> f64 {
    match formula {
        Formula::True => 0.0,
        Formula::False => 1.0,
        Formula::Atom(atom) => match fixed.get(atom) {
            Some(true) => 1.0,
            Some(false) => 0.0,
            None => p(atom),
        },
        Formula::Claim(id) => claim_fail(registry, id, fixed, p),
        Formula::And(parts) => {
            1.0 - parts
                .iter()
                .map(|q| 1.0 - fail(registry, q, fixed, p))
                .product::<f64>()
        }
        Formula::Or(parts) => parts.iter().map(|q| fail(registry, q, fixed, p)).product(),
    }
}

fn claim_fail(
    registry: &Registry,
    id: &ClaimId,
    fixed: &BTreeMap<AtomId, bool>,
    p: &dyn Fn(&AtomId) -> f64,
) -> f64 {
    match registry.claim(id) {
        Some(claim) if claim.status == ClaimStatus::Active => {
            fail(registry, &claim.formula, fixed, p)
        }
        _ => 1.0,
    }
}

/// An actuarially fair premium for insuring `value` against the formula
/// failing, with a loading factor (for example 1.2 for 20% loading).
pub fn premium(failure_probability: f64, value: f64, loading: f64) -> f64 {
    failure_probability * value * loading
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atom::{Atom, AtomClass};

    fn id(label: &str) -> AtomId {
        Atom::id_for(AtomClass::Economic, label)
    }

    #[test]
    fn and_or_independent() {
        let registry = Registry::new();
        let probabilities: BTreeMap<AtomId, f64> =
            [(id("a"), 0.1), (id("b"), 0.2)].into_iter().collect();
        let and = Formula::and(vec![Formula::atom(id("a")), Formula::atom(id("b"))]);
        let or = Formula::or(vec![Formula::atom(id("a")), Formula::atom(id("b"))]);
        let p_and = failure_probability(&registry, &and, &probabilities, 8).unwrap();
        let p_or = failure_probability(&registry, &or, &probabilities, 8).unwrap();
        assert!((p_and - (1.0 - 0.9 * 0.8)).abs() < 1e-12);
        assert!((p_or - 0.02).abs() < 1e-12);
    }

    #[test]
    fn shared_atoms_are_conditioned_exactly() {
        let registry = Registry::new();
        let probabilities: BTreeMap<AtomId, f64> = [(id("a"), 0.5), (id("b"), 0.5), (id("c"), 0.5)]
            .into_iter()
            .collect();
        // (a ∧ b) ∨ (a ∧ c) fails iff a fails or (b and c fail): 0.5 + 0.5·0.25.
        let formula = Formula::or(vec![
            Formula::and(vec![Formula::atom(id("a")), Formula::atom(id("b"))]),
            Formula::and(vec![Formula::atom(id("a")), Formula::atom(id("c"))]),
        ]);
        let p = failure_probability(&registry, &formula, &probabilities, 8).unwrap();
        assert!((p - 0.625).abs() < 1e-12, "{p}");
        assert!((premium(p, 1000.0, 1.2) - 750.0).abs() < 1e-9);
    }
}
