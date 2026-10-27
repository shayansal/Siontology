//! Validator identities and signatures.
//!
//! Validators sign consensus headers, votes, lane batch roots and Pulse
//! headers with the dual scheme accounts use: one ML-DSA-44 and one
//! SLH-DSA signature over the same message, both required, under the
//! [`VALIDATOR_DOMAIN`] context. A validator's identifier is the hash of
//! its two public keys, so anyone who knows the keys can check both the
//! identifier and the signatures. The ledger uses this to check proofs of
//! key possession when a validator stakes, and evidence of equivocation
//! when one is reported.

use sion_core::encode::decode;
use sion_core::ids::ValidatorId;

use crate::dual::DualSignature;
use crate::keys::{verify, PublicKey};

/// The signing domain (FIPS context string) of every validator signature.
pub const VALIDATOR_DOMAIN: &str = "sion/validator/v1";

/// The identifier of a validator with these public keys.
pub fn validator_id(lattice: &PublicKey, hash: &PublicKey) -> ValidatorId {
    ValidatorId::derive_parts(&[&lattice.bytes, &hash.bytes])
}

/// Whether `signature` (a Borsh-encoded [`DualSignature`]) is a valid
/// validator signature over `message` by the holder of both keys.
pub fn verify_validator(
    lattice: &PublicKey,
    hash: &PublicKey,
    message: &[u8],
    signature: &[u8],
) -> bool {
    let Ok(signature) = decode::<DualSignature>(signature) else {
        return false;
    };
    verify(lattice, VALIDATOR_DOMAIN, message, &signature.lattice).is_ok()
        && verify(hash, VALIDATOR_DOMAIN, message, &signature.hash).is_ok()
}

#[cfg(test)]
mod tests {
    use sion_core::encode::encode;

    use super::*;
    use crate::{DualKeypair, Scheme};

    #[test]
    fn validator_signatures_need_both_keys_and_the_domain() {
        let keys = DualKeypair::from_seed(&[3; 32], Scheme::SlhDsaSha2_128f);
        let (lattice, hash) = (keys.lattice.public().clone(), keys.hash.public().clone());
        let signature = encode(&keys.sign_deterministic(VALIDATOR_DOMAIN, b"msg").unwrap());
        assert!(verify_validator(&lattice, &hash, b"msg", &signature));
        assert!(!verify_validator(&lattice, &hash, b"other", &signature));
        let other = DualKeypair::from_seed(&[4; 32], Scheme::SlhDsaSha2_128f);
        assert!(!verify_validator(
            other.lattice.public(),
            &hash,
            b"msg",
            &signature
        ));
        let elsewhere = encode(&keys.sign_deterministic("another/domain", b"msg").unwrap());
        assert!(!verify_validator(&lattice, &hash, b"msg", &elsewhere));
        assert!(!verify_validator(&lattice, &hash, b"msg", b"garbage"));
        assert_ne!(
            validator_id(&lattice, &hash),
            validator_id(other.lattice.public(), &hash)
        );
    }
}
