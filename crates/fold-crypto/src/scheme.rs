//! Signature schemes and their mathematical families.

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};

/// A signature scheme.
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
pub enum Scheme {
    /// ML-DSA-44, FIPS 204: module-lattice signatures.
    MlDsa44,
    /// SLH-DSA-SHA2-128s, FIPS 205: stateless hash-based, small signatures, slow signing.
    SlhDsaSha2_128s,
    /// SLH-DSA-SHA2-128f, FIPS 205: stateless hash-based, fast signing, large signatures.
    SlhDsaSha2_128f,
}

/// The hardness family a scheme's security rests on. Two schemes from
/// different families fail independently under any known attack.
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
pub enum Family {
    /// Module-LWE / module-SIS hardness.
    Lattice,
    /// Second-preimage and PRF security of a hash function.
    HashBased,
}

impl Scheme {
    /// All supported schemes.
    pub const ALL: [Scheme; 3] = [
        Scheme::MlDsa44,
        Scheme::SlhDsaSha2_128s,
        Scheme::SlhDsaSha2_128f,
    ];

    /// The scheme's family.
    pub fn family(self) -> Family {
        match self {
            Scheme::MlDsa44 => Family::Lattice,
            Scheme::SlhDsaSha2_128s | Scheme::SlhDsaSha2_128f => Family::HashBased,
        }
    }

    /// Public key length in bytes.
    pub fn public_key_len(self) -> usize {
        match self {
            Scheme::MlDsa44 => fips204::ml_dsa_44::PK_LEN,
            Scheme::SlhDsaSha2_128s => fips205::slh_dsa_sha2_128s::PK_LEN,
            Scheme::SlhDsaSha2_128f => fips205::slh_dsa_sha2_128f::PK_LEN,
        }
    }

    /// Signature length in bytes.
    pub fn signature_len(self) -> usize {
        match self {
            Scheme::MlDsa44 => fips204::ml_dsa_44::SIG_LEN,
            Scheme::SlhDsaSha2_128s => fips205::slh_dsa_sha2_128s::SIG_LEN,
            Scheme::SlhDsaSha2_128f => fips205::slh_dsa_sha2_128f::SIG_LEN,
        }
    }

    /// Standard name.
    pub fn name(self) -> &'static str {
        match self {
            Scheme::MlDsa44 => "ML-DSA-44",
            Scheme::SlhDsaSha2_128s => "SLH-DSA-SHA2-128s",
            Scheme::SlhDsaSha2_128f => "SLH-DSA-SHA2-128f",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_match_the_standards() {
        assert_eq!(Scheme::MlDsa44.public_key_len(), 1312);
        assert_eq!(Scheme::MlDsa44.signature_len(), 2420);
        assert_eq!(Scheme::SlhDsaSha2_128s.public_key_len(), 32);
        assert_eq!(Scheme::SlhDsaSha2_128s.signature_len(), 7856);
        assert_eq!(Scheme::SlhDsaSha2_128f.signature_len(), 17088);
    }

    #[test]
    fn families_are_distinct() {
        assert_ne!(Scheme::MlDsa44.family(), Scheme::SlhDsaSha2_128s.family());
        assert_eq!(
            Scheme::SlhDsaSha2_128s.family(),
            Scheme::SlhDsaSha2_128f.family()
        );
    }
}
