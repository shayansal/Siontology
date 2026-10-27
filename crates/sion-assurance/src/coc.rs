//! Cost of corruption.
//!
//! The cost of corruption of a formula, under a consumer's policy, is the
//! cheapest total cost of a set of atom failures that makes the formula
//! false. Computed bottom-up:
//!
//! - an atom costs its bond plus its known attack cost (∞ if trusted, 0 if
//!   already broken);
//! - `∧` costs the minimum over its parts (breaking any part breaks it);
//! - `∨` costs the sum over its parts when the parts share no
//!   finite-cost atom (every route must be broken, independently).
//!
//! When alternatives share atoms the exact value is a minimum-weight
//! hitting-set problem, NP-hard in general (quantitative attack trees,
//! Lopuhaä-Zwakenberg, Budde and Stoelinga). The interval then reports
//! `max` of the parts as a sound lower bound (breaking `φ₁ ∨ φ₂` breaks both,
//! which costs at least the dearer one) and the sum as an upper bound.
//! [`exact_coc`] computes the exact value by branching on shared atoms
//! when there are few of them.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use sion_core::ids::{AtomId, ClaimId};
use sion_core::{Amount, Cost};

use crate::formula::Formula;
use crate::policy::Policy;
use crate::registry::{ClaimStatus, Registry};

/// A cost-of-corruption interval.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CocInterval {
    /// Sound lower bound.
    pub low: Cost,
    /// Upper bound.
    pub high: Cost,
    /// Whether `low` equals the exact value (no shared atoms on any `∨`).
    pub exact: bool,
}

impl CocInterval {
    fn point(cost: Cost) -> CocInterval {
        CocInterval {
            low: cost,
            high: cost,
            exact: true,
        }
    }
}

/// The cost of breaking one atom under `policy`.
pub fn atom_cost(registry: &Registry, policy: &Policy, atom: &AtomId) -> Cost {
    if registry.is_broken(atom) || policy.assumes_broken(atom) {
        return Cost::ZERO;
    }
    let Some(info) = registry.atom(atom) else {
        // Unknown atoms back nothing.
        return Cost::ZERO;
    };
    if policy.trusts(info) {
        return Cost::Infinite;
    }
    let mut cost = Cost::ZERO;
    if info.bonded {
        if let Some(bond) = registry.bond(atom) {
            cost += Cost::Finite(bond.available());
        }
    }
    if let Some(attack) = info.attack_cost {
        cost += Cost::Finite(attack);
    }
    cost
}

/// The cost-of-corruption interval of `formula` under `policy`.
pub fn coc_interval(registry: &Registry, policy: &Policy, formula: &Formula) -> CocInterval {
    let mut memo = BTreeMap::new();
    interval(registry, policy, formula, &mut memo)
}

/// The interval for a registered claim (zero if it is not active).
pub fn claim_coc(registry: &Registry, policy: &Policy, claim: &ClaimId) -> CocInterval {
    coc_interval(registry, policy, &Formula::Claim(*claim))
}

fn interval(
    registry: &Registry,
    policy: &Policy,
    formula: &Formula,
    memo: &mut BTreeMap<ClaimId, CocInterval>,
) -> CocInterval {
    match formula {
        Formula::True => CocInterval::point(Cost::Infinite),
        Formula::False => CocInterval::point(Cost::ZERO),
        Formula::Atom(atom) => CocInterval::point(atom_cost(registry, policy, atom)),
        Formula::Claim(id) => {
            if let Some(cached) = memo.get(id) {
                return *cached;
            }
            let result = match registry.claim(id) {
                Some(claim) if claim.status == ClaimStatus::Active => {
                    interval(registry, policy, &claim.formula, memo)
                }
                _ => CocInterval::point(Cost::ZERO),
            };
            memo.insert(*id, result);
            result
        }
        Formula::And(parts) => {
            let children: Vec<CocInterval> = parts
                .iter()
                .map(|p| interval(registry, policy, p, memo))
                .collect();
            CocInterval {
                low: children
                    .iter()
                    .map(|c| c.low)
                    .min()
                    .unwrap_or(Cost::Infinite),
                high: children
                    .iter()
                    .map(|c| c.high)
                    .min()
                    .unwrap_or(Cost::Infinite),
                exact: children.iter().all(|c| c.exact),
            }
        }
        Formula::Or(parts) => {
            let children: Vec<CocInterval> = parts
                .iter()
                .map(|p| interval(registry, policy, p, memo))
                .collect();
            let high = children.iter().fold(Cost::ZERO, |total, c| total + c.high);
            let all_exact = children.iter().all(|c| c.exact);
            if alternatives_are_disjoint(registry, policy, parts) {
                CocInterval {
                    low: children.iter().fold(Cost::ZERO, |total, c| total + c.low),
                    high,
                    exact: all_exact,
                }
            } else {
                let low = children.iter().map(|c| c.low).max().unwrap_or(Cost::ZERO);
                CocInterval {
                    low,
                    high,
                    exact: all_exact && low == high,
                }
            }
        }
    }
}

