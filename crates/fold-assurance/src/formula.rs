//! Trust formulas.
//!
//! A formula is a monotone Boolean expression over atoms and over other
//! claims (whose own formulas are substituted when evaluated). It holds when
//! enough of its assumptions hold: `∧` (joint use of inputs) fails if any
//! input fails; `∨` (independent alternative routes) fails only if every
//! route fails. These are the two operations of the PosBool provenance
//! semiring (Green, Karvounarakis and Tannen, PODS 2007).

use std::collections::BTreeSet;
use std::fmt;

use borsh::{BorshDeserialize, BorshSerialize};
use fold_core::ids::{AtomId, ClaimId};
use serde::{Deserialize, Serialize};

/// A monotone trust formula.
#[derive(
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    BorshSerialize,
    BorshDeserialize,
    Serialize,
    Deserialize,
)]
pub enum Formula {
    /// Always holds (nothing to break).
    True,
    /// Never holds.
    False,
    /// Holds while the atom holds.
    Atom(AtomId),
    /// Holds while the referenced claim's formula holds and the claim is active.
    Claim(ClaimId),
    /// Holds while every part holds.
    And(Vec<Formula>),
    /// Holds while any part holds.
    Or(Vec<Formula>),
}

impl Formula {
    /// An atom.
    pub fn atom(id: AtomId) -> Formula {
        Formula::Atom(id)
    }

    /// A reference to another claim.
    pub fn claim(id: ClaimId) -> Formula {
        Formula::Claim(id)
    }

    /// Conjunction, normalized: nested `∧` flattened, `True` dropped, any
    /// `False` absorbs, empty is `True`, a single part is returned as is.
    pub fn and(parts: impl IntoIterator<Item = Formula>) -> Formula {
        let mut flat = Vec::new();
        for part in parts {
            match part {
                Formula::True => {}
                Formula::False => return Formula::False,
                Formula::And(inner) => flat.extend(inner),
                other => flat.push(other),
            }
        }
        dedup(&mut flat);
        match flat.len() {
            0 => Formula::True,
            1 => flat.pop().expect("one element"),
            _ => Formula::And(flat),
        }
    }

    /// Disjunction, normalized: nested `∨` flattened, `False` dropped, any
    /// `True` absorbs, empty is `False`, a single part is returned as is.
    pub fn or(parts: impl IntoIterator<Item = Formula>) -> Formula {
        let mut flat = Vec::new();
        for part in parts {
            match part {
                Formula::False => {}
                Formula::True => return Formula::True,
                Formula::Or(inner) => flat.extend(inner),
                other => flat.push(other),
            }
        }
        dedup(&mut flat);
        match flat.len() {
            0 => Formula::False,
            1 => flat.pop().expect("one element"),
            _ => Formula::Or(flat),
        }
    }

    /// Atoms that appear directly (not through claim references).
    pub fn direct_atoms(&self) -> BTreeSet<AtomId> {
        let mut out = BTreeSet::new();
        self.collect_atoms(&mut out);
        out
    }

    fn collect_atoms(&self, out: &mut BTreeSet<AtomId>) {
        match self {
            Formula::Atom(id) => {
                out.insert(*id);
            }
            Formula::And(parts) | Formula::Or(parts) => {
                for part in parts {
                    part.collect_atoms(out);
                }
            }
            Formula::True | Formula::False | Formula::Claim(_) => {}
        }
    }

    /// Claims referenced directly.
    pub fn claim_refs(&self) -> BTreeSet<ClaimId> {
        let mut out = BTreeSet::new();
        self.collect_claims(&mut out);
        out
    }

    fn collect_claims(&self, out: &mut BTreeSet<ClaimId>) {
        match self {
            Formula::Claim(id) => {
                out.insert(*id);
            }
            Formula::And(parts) | Formula::Or(parts) => {
                for part in parts {
                    part.collect_claims(out);
                }
            }
            Formula::True | Formula::False | Formula::Atom(_) => {}
        }
    }

