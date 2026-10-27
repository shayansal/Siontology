//! Reservations: bounding total reliance on a claim.
//!
//! Cost of corruption is per claim, but reliance adds up across consumers:
//! three agents each risking 25,000 USD on a claim backed by 60,000 USD all
//! pass the decision rule at κ = 2, and a fraud would net 75,000 USD for
//! 60,000 forfeited. So every acceptance reserves κ·V of the claim's backing,
//! pushed down its formula:
//!
//! - every `∧` input carries the full reservation (breaking any one input
//!   breaks the claim);
//! - the `∨` alternatives share it (all of them must be broken);
//! - an atom carries the sum of every reservation reaching it, and no bond
//!   can be reserved beyond its unslashed amount.
//!
//! Then for any set of broken atoms, the value relying on the falsified
//! claims is at most the slashed bonds divided by κ, so victims can be paid
//! in full: a falsified `∧` has a falsified input carrying the full amount,
//! and a falsified `∨` has every alternative falsified, together carrying
//! the full amount; following this down reaches broken atoms whose
//! reservations sum to at least κ·V.
//!
//! Atoms the consumer's policy trusts absorb reservations without capacity
//! (the consumer accepts the risk that they break). Atoms that are neither
//! trusted nor bonded have no capacity: they may deter, but they cannot
//! compensate.
//!
//! When an atom occurs several times in a formula (claims sharing an
//! input), this implementation splits the atom's capacity evenly across its
//! occurrences. That is sound (the sum of its allocations never exceeds the
//! bond) but can refuse a reservation an exact linear program would admit.

use std::collections::BTreeMap;

use num_bigint::BigUint;
use num_traits::ToPrimitive;
use serde::{Deserialize, Serialize};
use sion_core::ids::{AtomId, ClaimId};
use sion_core::{Amount, Cost};

use crate::error::AssuranceError;
use crate::formula::Formula;
use crate::policy::Policy;
use crate::registry::{ClaimStatus, Registry};

const MAX_UNFOLDED_NODES: usize = 100_000;

/// How a reservation is spread over bonded atoms.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Allocation {
    /// Amount reserved at each bonded atom.
    pub per_atom: BTreeMap<AtomId, Amount>,
    /// Total requested (κ·V).
    pub amount: Amount,
}

/// Capacity of one atom for new reservations under `policy`.
fn capacity(registry: &Registry, policy: &Policy, atom: &AtomId) -> Cost {
    if registry.is_broken(atom) || policy.assumes_broken(atom) {
        return Cost::ZERO;
    }
    let Some(info) = registry.atom(atom) else {
        return Cost::ZERO;
    };
    if policy.trusts(info) {
        return Cost::Infinite;
    }
    match registry.bond(atom) {
        Some(bond) if info.bonded => Cost::Finite(bond.free_capacity()),
        _ => Cost::ZERO,
    }
}

struct Context<'a> {
    registry: &'a Registry,
    policy: &'a Policy,
    per_occurrence: BTreeMap<AtomId, Cost>,
    max_memo: BTreeMap<ClaimId, Cost>,
}

impl<'a> Context<'a> {
    fn new(
        registry: &'a Registry,
        policy: &'a Policy,
        formula: &Formula,
    ) -> Result<Self, AssuranceError> {
        let mut counts = BTreeMap::new();
        let mut nodes = 0usize;
        count_occurrences(registry, formula, &mut counts, &mut nodes)?;
        let per_occurrence = counts
            .into_iter()
            .map(|(atom, count)| {
                let cap = capacity(registry, policy, &atom);
                let share = match cap {
                    Cost::Finite(amount) => {
                        Cost::Finite(Amount(amount.0 / u128::from(count.max(1))))
                    }
                    Cost::Infinite => Cost::Infinite,
                };
                (atom, share)
            })
            .collect();
        Ok(Context {
            registry,
            policy,
            per_occurrence,
            max_memo: BTreeMap::new(),
        })
    }

    fn atom_capacity(&self, atom: &AtomId) -> Cost {
        self.per_occurrence
            .get(atom)
            .copied()
            .unwrap_or_else(|| capacity(self.registry, self.policy, atom))
    }

    fn max(&mut self, formula: &Formula) -> Cost {
        match formula {
            Formula::True => Cost::Infinite,
            Formula::False => Cost::ZERO,
            Formula::Atom(atom) => self.atom_capacity(atom),
            Formula::Claim(id) => {
                if let Some(cached) = self.max_memo.get(id) {
                    return *cached;
                }
                let result = match self.registry.claim(id) {
                    Some(claim) if claim.status == ClaimStatus::Active => {
                        let inner = claim.formula.clone();
                        self.max(&inner)
                    }
                    _ => Cost::ZERO,
                };
                self.max_memo.insert(*id, result);
                result
            }
            Formula::And(parts) => parts
                .iter()
                .map(|p| self.max(p))
                .min()
                .unwrap_or(Cost::Infinite),
            Formula::Or(parts) => parts
                .iter()
                .fold(Cost::ZERO, |total, p| total + self.max(p)),
        }
    }