/// Atoms with a finite, positive cost that a formula rests on. Atoms that
/// are trusted (∞) or already broken (0) can be shared without affecting
/// the sum rule.
fn priced_atoms(registry: &Registry, policy: &Policy, formula: &Formula) -> BTreeSet<AtomId> {
    registry
        .formula_atoms(formula)
        .into_iter()
        .filter(|atom| {
            let cost = atom_cost(registry, policy, atom);
            cost.is_finite() && cost > Cost::ZERO
        })
        .collect()
}

fn alternatives_are_disjoint(registry: &Registry, policy: &Policy, parts: &[Formula]) -> bool {
    let mut seen = BTreeSet::new();
    for part in parts {
        for atom in priced_atoms(registry, policy, part) {
            if !seen.insert(atom) {
                return false;
            }
        }
    }
    true
}

/// Occurrences of each atom in the formula with claim references unfolded.
fn occurrences(
    registry: &Registry,
    formula: &Formula,
    memo: &mut BTreeMap<ClaimId, BTreeMap<AtomId, u64>>,
) -> BTreeMap<AtomId, u64> {
    let mut out = BTreeMap::new();
    match formula {
        Formula::True | Formula::False => {}
        Formula::Atom(atom) => {
            out.insert(*atom, 1);
        }
        Formula::Claim(id) => {
            if let Some(cached) = memo.get(id) {
                return cached.clone();
            }
            if let Some(claim) = registry.claim(id) {
                if claim.status == ClaimStatus::Active {
                    out = occurrences(registry, &claim.formula, memo);
                }
            }
            memo.insert(*id, out.clone());
        }
        Formula::And(parts) | Formula::Or(parts) => {
            for part in parts {
                for (atom, count) in occurrences(registry, part, memo) {
                    *out.entry(atom).or_insert(0) += count;
                }
            }
        }
    }
    out
}

/// The exact cost of corruption, branching over atoms that occur more than
/// once (with a finite positive cost). Returns `None` if more than
/// `max_shared` such atoms exist.
pub fn exact_coc(
    registry: &Registry,
    policy: &Policy,
    formula: &Formula,
    max_shared: usize,
) -> Option<Cost> {
    let mut memo = BTreeMap::new();
    let shared: Vec<AtomId> = occurrences(registry, formula, &mut memo)
        .into_iter()
        .filter(|(atom, count)| {
            let cost = atom_cost(registry, policy, atom);
            *count > 1 && cost.is_finite() && cost > Cost::ZERO
        })
        .map(|(atom, _)| atom)
        .collect();
    if shared.len() > max_shared || shared.len() >= 63 {
        return None;
    }
    let mut best = Cost::Infinite;
    for mask in 0u64..(1u64 << shared.len()) {
        let mut fixed = BTreeMap::new();
        let mut paid = Cost::ZERO;
        for (i, atom) in shared.iter().enumerate() {
            let broken = mask & (1 << i) != 0;
            if broken {
                paid += atom_cost(registry, policy, atom);
            }
            fixed.insert(*atom, broken);
        }
        let rest = falsify_cost(registry, policy, formula, &fixed);
        best = best.min(paid + rest);
    }
    Some(best)
}

/// Cheapest way to make `formula` false when the atoms in `fixed` are
/// pinned (broken = already paid for; unbroken = may not be broken) and
/// every other atom occurs once.
fn falsify_cost(
    registry: &Registry,
    policy: &Policy,
    formula: &Formula,
    fixed: &BTreeMap<AtomId, bool>,
) -> Cost {
    match formula {
        Formula::True => Cost::Infinite,
        Formula::False => Cost::ZERO,
        Formula::Atom(atom) => match fixed.get(atom) {
            Some(true) => Cost::ZERO,
            Some(false) => Cost::Infinite,
            None => atom_cost(registry, policy, atom),
        },
        Formula::Claim(id) => match registry.claim(id) {
            Some(claim) if claim.status == ClaimStatus::Active => {
                falsify_cost(registry, policy, &claim.formula, fixed)
            }
            _ => Cost::ZERO,
        },
        Formula::And(parts) => parts
            .iter()
            .map(|p| falsify_cost(registry, policy, p, fixed))
            .min()
            .unwrap_or(Cost::Infinite),
        Formula::Or(parts) => parts.iter().fold(Cost::ZERO, |total, p| {
            total + falsify_cost(registry, policy, p, fixed)
        }),
    }
}

