//! Account authorization policies.
//!
//! An account is identified by the hash of its genesis policy, so its ID
//! survives key rotation and scheme replacement: a broken scheme is swapped
//! in place, not migrated around. The default policy demands one valid
//! signature from each of two unrelated families, so forging an
//! authorization requires breaking both (trust formula `lattice ∨ hash`).

use std::collections::BTreeSet;

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use sion_core::encode::encode;
use sion_core::ids::AccountId;

use crate::error::CryptoError;
use crate::keys::{verify, PublicKey, Signature};
use crate::scheme::Family;

/// Who may authorize actions for an account.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct AuthPolicy {
    /// Registered keys, addressed by index in authorizations.
    pub keys: Vec<PublicKey>,
    /// Minimum number of valid signatures.
    pub min_signatures: u8,
    /// Minimum number of distinct families among the valid signatures.
    pub min_families: u8,
}

/// One signature in an authorization, naming the key that made it.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct KeySignature {
    /// Index into [`AuthPolicy::keys`].
    pub key_index: u8,
    /// The signature.
    pub signature: Signature,
}

/// The signatures presented for one action.
#[derive(
    Clone, Debug, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize,
)]
pub struct Authorization {
    /// Signatures, each over the same domain-separated message.
    pub signatures: Vec<KeySignature>,
}

impl Authorization {
    /// Total signature bytes, for bandwidth accounting.
    pub fn signature_bytes(&self) -> usize {
        self.signatures
            .iter()
            .map(|s| s.signature.bytes.len())
            .sum()
    }
}

impl AuthPolicy {
    /// The default dual-origin policy: one lattice and one hash-based key,
    /// both required.
    pub fn dual(lattice: PublicKey, hash: PublicKey) -> AuthPolicy {
        AuthPolicy {
            keys: vec![lattice, hash],
            min_signatures: 2,
            min_families: 2,
        }
    }

    /// Checks that the policy is satisfiable and meets the dual-origin floor.
    pub fn validate(&self) -> Result<(), CryptoError> {
        if self.keys.is_empty() || self.keys.len() > usize::from(u8::MAX) {
            return Err(CryptoError::InvalidPolicy(
                "between 1 and 255 keys required",
            ));
        }
        if self.min_signatures == 0 || usize::from(self.min_signatures) > self.keys.len() {
            return Err(CryptoError::InvalidPolicy("unsatisfiable signature count"));
        }
        let families: BTreeSet<Family> = self.keys.iter().map(|k| k.scheme.family()).collect();
        if self.min_families < 2 {
            return Err(CryptoError::InvalidPolicy(
                "policies must require at least two unrelated families",
            ));
        }
        if usize::from(self.min_families) > families.len() {
            return Err(CryptoError::InvalidPolicy("unsatisfiable family count"));
        }
        let unique: BTreeSet<&Vec<u8>> = self.keys.iter().map(|k| &k.bytes).collect();
        if unique.len() != self.keys.len() {
            return Err(CryptoError::InvalidPolicy("duplicate keys"));
        }
        Ok(())
    }

    /// The account ID this policy creates when used as a genesis policy.
    pub fn account_id(&self) -> AccountId {
        AccountId::derive(&encode(self))
    }

    /// Families of the registered keys.
    pub fn families(&self) -> BTreeSet<Family> {
        self.keys.iter().map(|k| k.scheme.family()).collect()
    }

