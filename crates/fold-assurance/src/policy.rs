//! Consumer policies.
//!
//! Cost of corruption is relative to the consumer: an atom the consumer's
//! policy trusts costs infinity to break; any other atom costs its bond plus
//! its known attack cost (zero if it has neither). Each consumer evaluates its
//! own policy locally; the network only proves the formulas and bonds.

use std::collections::BTreeSet;

use borsh::{BorshDeserialize, BorshSerialize};
use fold_core::encode::Canonical;
use fold_core::hash::Hash32;
use fold_core::ids::AtomId;
use fold_core::Amount;
use serde::{Deserialize, Serialize};

use crate::atom::{Atom, AtomClass};

/// A non-negative rational `num / den`, for κ and detection factors.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Hash,
    BorshSerialize,
    BorshDeserialize,
    Serialize,
    Deserialize,
)]
pub struct Ratio {
    /// Numerator.
    pub num: u64,
    /// Denominator (never zero).
    pub den: u64,
}

impl Ratio {
    /// One.
    pub const ONE: Ratio = Ratio { num: 1, den: 1 };

    /// A ratio; panics on a zero denominator.
    pub fn new(num: u64, den: u64) -> Ratio {
        assert!(den != 0, "zero denominator");
        Ratio { num, den }
    }

    /// From thousandths, for example `Ratio::permille(500)` is one half.
    pub fn permille(value: u64) -> Ratio {
        Ratio::new(value, 1000)
    }

    /// `amount × num / den`, rounded down; saturates on overflow.
    pub fn apply(&self, amount: Amount) -> Amount {
        amount
            .mul_ratio(u128::from(self.num), u128::from(self.den))
            .unwrap_or(Amount(u128::MAX))
    }

    /// `amount × den / num` rounded up (the inverse scaling), saturating.
    pub fn apply_inverse_ceil(&self, amount: Amount) -> Amount {
        if self.num == 0 {
            return Amount(u128::MAX);
        }
        let numerator = amount.0.saturating_mul(u128::from(self.den));
        let den = u128::from(self.num);
        Amount(numerator / den + u128::from(numerator % den != 0))
    }

    /// Whether the ratio is at most one.
    pub fn at_most_one(&self) -> bool {
        self.num <= self.den
    }
}

/// A consumer's trust policy.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct Policy {
    /// Atoms trusted individually.
    pub trusted_atoms: BTreeSet<AtomId>,
    /// Classes trusted wholesale.
    pub trusted_classes: BTreeSet<AtomClass>,
    /// Atoms distrusted even if their class is trusted.
    pub distrusted_atoms: BTreeSet<AtomId>,
    /// Safety factor κ ≥ 1: accept only if cost of corruption ≥ κ × value at risk.
    pub kappa: Ratio,
}

impl Policy {
    /// Trusts cryptographic hardness and the specification, nothing else;
    /// κ = 2. A sensible default for agents.
    pub fn standard() -> Policy {
        Policy {
            trusted_atoms: BTreeSet::new(),
            trusted_classes: [AtomClass::Cryptographic, AtomClass::Specification]
                .into_iter()
                .collect(),
            distrusted_atoms: BTreeSet::new(),
            kappa: Ratio::new(2, 1),
        }
    }

    /// Trusts nothing; every assumption must be backed by money.
    pub fn paranoid() -> Policy {
        Policy {
            trusted_atoms: BTreeSet::new(),
            trusted_classes: BTreeSet::new(),
            distrusted_atoms: BTreeSet::new(),
            kappa: Ratio::new(2, 1),
        }
    }

    /// Adds trusted atoms.
    pub fn trusting(mut self, atoms: impl IntoIterator<Item = AtomId>) -> Policy {
        self.trusted_atoms.extend(atoms);
        self
    }

    /// Adds distrusted atoms.
    pub fn distrusting(mut self, atoms: impl IntoIterator<Item = AtomId>) -> Policy {
        self.distrusted_atoms.extend(atoms);
        self
    }

    /// Sets κ.
    pub fn with_kappa(mut self, kappa: Ratio) -> Policy {
        self.kappa = kappa;
        self
    }

    /// Whether the policy trusts `atom`.
    pub fn trusts(&self, atom: &Atom) -> bool {
        !self.distrusted_atoms.contains(&atom.id)
            && (self.trusted_atoms.contains(&atom.id) || self.trusted_classes.contains(&atom.class))
    }

    /// A digest of the policy, recorded with reservations.
    pub fn digest(&self) -> Hash32 {
        Canonical::digest(self, "fold/assurance/policy")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atom::standard;

    #[test]
    fn standard_policy_trusts_crypto_not_hardware() {
        let policy = Policy::standard();
        assert!(policy.trusts(&standard::sha3()));
        assert!(!policy.trusts(&standard::tee_resists_host("x")));
        let stack = standard::proof_stack("a");
        assert!(!policy.trusts(&stack));
        let trusting = policy.clone().trusting([stack.id]);
        assert!(trusting.trusts(&stack));
        let distrusting = trusting.distrusting([stack.id]);
        assert!(!distrusting.trusts(&stack));
    }

    #[test]
    fn ratios() {
        let half = Ratio::permille(500);
        assert_eq!(half.apply(Amount::units(10)), Amount::units(5));
        let kappa = Ratio::new(2, 1);
        assert_eq!(kappa.apply(Amount::units(25_000)), Amount::units(50_000));
        assert_eq!(
            kappa.apply_inverse_ceil(Amount::units(60_000)),
            Amount::units(30_000)
        );
        assert_eq!(Ratio::new(3, 1).apply_inverse_ceil(Amount(10)), Amount(4));
    }
}