/// Brute force over every subset of priced atoms; for tests on tiny formulas.
pub fn brute_force_coc(registry: &Registry, policy: &Policy, formula: &Formula) -> Cost {
    let atoms: Vec<AtomId> = registry.formula_atoms(formula).into_iter().collect();
    assert!(atoms.len() <= 16, "brute force is for tiny formulas");
    let mut best = Cost::Infinite;
    for mask in 0u32..(1u32 << atoms.len()) {
        let broken: BTreeSet<AtomId> = atoms
            .iter()
            .enumerate()
            .filter(|(i, _)| mask & (1 << i) != 0)
            .map(|(_, a)| *a)
            .collect();
        let cost = broken.iter().fold(Cost::ZERO, |total, a| {
            total + atom_cost(registry, policy, a)
        });
        if cost >= best {
            continue;
        }
        let is_broken = |a: &AtomId| broken.contains(a) || registry.is_broken(a);
        if !holds_with(registry, formula, &is_broken) {
            best = cost;
        }
    }
    best
}

fn holds_with(registry: &Registry, formula: &Formula, broken: &dyn Fn(&AtomId) -> bool) -> bool {
    let claim_holds = |id: &ClaimId| match registry.claim(id) {
        Some(claim) if claim.status == ClaimStatus::Active => {
            holds_with(registry, &claim.formula, broken)
        }
        _ => false,
    };
    formula.holds(broken, &claim_holds)
}