    fn allocate(
        &mut self,
        formula: &Formula,
        amount: Amount,
        out: &mut BTreeMap<AtomId, Amount>,
    ) -> Result<(), AssuranceError> {
        if amount == Amount::ZERO {
            return Ok(());
        }
        match formula {
            Formula::True => Ok(()),
            Formula::False => Err(AssuranceError::Invalid(
                "cannot back a false formula".into(),
            )),
            Formula::Atom(atom) => {
                match self.atom_capacity(atom) {
                    Cost::Infinite => {}
                    Cost::Finite(cap) if cap >= amount => {
                        let entry = out.entry(*atom).or_insert(Amount::ZERO);
                        *entry = entry
                            .checked_add(amount)
                            .map_err(|_| AssuranceError::Overflow)?;
                    }
                    Cost::Finite(_) => {
                        return Err(AssuranceError::Invalid("atom capacity exceeded".into()));
                    }
                }
                Ok(())
            }
            Formula::Claim(id) => {
                let claim = self
                    .registry
                    .claim(id)
                    .ok_or(AssuranceError::UnknownClaim(*id))?;
                if claim.status != ClaimStatus::Active {
                    return Err(AssuranceError::InactiveClaim(*id));
                }
                let inner = claim.formula.clone();
                self.allocate(&inner, amount, out)
            }
            Formula::And(parts) => {
                for part in parts {
                    self.allocate(part, amount, out)?;
                }
                Ok(())
            }
            Formula::Or(parts) => {
                let maxes: Vec<Cost> = parts.iter().map(|p| self.max(p)).collect();
                // Trusted (infinite) routes absorb the whole amount.
                if let Some(index) = maxes.iter().position(|m| *m == Cost::Infinite) {
                    return self.allocate(&parts[index], amount, out);
                }
                let finite: Vec<u128> = maxes
                    .iter()
                    .map(|m| m.finite().map(|a| a.0).unwrap_or(0))
                    .collect();
                // Exact arithmetic: the sum of the routes and the products
                // below may exceed 128 bits even though every share fits.
                let total: BigUint = finite.iter().map(|cap| BigUint::from(*cap)).sum();
                if total < BigUint::from(amount.0) {
                    return Err(AssuranceError::Invalid(
                        "alternatives cannot cover the amount".into(),
                    ));
                }
                // Proportional split, rounded down, then the remainder to the
                // routes with the most headroom (deterministic by index).
                // Each share is at most its route's capacity, since the
                // amount is at most the total.
                let mut shares: Vec<u128> = finite
                    .iter()
                    .map(|cap| {
                        (BigUint::from(amount.0) * BigUint::from(*cap) / &total)
                            .to_u128()
                            .expect("a share never exceeds its route's capacity")
                    })
                    .collect();
                let mut remainder = amount.0 - shares.iter().sum::<u128>();
                while remainder > 0 {
                    let (index, _) = shares
                        .iter()
                        .zip(&finite)
                        .enumerate()
                        .map(|(i, (share, cap))| (i, cap - share))
                        .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)))
                        .expect("at least one route");
                    let headroom = finite[index] - shares[index];
                    let step = remainder.min(headroom);
                    if step == 0 {
                        return Err(AssuranceError::Invalid("no headroom for remainder".into()));
                    }
                    shares[index] += step;
                    remainder -= step;
                }
                for (part, share) in parts.iter().zip(shares) {
                    self.allocate(part, Amount(share), out)?;
                }
                Ok(())
            }
        }
    }
}

fn count_occurrences(
    registry: &Registry,
    formula: &Formula,
    counts: &mut BTreeMap<AtomId, u64>,
    nodes: &mut usize,
) -> Result<(), AssuranceError> {
    *nodes += 1;
    if *nodes > MAX_UNFOLDED_NODES {
        return Err(AssuranceError::TooLarge(*nodes));
    }
    match formula {
        Formula::True | Formula::False => Ok(()),
        Formula::Atom(atom) => {
            *counts.entry(*atom).or_insert(0) += 1;
            Ok(())
        }
        Formula::Claim(id) => match registry.claim(id) {
            Some(claim) if claim.status == ClaimStatus::Active => {
                count_occurrences(registry, &claim.formula, counts, nodes)
            }
            _ => Ok(()),
        },
        Formula::And(parts) | Formula::Or(parts) => {
            for part in parts {
                count_occurrences(registry, part, counts, nodes)?;
            }
            Ok(())
        }
    }
}

/// The largest amount that could be reserved against `formula` now.
pub fn max_reservable(
    registry: &Registry,
    policy: &Policy,
    formula: &Formula,
) -> Result<Cost, AssuranceError> {
    let mut context = Context::new(registry, policy, formula)?;
    Ok(context.max(formula))
}

