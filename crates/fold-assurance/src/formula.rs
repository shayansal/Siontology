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
use std::io;

use borsh::{BorshDeserialize, BorshSerialize};
use fold_core::ids::{AtomId, ClaimId};
use serde::{Deserialize, Serialize};

/// The deepest nesting a decoded formula may have ([`Formula::depth`]).
/// Claims are limited to
/// [`MAX_FORMULA_DEPTH`](crate::registry::MAX_FORMULA_DEPTH) when they are
/// issued; this bound applies earlier, when bytes are decoded, so no
/// decoder, validator or API thread recurses without bound on hostile
/// input. A deeper formula is not a valid encoding.
pub const MAX_DECODE_DEPTH: usize = 64;

/// A monotone trust formula.
///
/// Its Borsh encoding is the derived one (a one-byte variant index, then
/// the fields); decoding additionally rejects nesting deeper than
/// [`MAX_DECODE_DEPTH`].
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, BorshSerialize, Serialize, Deserialize)]
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

    /// Nesting depth: 1 for an atom, a claim reference or a constant; one
    /// more than the deepest part for `∧` and `∨`. Computed without
    /// recursion, so any formula that decoded can be measured.
    pub fn depth(&self) -> usize {
        let mut deepest = 0;
        let mut stack = vec![(self, 1usize)];
        while let Some((formula, depth)) = stack.pop() {
            deepest = deepest.max(depth);
            if let Formula::And(parts) | Formula::Or(parts) = formula {
                stack.extend(parts.iter().map(|p| (p, depth + 1)));
            }
        }
        deepest
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

impl BorshDeserialize for Formula {
    fn deserialize_reader<R: io::Read>(reader: &mut R) -> io::Result<Self> {
        decode(reader, 1)
    }
}

/// Decodes one formula at nesting `depth`, the same layout as the derived
/// decoder but bounded.
fn decode<R: io::Read>(reader: &mut R, depth: usize) -> io::Result<Formula> {
    if depth > MAX_DECODE_DEPTH {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("formula nested deeper than {MAX_DECODE_DEPTH}"),
        ));
    }
    Ok(match u8::deserialize_reader(reader)? {
        0 => Formula::True,
        1 => Formula::False,
        2 => Formula::Atom(AtomId::deserialize_reader(reader)?),
        3 => Formula::Claim(ClaimId::deserialize_reader(reader)?),
        tag @ (4 | 5) => {
            let len = u32::deserialize_reader(reader)?;
            // The length is not trusted for allocation: parts are read one
            // at a time, so a short input fails at its end.
            let mut parts = Vec::new();
            for _ in 0..len {
                parts.push(decode(reader, depth + 1)?);
            }
            if tag == 4 {
                Formula::And(parts)
            } else {
                Formula::Or(parts)
            }
        }
        tag => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unknown formula variant {tag}"),
            ))
        }
    })
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

    /// Encodes `depth` nested one-part conjunctions around an atom without
    /// building the formula (which would itself need deep recursion to drop).
    fn nested_bytes(depth: usize) -> Vec<u8> {
        let mut bytes = Vec::new();
        for _ in 1..depth {
            bytes.push(4);
            bytes.extend_from_slice(&1u32.to_le_bytes());
        }
        bytes.push(2);
        bytes.extend_from_slice(&AtomId::derive(&[7]).0 .0);
        bytes
    }

    #[test]
    fn decoding_matches_the_derived_layout_and_bounds_depth() {
        let formula = Formula::and(vec![
            Formula::or(vec![a(1), Formula::claim(ClaimId::derive(b"c"))]),
            a(2),
            Formula::True,
            Formula::or(vec![Formula::False, a(3), a(4)]),
        ]);
        let bytes = borsh::to_vec(&formula).unwrap();
        assert_eq!(borsh::from_slice::<Formula>(&bytes).unwrap(), formula);
        assert_eq!(
            borsh::from_slice::<Formula>(&[0]).unwrap(),
            Formula::True,
            "variant indices follow declaration order"
        );

        let deepest = borsh::from_slice::<Formula>(&nested_bytes(MAX_DECODE_DEPTH)).unwrap();
        assert_eq!(deepest.depth(), MAX_DECODE_DEPTH);
        assert!(borsh::from_slice::<Formula>(&nested_bytes(MAX_DECODE_DEPTH + 1)).is_err());
        // Hostile input a million levels deep fails quickly instead of
        // overflowing the decoding thread's stack.
        assert!(borsh::from_slice::<Formula>(&nested_bytes(1_000_000)).is_err());
        // Unknown variants, truncated input and lying lengths are errors.
        assert!(borsh::from_slice::<Formula>(&[6]).is_err());
        assert!(borsh::from_slice::<Formula>(&[4, 0xff, 0xff, 0xff, 0xff, 0]).is_err());
        assert!(borsh::from_slice::<Formula>(&[2, 1, 2]).is_err());
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