    /// Number of nodes (without unfolding claim references).
    pub fn size(&self) -> usize {
        match self {
            Formula::And(parts) | Formula::Or(parts) => {
                1 + parts.iter().map(Formula::size).sum::<usize>()
            }
            _ => 1,
        }
    }

    /// Evaluates the formula given which atoms are broken and a lookup for
    /// whether a referenced claim currently holds.
    pub fn holds(
        &self,
        broken: &dyn Fn(&AtomId) -> bool,
        claim_holds: &dyn Fn(&ClaimId) -> bool,
    ) -> bool {
        match self {
            Formula::True => true,
            Formula::False => false,
            Formula::Atom(id) => !broken(id),
            Formula::Claim(id) => claim_holds(id),
            Formula::And(parts) => parts.iter().all(|p| p.holds(broken, claim_holds)),
            Formula::Or(parts) => parts.iter().any(|p| p.holds(broken, claim_holds)),
        }
    }
}

/// Removes repeated parts, keeping the first occurrence (`φ ∧ φ = φ`, `φ ∨ φ = φ`).
fn dedup(parts: &mut Vec<Formula>) {
    let mut seen = BTreeSet::new();
    parts.retain(|part| seen.insert(part.clone()));
}

impl fmt::Debug for Formula {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl fmt::Display for Formula {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Formula::True => f.write_str("⊤"),
            Formula::False => f.write_str("⊥"),
            Formula::Atom(id) => write!(f, "a:{}", id.short()),
            Formula::Claim(id) => write!(f, "c:{}", id.short()),
            Formula::And(parts) | Formula::Or(parts) => {
                let symbol = if matches!(self, Formula::And(_)) {
                    " ∧ "
                } else {
                    " ∨ "
                };
                f.write_str("(")?;
                for (i, part) in parts.iter().enumerate() {
                    if i > 0 {
                        f.write_str(symbol)?;
                    }
                    write!(f, "{part}")?;
                }
                f.write_str(")")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(n: u8) -> Formula {
        Formula::atom(AtomId::derive(&[n]))
    }

    #[test]
    fn normalization() {
        assert_eq!(Formula::and(vec![]), Formula::True);
        assert_eq!(Formula::or(vec![]), Formula::False);
        assert_eq!(Formula::and(vec![a(1)]), a(1));
        assert_eq!(Formula::and(vec![a(1), Formula::False]), Formula::False);
        assert_eq!(Formula::or(vec![a(1), Formula::True]), Formula::True);
        assert_eq!(
            Formula::and(vec![a(1), Formula::and(vec![a(2), a(3)]), Formula::True]),
            Formula::And(vec![a(1), a(2), a(3)])
        );
        assert_eq!(
            Formula::or(vec![Formula::or(vec![a(1), a(2)]), Formula::False, a(3)]),
            Formula::Or(vec![a(1), a(2), a(3)])
        );
        assert_eq!(Formula::and(vec![a(1), a(1)]), a(1));
        assert_eq!(
            Formula::or(vec![a(2), a(1), a(2)]),
            Formula::Or(vec![a(2), a(1)])
        );
    }

    #[test]
    fn evaluation_is_monotone() {
        let formula = Formula::and(vec![Formula::or(vec![a(1), a(2)]), a(3)]);
        let broken =
            |set: Vec<u8>| move |id: &AtomId| set.iter().any(|n| AtomId::derive(&[*n]) == *id);
        let no_claims = |_: &ClaimId| true;
        assert!(formula.holds(&broken(vec![]), &no_claims));
        assert!(formula.holds(&broken(vec![1]), &no_claims));
        assert!(!formula.holds(&broken(vec![1, 2]), &no_claims));
        assert!(!formula.holds(&broken(vec![3]), &no_claims));
    }

    #[test]
    fn atom_and_claim_collection() {
        let c = ClaimId::derive(b"c");
        let formula = Formula::and(vec![a(1), Formula::or(vec![a(2), Formula::claim(c)])]);
        assert_eq!(formula.direct_atoms().len(), 2);
        assert_eq!(
            formula.claim_refs().into_iter().collect::<Vec<_>>(),
            vec![c]
        );
        assert_eq!(formula.size(), 5);
    }
}