/// Computes how `amount` would be spread over bonded atoms, without
/// changing the registry.
pub fn allocate(
    registry: &Registry,
    policy: &Policy,
    formula: &Formula,
    amount: Amount,
) -> Result<Allocation, AssuranceError> {
    let mut context = Context::new(registry, policy, formula)?;
    let available = context.max(formula);
    if available < Cost::Finite(amount) {
        return Err(AssuranceError::InsufficientBacking {
            requested: amount,
            available: available.to_string(),
        });
    }
    let mut per_atom = BTreeMap::new();
    context.allocate(formula, amount, &mut per_atom)?;
    Ok(Allocation { per_atom, amount })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atom::{Atom, AtomClass};
    use crate::policy::Ratio;
    use crate::registry::ClaimRecord;
    use sion_core::ids::{AccountId, Pulse};

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

    /// Found by the consensus audit: the proportional split multiplied two
    /// 128-bit amounts before dividing, which overflowed (and panicked) for
    /// large but legitimate capacities.
    #[test]
    fn large_alternatives_split_exactly() {
        let mut registry = Registry::new();
        let big = 1u128 << 64;
        let atoms: Vec<AtomId> = (0..2)
            .map(|i| {
                let id = registry
                    .register_atom(Atom::bonded(AtomClass::Economic, format!("big{i}")))
                    .unwrap();
                registry
                    .post_bond(id, AccountId::derive(&[i]), Amount(big), Pulse(0))
                    .unwrap();
                id
            })
            .collect();
        let formula = Formula::or(atoms.iter().map(|a| Formula::atom(*a)));
        let allocation =
            allocate(&registry, &Policy::paranoid(), &formula, Amount(2 * big)).unwrap();
        assert_eq!(allocation.per_atom[&atoms[0]], Amount(big));
        assert_eq!(allocation.per_atom[&atoms[1]], Amount(big));
    }

    #[test]
    fn or_splits_and_and_duplicates() {
        let mut registry = Registry::new();
        let w: Vec<AtomId> = (0..3)
            .map(|i| bonded(&mut registry, &format!("w{i}"), 20_000))
            .collect();
        let bank = bonded(&mut registry, "bank", 1_000_000);
        let formula = Formula::and(vec![
            Formula::or(w.iter().map(|a| Formula::atom(*a))),
            Formula::atom(bank),
        ]);
        let policy = Policy::paranoid();
        assert_eq!(
            max_reservable(&registry, &policy, &formula).unwrap(),
            Cost::Finite(Amount::units(60_000))
        );
        let allocation = allocate(&registry, &policy, &formula, Amount::units(50_000)).unwrap();
        // The bank carries the whole amount; witnesses share it.
        assert_eq!(allocation.per_atom[&bank], Amount::units(50_000));
        let witness_total: u128 = w.iter().map(|a| allocation.per_atom[a].0).sum();
        assert_eq!(witness_total, Amount::units(50_000).0);
        assert!(allocate(&registry, &policy, &formula, Amount::units(60_001)).is_err());
    }

    #[test]
    fn trusted_routes_absorb_without_capacity() {
        let mut registry = Registry::with_standard_atoms();
        let operator = bonded(&mut registry, "operator", 100);
        let stack = crate::atom::standard::proof_stack("A").id;
        let formula = Formula::or(vec![Formula::atom(stack), Formula::atom(operator)]);
        let trusting = Policy::paranoid().trusting([stack]);
        let allocation =
            allocate(&registry, &trusting, &formula, Amount::units(1_000_000)).unwrap();
        assert!(allocation.per_atom.is_empty());
        let paranoid = Policy::paranoid();
        assert!(allocate(&registry, &paranoid, &formula, Amount::units(101)).is_err());
    }

    #[test]
    fn shared_atoms_are_split_soundly() {
        let mut registry = Registry::new();
        let a = bonded(&mut registry, "a", 100);
        let b = bonded(&mut registry, "b", 100);
        let c = bonded(&mut registry, "c", 100);
        let mut issue = |name: &str, formula: Formula| {
            let id = ClaimId::derive(name.as_bytes());
            registry
                .issue_claim(ClaimRecord {
                    id,
                    issuer: AccountId::derive(b"i"),
                    formula,
                    adjudicator: None,
                    detection: Ratio::ONE,
                    issued_at: Pulse(0),
                    valid_until: Pulse(10),
                    challenge_until: Pulse(10),
                    status: ClaimStatus::Active,
                    sequence: 0,
                })
                .unwrap();
            id
        };
        let first = issue(
            "first",
            Formula::and(vec![Formula::atom(a), Formula::atom(b)]),
        );
        let second = issue(
            "second",
            Formula::and(vec![Formula::atom(a), Formula::atom(c)]),
        );
        // Two routes sharing atom a: each occurrence may use half of a's bond.
        let formula = Formula::or(vec![Formula::claim(first), Formula::claim(second)]);
        let max = max_reservable(&registry, &Policy::paranoid(), &formula).unwrap();
        assert_eq!(max, Cost::Finite(Amount::units(100)));
        let allocation =
            allocate(&registry, &Policy::paranoid(), &formula, Amount::units(100)).unwrap();
        assert_eq!(allocation.per_atom[&a], Amount::units(100));
        assert_eq!(allocation.per_atom[&b], Amount::units(50));
        assert_eq!(allocation.per_atom[&c], Amount::units(50));
    }
}
