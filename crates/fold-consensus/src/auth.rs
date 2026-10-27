//! Signature abstraction for the ordering layer.
//!
//! The protocols in this crate only need two capabilities: a validator can
//! sign a message, and anyone can verify a validator's signature. Keeping
//! them behind traits lets the devnet run thousands of simulated rounds with
//! a fast stand-in, while `fold-node` plugs in the real dual post-quantum
//! signatures (ML-DSA-44 and SLH-DSA, both required) from `fold-crypto`.
//!
//! Every message signed in this crate is first reduced to a domain-separated
//! digest by [`signing_digest`], so a vote can never be replayed as a lane
//! approval or a header signature.

use std::collections::BTreeMap;
use std::fmt;

use fold_core::{hash_parts, Hash32, Hasher, ValidatorId};

/// A validator's signing capability.
pub trait Authenticator {
    /// The validator this authenticator signs for.
    fn validator(&self) -> ValidatorId;
    /// Signs `message` (always a domain-separated digest in this crate).
    fn sign(&self, message: &[u8]) -> Vec<u8>;
}

/// Verification of any validator's signatures.
pub trait SignatureVerifier {
    /// Whether `signature` is `validator`'s signature on `message`.
    fn verify(&self, validator: &ValidatorId, message: &[u8], signature: &[u8]) -> bool;
}

impl<T: Authenticator + ?Sized> Authenticator for &T {
    fn validator(&self) -> ValidatorId {
        (**self).validator()
    }
    fn sign(&self, message: &[u8]) -> Vec<u8> {
        (**self).sign(message)
    }
}

impl<T: SignatureVerifier + ?Sized> SignatureVerifier for &T {
    fn verify(&self, validator: &ValidatorId, message: &[u8], signature: &[u8]) -> bool {
        (**self).verify(validator, message, signature)
    }
}

impl<T: SignatureVerifier + ?Sized> SignatureVerifier for std::sync::Arc<T> {
    fn verify(&self, validator: &ValidatorId, message: &[u8], signature: &[u8]) -> bool {
        (**self).verify(validator, message, signature)
    }
}

/// Reduces a message to the 32-byte digest that is actually signed.
pub fn signing_digest(domain: &str, parts: &[&[u8]]) -> Hash32 {
    hash_parts(domain, parts)
}

const SIM_MAC_DOMAIN: &str = "fold/sim/INSECURE-mac";
const SIM_PUBLIC_DOMAIN: &str = "fold/sim/INSECURE-public";

/// **Simulation only. Not a signature scheme.** A keyed SHA3 MAC whose
/// verifier knows every secret, so anyone holding the verifier can forge.
/// It exists so the simulator can run many rounds quickly; production nodes
/// use `fold-crypto`'s dual post-quantum signatures.
#[derive(Clone)]
pub struct InsecureSimAuthenticator {
    validator: ValidatorId,
    secret: [u8; 32],
}

impl fmt::Debug for InsecureSimAuthenticator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "InsecureSimAuthenticator({})", self.validator.short())
    }
}

impl InsecureSimAuthenticator {
    /// Derives a simulated validator identity from a secret.
    pub fn from_secret(secret: [u8; 32]) -> Self {
        let public = fold_core::hash_domain(SIM_PUBLIC_DOMAIN, &secret);
        InsecureSimAuthenticator {
            validator: ValidatorId::derive(&public.0),
            secret,
        }
    }

    fn mac(secret: &[u8; 32], message: &[u8]) -> Vec<u8> {
        let mut hasher = Hasher::new(SIM_MAC_DOMAIN);
        hasher.update(secret).update_part(message);
        hasher.finalize().0.to_vec()
    }
}

impl Authenticator for InsecureSimAuthenticator {
    fn validator(&self) -> ValidatorId {
        self.validator
    }

    fn sign(&self, message: &[u8]) -> Vec<u8> {
        Self::mac(&self.secret, message)
    }
}

/// **Simulation only.** Verifies [`InsecureSimAuthenticator`] MACs using a
/// registry of every validator's secret.
#[derive(Clone, Default)]
pub struct InsecureSimVerifier {
    secrets: BTreeMap<ValidatorId, [u8; 32]>,
}

impl fmt::Debug for InsecureSimVerifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "InsecureSimVerifier({} validators)", self.secrets.len())
    }
}

impl InsecureSimVerifier {
    /// Registers a simulated validator.
    pub fn register(&mut self, authenticator: &InsecureSimAuthenticator) {
        self.secrets
            .insert(authenticator.validator, authenticator.secret);
    }
}

impl SignatureVerifier for InsecureSimVerifier {
    fn verify(&self, validator: &ValidatorId, message: &[u8], signature: &[u8]) -> bool {
        match self.secrets.get(validator) {
            Some(secret) => InsecureSimAuthenticator::mac(secret, message) == signature,
            None => false,
        }
    }
}

/// **Simulation only.** Creates `count` deterministic simulated validators
/// and a verifier that knows all of them.
pub fn insecure_sim_keyring(
    count: usize,
    seed: u64,
) -> (Vec<InsecureSimAuthenticator>, InsecureSimVerifier) {
    let mut verifier = InsecureSimVerifier::default();
    let authenticators: Vec<InsecureSimAuthenticator> = (0..count)
        .map(|i| {
            let secret = hash_parts(
                "fold/sim/INSECURE-secret",
                &[&seed.to_be_bytes(), &(i as u64).to_be_bytes()],
            );
            let authenticator = InsecureSimAuthenticator::from_secret(secret.0);
            verifier.register(&authenticator);
            authenticator
        })
        .collect();
    (authenticators, verifier)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sim_signatures_verify_and_reject_tampering() {
        let (keys, verifier) = insecure_sim_keyring(3, 7);
        let message = signing_digest("fold/test", &[b"hello"]);
        let signature = keys[0].sign(&message.0);
        assert!(verifier.verify(&keys[0].validator(), &message.0, &signature));
        assert!(!verifier.verify(&keys[1].validator(), &message.0, &signature));
        let other = signing_digest("fold/test", &[b"hellp"]);
        assert!(!verifier.verify(&keys[0].validator(), &other.0, &signature));
        let mut bad = signature.clone();
        bad[0] ^= 1;
        assert!(!verifier.verify(&keys[0].validator(), &message.0, &bad));
    }

    #[test]
    fn keyring_is_deterministic_and_distinct() {
        let (a, _) = insecure_sim_keyring(4, 1);
        let (b, _) = insecure_sim_keyring(4, 1);
        let (c, _) = insecure_sim_keyring(4, 2);
        assert_eq!(a[2].validator(), b[2].validator());
        assert_ne!(a[2].validator(), c[2].validator());
        let ids: std::collections::BTreeSet<_> = a.iter().map(|k| k.validator()).collect();
        assert_eq!(ids.len(), 4);
    }
}