/// Total of a set of finite amounts (helper for reports).
pub fn sum_amounts(amounts: impl IntoIterator<Item = Amount>) -> Amount {
    amounts.into_iter().fold(Amount::ZERO, |total, next| {
        Amount(total.0.saturating_add(next.0))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atom::{Atom, AtomClass};
    use crate::policy::Ratio;
    use crate::registry::ClaimRecord;
    use proptest::prelude::*;
    use sion_core::ids::{AccountId, Pulse};

    fn bonded(registry: &mut Registry, label: &str, units: u64) -> AtomId {
        let atom = Atom::bonded(AtomClass::Economic, label);
        let id = registry.register_atom(atom).unwrap();
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

    fn issue(registry: &mut Registry, name: &str, formula: Formula) -> ClaimId {
        let id = ClaimId::derive(name.as_bytes());
        registry
            .issue_claim(ClaimRecord {
                id,
                issuer: AccountId::derive(b"issuer"),
                formula,
                adjudicator: None,
                detection: Ratio::ONE,
                issued_at: Pulse(0),
                valid_until: Pulse(100),
                challenge_until: Pulse(114),
                status: ClaimStatus::Active,
                sequence: 0,
            })
            .unwrap();
        id
    }

    #[test]
    fn invoice_1042_from_the_architecture() {
        // Delivery: three witnesses at 20,000 each. Arithmetic: two proving
        // stacks. Supplier identity: a bank bonding 1,000,000.
        let mut registry = Registry::with_standard_atoms();
        let w: Vec<AtomId> = (1..=3)
            .map(|i| bonded(&mut registry, &format!("witness{i}"), 20_000))
            .collect();
        let bank = {
            let atom = Atom::bonded(AtomClass::Institutional, "bank-kyc");
            let id = registry.register_atom(atom).unwrap();
            registry
                .post_bond(
                    id,
                    AccountId::derive(b"bank"),
                    Amount::units(1_000_000),
                    Pulse(0),
                )
                .unwrap();
            id
        };
        let stack_a = crate::atom::standard::proof_stack("A").id;
        let stack_b = crate::atom::standard::proof_stack("B").id;
        let delivery = issue(
            &mut registry,
            "delivery",
            Formula::or(w.iter().map(|a| Formula::atom(*a))),
        );
        let arithmetic = issue(
            &mut registry,
            "arithmetic",
            Formula::or(vec![Formula::atom(stack_a), Formula::atom(stack_b)]),
        );
        let supplier = issue(&mut registry, "supplier", Formula::atom(bank));
        let payable = issue(
            &mut registry,
            "payable",
            Formula::and(vec![
                Formula::claim(delivery),
                Formula::claim(arithmetic),
                Formula::claim(supplier),
            ]),
        );

        let policy = Policy::standard().trusting([stack_a, stack_b]);
        let coc = claim_coc(&registry, &policy, &payable);
        assert_eq!(coc.low, Cost::Finite(Amount::units(60_000)));
        assert!(coc.exact);

        // Distrusting stack A: stack B still carries the proof branch to ∞.
        let partial = Policy::standard().trusting([stack_b]);
        assert_eq!(
            claim_coc(&registry, &partial, &payable).low,
            Cost::Finite(Amount::units(60_000))
        );
        // Trusting neither stack: the arithmetic branch costs nothing to break.
        let none = Policy::standard();
        assert_eq!(claim_coc(&registry, &none, &payable).low, Cost::ZERO);
    }

    #[test]
    fn shared_atoms_give_an_interval_and_exact_solver_closes_it() {
        let mut registry = Registry::new();
        let a = bonded(&mut registry, "a", 10);
        let b = bonded(&mut registry, "b", 20);
        let c = bonded(&mut registry, "c", 30);
        // (a ∧ b) ∨ (a ∧ c): breaking a alone breaks both routes.
        let formula = Formula::or(vec![
            Formula::and(vec![Formula::atom(a), Formula::atom(b)]),
            Formula::and(vec![Formula::atom(a), Formula::atom(c)]),
        ]);
        let policy = Policy::paranoid();
        let interval = coc_interval(&registry, &policy, &formula);
        assert_eq!(interval.low, Cost::Finite(Amount::units(10)));
        assert_eq!(interval.high, Cost::Finite(Amount::units(20)));
        assert!(!interval.exact);
        assert_eq!(
            exact_coc(&registry, &policy, &formula, 8),
            Some(Cost::Finite(Amount::units(10)))
        );
        assert_eq!(
            brute_force_coc(&registry, &policy, &formula),
            Cost::Finite(Amount::units(10))
        );
    }

    #[test]
    fn broken_and_trusted_atoms() {
        let mut registry = Registry::new();
        let a = bonded(&mut registry, "a", 10);
        let tee = registry
            .register_atom(crate::atom::standard::tee_resists_host("vendor"))
            .unwrap();
        let formula = Formula::or(vec![Formula::atom(a), Formula::atom(tee)]);
        // TEE priced at its 1,000 attack cost, never infinite under a sane policy.
        assert_eq!(
            coc_interval(&registry, &Policy::standard(), &formula).low,
            Cost::Finite(Amount::units(1_010))
        );
        let reckless = Policy::standard().trusting([tee]);
        assert_eq!(
            coc_interval(&registry, &reckless, &formula).low,
            Cost::Infinite
        );
        registry.broken.insert(a, Pulse(1));
        assert_eq!(
            coc_interval(&registry, &Policy::standard(), &formula).low,
            Cost::Finite(Amount::units(1_000))
        );
    }

    fn random_formula(depth: u32, atoms: &[AtomId]) -> impl Strategy<Value = Formula> {
        let leaf = proptest::sample::select(atoms.to_vec()).prop_map(Formula::Atom);
        leaf.prop_recursive(depth, 24, 4, |inner| {
            prop_oneof![
                proptest::collection::vec(inner.clone(), 1..4).prop_map(Formula::and),
                proptest::collection::vec(inner, 1..4).prop_map(Formula::or),
            ]
        })
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(200))]
        #[test]
        fn interval_brackets_the_exact_value(
            costs in proptest::collection::vec(1u64..50, 6),
            seed_formula in random_formula(4, &(0..6u8).map(|i| Atom::id_for(AtomClass::Economic, &format!("p{i}"))).collect::<Vec<_>>()),
        ) {
            let mut registry = Registry::new();
            for (i, cost) in costs.iter().enumerate() {
                bonded(&mut registry, &format!("p{i}"), *cost);
            }
            let policy = Policy::paranoid();
            let interval = coc_interval(&registry, &policy, &seed_formula);
            let truth = brute_force_coc(&registry, &policy, &seed_formula);
            prop_assert!(interval.low <= truth, "low {} > exact {}", interval.low, truth);
            prop_assert!(truth <= interval.high, "exact {} > high {}", truth, interval.high);
            if interval.exact {
                prop_assert_eq!(interval.low, truth);
            }
            prop_assert_eq!(exact_coc(&registry, &policy, &seed_formula, 12), Some(truth));
        }
    }
}
