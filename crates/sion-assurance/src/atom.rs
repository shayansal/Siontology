//! Atoms: irreducible trust assumptions.

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use sion_core::ids::AtomId;
use sion_core::Amount;

/// What kind of assumption an atom is. Policies can trust whole classes.
#[derive(
    Clone,
    Copy,
    Debug,
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
pub enum AtomClass {
    /// Hardness of a cryptographic problem (hash collision resistance, lattices).
    Cryptographic,
    /// A program or circuit meets its specification.
    Code,
    /// The protocol specification itself is correct.
    Specification,
    /// Hardware integrity: a TEE resisting its own host, a sensor's secure element.
    Hardware,
    /// An institution's record (a bank's KYC, a registry).
    Institutional,
    /// A bonded party's honesty: its slashable stake backs it.
    Economic,
    /// A committee assumption (fewer than t members collude).
    Committee,
    /// An adjudicator rules honestly.
    Adjudicator,
}

impl AtomClass {
    fn tag(self) -> &'static str {
        match self {
            AtomClass::Cryptographic => "cryptographic",
            AtomClass::Code => "code",
            AtomClass::Specification => "specification",
            AtomClass::Hardware => "hardware",
            AtomClass::Institutional => "institutional",
            AtomClass::Economic => "economic",
            AtomClass::Committee => "committee",
            AtomClass::Adjudicator => "adjudicator",
        }
    }
}

/// One irreducible assumption.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct Atom {
    /// Identifier: the hash of class and label.
    pub id: AtomId,
    /// Class.
    pub class: AtomClass,
    /// Human-readable label, for example `"bond:<object id>"` or `"sha3-256"`.
    pub label: String,
    /// What an attacker must spend to break it (for example, the cheapest
    /// known physical attack on a TEE). Not slashable: it deters but does not
    /// compensate.
    pub attack_cost: Option<Amount>,
    /// Whether a slashable bond backs this atom.
    pub bonded: bool,
}

impl Atom {
    /// The identifier an atom of `class` with `label` gets.
    pub fn id_for(class: AtomClass, label: &str) -> AtomId {
        AtomId::derive_parts(&[class.tag().as_bytes(), label.as_bytes()])
    }

    /// An unbonded atom.
    pub fn new(class: AtomClass, label: impl Into<String>) -> Atom {
        let label = label.into();
        Atom {
            id: Self::id_for(class, &label),
            class,
            label,
            attack_cost: None,
            bonded: false,
        }
    }

    /// A bonded atom (economic, institutional or adjudicator honesty).
    pub fn bonded(class: AtomClass, label: impl Into<String>) -> Atom {
        Atom {
            bonded: true,
            ..Self::new(class, label)
        }
    }

    /// Sets the cheapest known attack cost.
    pub fn with_attack_cost(mut self, cost: Amount) -> Atom {
        self.attack_cost = Some(cost);
        self
    }
}

/// Standard atoms every deployment registers at genesis.
pub mod standard {
    use super::{Atom, AtomClass};

    /// SHA3-256 collision and second-preimage resistance.
    pub fn sha3() -> Atom {
        Atom::new(AtomClass::Cryptographic, "sha3-256")
    }

    /// SHA-256 security (used by SLH-DSA-SHA2).
    pub fn sha2() -> Atom {
        Atom::new(AtomClass::Cryptographic, "sha2-256")
    }

    /// Module-LWE / module-SIS hardness (ML-DSA, ML-KEM).
    pub fn module_lattice() -> Atom {
        Atom::new(AtomClass::Cryptographic, "module-lattice")
    }

    /// The Siontology protocol specification is correct.
    pub fn specification() -> Atom {
        Atom::new(AtomClass::Specification, "sion-spec-v1")
    }

    /// Proof stack `name` is sound.
    pub fn proof_stack(name: &str) -> Atom {
        Atom::new(AtomClass::Code, format!("proof-stack:{name}"))
    }

    /// A TEE resists its own host. Physical memory-bus attacks costing under
    /// 1,000 USD broke TDX, SEV-SNP and SGX attestation in 2025-2026, so the
    /// default attack cost is 1,000 USD and policies should never trust it.
    pub fn tee_resists_host(vendor: &str) -> Atom {
        Atom::new(AtomClass::Hardware, format!("tee-resists-host:{vendor}"))
            .with_attack_cost(sion_core::Amount::units(1_000))
    }

    /// Fewer than `threshold` members of a sealing committee collude.
    pub fn committee_threshold(label: &str) -> Atom {
        Atom::new(AtomClass::Committee, format!("committee:{label}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_depend_on_class_and_label() {
        let a = Atom::new(AtomClass::Cryptographic, "x");
        let b = Atom::new(AtomClass::Code, "x");
        let c = Atom::new(AtomClass::Cryptographic, "y");
        assert_ne!(a.id, b.id);
        assert_ne!(a.id, c.id);
        assert_eq!(a.id, Atom::id_for(AtomClass::Cryptographic, "x"));
    }

    #[test]
    fn standard_tee_is_priced() {
        assert_eq!(
            standard::tee_resists_host("nvidia").attack_cost,
            Some(Amount::units(1_000))
        );
    }
}