    /// Verifies an authorization over `message` in `domain`. Every presented
    /// signature must be valid and from a distinct key; the valid set must
    /// meet both thresholds.
    pub fn verify(
        &self,
        domain: &str,
        message: &[u8],
        authorization: &Authorization,
    ) -> Result<(), CryptoError> {
        self.validate()?;
        let mut used = BTreeSet::new();
        let mut families = BTreeSet::new();
        for entry in &authorization.signatures {
            let index = usize::from(entry.key_index);
            let key = self
                .keys
                .get(index)
                .ok_or_else(|| CryptoError::Unauthorized(format!("no key at index {index}")))?;
            if !used.insert(index) {
                return Err(CryptoError::Unauthorized(format!("key {index} used twice")));
            }
            verify(key, domain, message, &entry.signature)
                .map_err(|e| CryptoError::Unauthorized(format!("key {index}: {e}")))?;
            families.insert(key.scheme.family());
        }
        if used.len() < usize::from(self.min_signatures) {
            return Err(CryptoError::Unauthorized(format!(
                "{} signatures, policy requires {}",
                used.len(),
                self.min_signatures
            )));
        }
        if families.len() < usize::from(self.min_families) {
            return Err(CryptoError::Unauthorized(format!(
                "{} families, policy requires {}",
                families.len(),
                self.min_families
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dual::DualKeypair;
    use crate::keys::Keypair;
    use crate::scheme::Scheme;

    const DOMAIN: &str = "sion/test/policy";

    fn keys(seed: u8) -> DualKeypair {
        DualKeypair::from_seed(&[seed; 32], Scheme::SlhDsaSha2_128f)
    }

    #[test]
    fn dual_policy_accepts_both_and_rejects_one() {
        let k = keys(1);
        let policy = k.policy();
        policy.validate().unwrap();
        let auth = k.authorize(DOMAIN, b"tx").unwrap();
        policy.verify(DOMAIN, b"tx", &auth).unwrap();

        let mut only_lattice = auth.clone();
        only_lattice.signatures.truncate(1);
        assert!(policy.verify(DOMAIN, b"tx", &only_lattice).is_err());

        let mut duplicate = auth.clone();
        duplicate.signatures[1] = duplicate.signatures[0].clone();
        assert!(policy.verify(DOMAIN, b"tx", &duplicate).is_err());

        assert!(policy.verify(DOMAIN, b"other", &auth).is_err());
    }

    #[test]
    fn single_family_policies_are_invalid() {
        let a = Keypair::from_seed(Scheme::MlDsa44, &[1; 32]);
        let b = Keypair::from_seed(Scheme::MlDsa44, &[2; 32]);
        let policy = AuthPolicy {
            keys: vec![a.public().clone(), b.public().clone()],
            min_signatures: 2,
            min_families: 2,
        };
        assert!(policy.validate().is_err());
        let weak = AuthPolicy {
            min_families: 1,
            ..policy
        };
        assert!(weak.validate().is_err());
    }

    #[test]
    fn account_id_is_stable_and_policy_specific() {
        let p1 = keys(1).policy();
        let p2 = keys(2).policy();
        assert_eq!(p1.account_id(), keys(1).policy().account_id());
        assert_ne!(p1.account_id(), p2.account_id());
    }

    #[test]
    fn two_of_three_with_backup_key() {
        // A recovery-style policy: lattice key, hash key and a backup hash key;
        // any two signatures from two families.
        let main = keys(3);
        let backup = Keypair::from_seed(Scheme::SlhDsaSha2_128f, &[4; 32]);
        let policy = AuthPolicy {
            keys: vec![
                main.lattice.public().clone(),
                main.hash.public().clone(),
                backup.public().clone(),
            ],
            min_signatures: 2,
            min_families: 2,
        };
        policy.validate().unwrap();
        let auth = Authorization {
            signatures: vec![
                KeySignature {
                    key_index: 0,
                    signature: main.lattice.sign_deterministic(DOMAIN, b"m").unwrap(),
                },
                KeySignature {
                    key_index: 2,
                    signature: backup.sign_deterministic(DOMAIN, b"m").unwrap(),
                },
            ],
        };
        policy.verify(DOMAIN, b"m", &auth).unwrap();
        // Two hash-based signatures are two signatures but one family.
        let hash_only = Authorization {
            signatures: vec![
                KeySignature {
                    key_index: 1,
                    signature: main.hash.sign_deterministic(DOMAIN, b"m").unwrap(),
                },
                KeySignature {
                    key_index: 2,
                    signature: backup.sign_deterministic(DOMAIN, b"m").unwrap(),
                },
            ],
        };
        assert!(policy.verify(DOMAIN, b"m", &hash_only).is_err());
    }
}
