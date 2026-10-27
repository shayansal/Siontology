//! Validator identities and signatures.
//!
//! Validators sign consensus headers, votes and lane batch roots with the
//! same dual post-quantum scheme accounts use: one ML-DSA-44 signature and
//! one SLH-DSA-SHA2-128f signature over the same digest, both required, so a
//! forger must break module lattices *and* SHA-256. For quick experiments the
//! devnet can instead run the consensus layer's insecure simulation MAC
//! ([`SignatureMode::InsecureFast`]); that mode is labeled everywhere it
//! appears and must never be used outside a local devnet.

use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sion_consensus::{
    Authenticator, InsecureSimAuthenticator, InsecureSimVerifier, SignatureVerifier,
};
use sion_core::encode::encode;
use sion_core::hash::hash_domain;
use sion_core::ids::ValidatorId;
use sion_crypto::{DualKeypair, PublicKey, Scheme};
pub use sion_light::header::{validator_id, ValidatorSet, VALIDATOR_DOMAIN};

/// How validators sign.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SignatureMode {
    /// ML-DSA-44 and SLH-DSA-SHA2-128f, both required (the real scheme).
    DualPostQuantum,
    /// **Insecure, simulation only**: a keyed hash whose verifier can forge.
    /// For load experiments on one machine.
    InsecureFast,
}

/// A validator's keys.
#[derive(Clone)]
pub struct ValidatorKeys {
    /// The validator's identifier.
    pub id: ValidatorId,
    /// The dual keypair.
    pub keys: Arc<DualKeypair>,
    /// The insecure simulation key (used only in [`SignatureMode::InsecureFast`]).
    sim: InsecureSimAuthenticator,
}

impl fmt::Debug for ValidatorKeys {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ValidatorKeys({})", self.id.short())
    }
}

impl ValidatorKeys {
    /// Deterministic devnet keys for validator `index` of chain `chain`.
    pub fn devnet(chain: &str, index: usize) -> Self {
        let seed = hash_domain(
            "sion/devnet/validator-seed",
            format!("{chain}/{index}").as_bytes(),
        );
        let keys = DualKeypair::from_seed(&seed.0, Scheme::SlhDsaSha2_128f);
        // The identifier is always the hash of the public keys, as the state
        // registers it; only the signatures differ between the modes.
        let id = validator_id(keys.lattice.public(), keys.hash.public());
        let sim = InsecureSimAuthenticator::with_identity(
            id,
            hash_domain("sion/devnet/sim-secret", &seed.0).0,
        );
        ValidatorKeys {
            id,
            keys: Arc::new(keys),
            sim,
        }
    }

    /// The public keys.
    pub fn public(&self) -> (PublicKey, PublicKey) {
        (
            self.keys.lattice.public().clone(),
            self.keys.hash.public().clone(),
        )
    }

    /// A dual post-quantum validator signature over `message`, whatever the
    /// devnet's signature mode: proofs of key possession and equivocation
    /// evidence are checked by the state against the registered keys.
    pub fn sign_dual(&self, message: &[u8]) -> Vec<u8> {
        encode(
            &self
                .keys
                .sign_deterministic(VALIDATOR_DOMAIN, message)
                .expect("signing a digest cannot fail"),
        )
    }

    /// The authenticator for `mode`.
    pub fn authenticator(&self, mode: SignatureMode) -> NodeAuthenticator {
        match mode {
            SignatureMode::DualPostQuantum => NodeAuthenticator::Dual {
                id: self.id,
                keys: self.keys.clone(),
            },
            SignatureMode::InsecureFast => NodeAuthenticator::InsecureFast(self.sim.clone()),
        }
    }
}

/// Signs as one validator.
#[derive(Clone)]
pub enum NodeAuthenticator {
    /// Dual post-quantum signatures.
    Dual {
        /// The validator.
        id: ValidatorId,
        /// Its keys.
        keys: Arc<DualKeypair>,
    },
    /// **Insecure, simulation only.**
    InsecureFast(InsecureSimAuthenticator),
}

impl fmt::Debug for NodeAuthenticator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NodeAuthenticator::Dual { id, .. } => write!(f, "DualAuthenticator({})", id.short()),
            NodeAuthenticator::InsecureFast(sim) => write!(f, "INSECURE {sim:?}"),
        }
    }
}

impl Authenticator for NodeAuthenticator {
    fn validator(&self) -> ValidatorId {
        match self {
            NodeAuthenticator::Dual { id, .. } => *id,
            NodeAuthenticator::InsecureFast(sim) => sim.validator(),
        }
    }

    fn sign(&self, message: &[u8]) -> Vec<u8> {
        match self {
            NodeAuthenticator::Dual { keys, .. } => {
                let signature = keys
                    .sign_deterministic(VALIDATOR_DOMAIN, message)
                    .expect("signing a 32-byte digest cannot fail");
                encode(&signature)
            }
            NodeAuthenticator::InsecureFast(sim) => sim.sign(message),
        }
    }
}

