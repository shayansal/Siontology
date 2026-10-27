//! Dual-origin keypairs: one lattice key and one hash-based key.

use std::fmt;

use borsh::{BorshDeserialize, BorshSerialize};
use rand_core::CryptoRngCore;
use serde::{Deserialize, Serialize};

use crate::error::CryptoError;
use crate::keys::{verify, Keypair, PublicKey, Signature};
use crate::policy::{AuthPolicy, Authorization, KeySignature};
use crate::scheme::Scheme;

/// A lattice keypair (ML-DSA-44) and a hash-based keypair (SLH-DSA).
#[derive(Clone)]
pub struct DualKeypair {
    /// The ML-DSA-44 keypair.
    pub lattice: Keypair,
    /// The SLH-DSA keypair.
    pub hash: Keypair,
}

impl fmt::Debug for DualKeypair {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DualKeypair")
            .field("lattice", self.lattice.public())
            .field("hash", self.hash.public())
            .finish()
    }
}

/// One signature from each family.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct DualSignature {
    /// The ML-DSA-44 signature.
    pub lattice: Signature,
    /// The SLH-DSA signature.
    pub hash: Signature,
}

impl DualSignature {
    /// Total encoded size in bytes.
    pub fn len(&self) -> usize {
        self.lattice.bytes.len() + self.hash.bytes.len()
    }

    /// Whether both signatures are empty (never true for a real signature).
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl DualKeypair {
    /// Derives both keys from one seed. `hash_scheme` selects SLH-DSA-128s
    /// (small signatures, slow signing; for infrequent high-value accounts)
    /// or SLH-DSA-128f (fast signing; for busy agents and simulations).
    pub fn from_seed(seed: &[u8; 32], hash_scheme: Scheme) -> DualKeypair {
        assert_eq!(
            hash_scheme.family(),
            crate::scheme::Family::HashBased,
            "the second leg must be hash-based"
        );
        let mut lattice_seed = *seed;
        lattice_seed[0] ^= 0x4c; // 'L'
        let mut hash_seed = *seed;
        hash_seed[0] ^= 0x48; // 'H'
        DualKeypair {
            lattice: Keypair::from_seed(Scheme::MlDsa44, &lattice_seed),
            hash: Keypair::from_seed(hash_scheme, &hash_seed),
        }
    }

    /// Generates both keys from the given generator.
    pub fn generate_with_rng(rng: &mut impl CryptoRngCore, hash_scheme: Scheme) -> DualKeypair {
        let mut seed = [0u8; 32];
        rng.fill_bytes(&mut seed);
        Self::from_seed(&seed, hash_scheme)
    }

    /// The default dual authorization policy for these keys: one signature
    /// from each family is required.
    pub fn policy(&self) -> AuthPolicy {
        AuthPolicy::dual(self.lattice.public().clone(), self.hash.public().clone())
    }

    /// Public keys.
    pub fn public_keys(&self) -> (&PublicKey, &PublicKey) {
        (self.lattice.public(), self.hash.public())
    }

    /// Signs with both keys deterministically.
    pub fn sign_deterministic(&self, domain: &str, message: &[u8]) -> Result<DualSignature, CryptoError> {
        Ok(DualSignature {
            lattice: self.lattice.sign_deterministic(domain, message)?,
            hash: self.hash.sign_deterministic(domain, message)?,
        })
    }

    /// Signs with both keys using hedged randomness.
    pub fn sign_with_rng(
        &self,
        rng: &mut impl CryptoRngCore,
        domain: &str,
        message: &[u8],
    ) -> Result<DualSignature, CryptoError> {
        Ok(DualSignature {
            lattice: self.lattice.sign_with_rng(rng, domain, message)?,
            hash: self.hash.sign_with_rng(rng, domain, message)?,
        })
    }

    /// Produces an [`Authorization`] that satisfies [`DualKeypair::policy`].
    pub fn authorize(&self, domain: &str, message: &[u8]) -> Result<Authorization, CryptoError> {
        let signature = self.sign_deterministic(domain, message)?;
        Ok(Authorization {
            signatures: vec![
                KeySignature {
                    key_index: 0,
                    signature: signature.lattice,
                },
                KeySignature {
                    key_index: 1,
                    signature: signature.hash,
                },
            ],
        })
    }
}

/// Verifies a dual signature: both legs must verify.
pub fn verify_dual(
    lattice: &PublicKey,
    hash: &PublicKey,
    domain: &str,
    message: &[u8],
    signature: &DualSignature,
) -> Result<(), CryptoError> {
    verify(lattice, domain, message, &signature.lattice)?;
    verify(hash, domain, message, &signature.hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dual_signatures_need_both_legs() {
        let keys = DualKeypair::from_seed(&[9u8; 32], Scheme::SlhDsaSha2_128f);
        let (lattice, hash) = keys.public_keys();
        let signature = keys.sign_deterministic("fold/test/dual", b"m").unwrap();
        verify_dual(lattice, hash, "fold/test/dual", b"m", &signature).unwrap();
        assert_eq!(signature.len(), 2420 + 17088);

        let mut broken = signature.clone();
        broken.hash.bytes[0] ^= 1;
        assert!(verify_dual(lattice, hash, "fold/test/dual", b"m", &broken).is_err());
        let mut broken = signature;
        broken.lattice.bytes[0] ^= 1;
        assert!(verify_dual(lattice, hash, "fold/test/dual", b"m", &broken).is_err());
    }

    #[test]
    fn legs_use_independent_seeds() {
        let keys = DualKeypair::from_seed(&[1u8; 32], Scheme::SlhDsaSha2_128f);
        let alone = Keypair::from_seed(Scheme::MlDsa44, &[1u8; 32]);
        assert_ne!(keys.lattice.public(), alone.public());
    }
}