/// Verifies any validator's signatures, remembering successful checks.
///
/// One lane batch signature covers every approval in the batch, so the same
/// (validator, digest, signature) triple arrives once per transaction; the
/// cache makes every check after the first a lookup. Each clone starts with
/// its own empty cache, as each node would.
#[derive(Default)]
pub struct NodeVerifier {
    /// Public keys (dual post-quantum mode).
    pub set: ValidatorSet,
    sim: Option<InsecureSimVerifier>,
    verified: std::sync::Mutex<std::collections::BTreeSet<sion_core::hash::Hash32>>,
}

impl Clone for NodeVerifier {
    fn clone(&self) -> Self {
        NodeVerifier {
            set: self.set.clone(),
            sim: self.sim.clone(),
            verified: std::sync::Mutex::new(std::collections::BTreeSet::new()),
        }
    }
}

/// Successful verifications remembered before the cache is cleared.
const VERIFIED_CACHE: usize = 200_000;

impl fmt::Debug for NodeVerifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "NodeVerifier({:?}{})",
            self.set,
            if self.sim.is_some() {
                ", INSECURE sim mode"
            } else {
                ""
            }
        )
    }
}

impl NodeVerifier {
    /// A verifier for `validators` under `mode`.
    pub fn new(validators: &[ValidatorKeys], mode: SignatureMode) -> Self {
        match mode {
            SignatureMode::DualPostQuantum => {
                let mut set = ValidatorSet::default();
                for v in validators {
                    let (lattice, hash) = v.public();
                    set.insert(lattice, hash);
                }
                NodeVerifier {
                    set,
                    ..NodeVerifier::default()
                }
            }
            SignatureMode::InsecureFast => {
                let mut sim = InsecureSimVerifier::default();
                for v in validators {
                    sim.register(&v.sim);
                }
                NodeVerifier {
                    sim: Some(sim),
                    ..NodeVerifier::default()
                }
            }
        }
    }
}

impl NodeVerifier {
    /// Adds the keys of every validator the state registers (dual
    /// post-quantum mode; the simulation scheme knows every devnet key from
    /// the start). Identifiers that do not derive from their keys are
    /// skipped.
    pub fn extend_from(&mut self, set: &sion_exec::validators::ValidatorSet) {
        if self.sim.is_some() {
            return;
        }
        for record in set.validators.values() {
            if !self.set.keys.contains_key(&record.id)
                && validator_id(&record.lattice, &record.hash) == record.id
            {
                self.set.insert(record.lattice.clone(), record.hash.clone());
            }
        }
    }

    /// Adds keys from a key set (the committee view's).
    pub fn extend_keys(&mut self, keys: &ValidatorSet) {
        if self.sim.is_some() {
            return;
        }
        for (id, (lattice, hash)) in &keys.keys {
            if !self.set.keys.contains_key(id) && validator_id(lattice, hash) == *id {
                self.set.insert(lattice.clone(), hash.clone());
            }
        }
    }

    /// Whether signatures by `validator` can be checked.
    pub fn knows(&self, validator: &ValidatorId) -> bool {
        match &self.sim {
            Some(sim) => sim.knows(validator),
            None => self.set.keys.contains_key(validator),
        }
    }
}

impl SignatureVerifier for NodeVerifier {
    fn verify(&self, validator: &ValidatorId, message: &[u8], signature: &[u8]) -> bool {
        if let Some(sim) = &self.sim {
            return sim.verify(validator, message, signature);
        }
        let key = sion_core::hash::hash_parts(
            "sion/node/verified",
            &[&validator.0 .0, message, signature],
        );
        if let Ok(cache) = self.verified.lock() {
            if cache.contains(&key) {
                return true;
            }
        }
        let valid = self.set.verify(validator, message, signature);
        if valid {
            if let Ok(mut cache) = self.verified.lock() {
                if cache.len() >= VERIFIED_CACHE {
                    cache.clear();
                }
                cache.insert(key);
            }
        }
        valid
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dual_signatures_need_both_halves() {
        let keys: Vec<ValidatorKeys> = (0..2).map(|i| ValidatorKeys::devnet("test", i)).collect();
        let verifier = NodeVerifier::new(&keys, SignatureMode::DualPostQuantum);
        let auth = keys[0].authenticator(SignatureMode::DualPostQuantum);
        let message = [9u8; 32];
        let signature = auth.sign(&message);
        assert!(verifier.verify(&keys[0].id, &message, &signature));
        assert!(!verifier.verify(&keys[1].id, &message, &signature));
        let mut parsed: sion_crypto::DualSignature = sion_core::encode::decode(&signature).unwrap();
        parsed.hash.bytes[3] ^= 1;
        assert!(!verifier.verify(&keys[0].id, &message, &encode(&parsed)));
    }
}
