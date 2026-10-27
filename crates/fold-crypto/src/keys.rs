//! Keypairs, public keys and signatures for the supported schemes.

use std::fmt;

use borsh::{BorshDeserialize, BorshSerialize};
use fips204::ml_dsa_44;
use fips204::traits::{
    KeyGen as KeyGen204, SerDes as SerDes204, Signer as Signer204, Verifier as Verifier204,
};
use fips205::traits::{
    KeyGen as KeyGen205, SerDes as SerDes205, Signer as Signer205, Verifier as Verifier205,
};
use fips205::{slh_dsa_sha2_128f, slh_dsa_sha2_128s};
use fold_core::hash::{hash_parts, Hash32};
use rand_chacha::ChaCha20Rng;
use rand_core::{CryptoRngCore, OsRng, RngCore, SeedableRng};
use serde::{Deserialize, Serialize};

use crate::error::CryptoError;
use crate::scheme::Scheme;

/// A public key tagged with its scheme.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct PublicKey {
    /// The scheme.
    pub scheme: Scheme,
    /// The encoded key.
    #[serde(with = "hex_bytes")]
    pub bytes: Vec<u8>,
}

impl PublicKey {
    /// A digest identifying this key.
    pub fn fingerprint(&self) -> Hash32 {
        hash_parts("fold/crypto/key-fingerprint", &[self.scheme.name().as_bytes(), &self.bytes])
    }
}

impl fmt::Debug for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PublicKey({}, {})", self.scheme.name(), self.fingerprint().short())
    }
}

/// A signature tagged with its scheme.
#[derive(Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct Signature {
    /// The scheme.
    pub scheme: Scheme,
    /// The encoded signature.
    #[serde(with = "hex_bytes")]
    pub bytes: Vec<u8>,
}

impl fmt::Debug for Signature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Signature({}, {} bytes)", self.scheme.name(), self.bytes.len())
    }
}

#[derive(Clone)]
enum SecretKey {
    MlDsa44(Box<ml_dsa_44::PrivateKey>),
    SlhSha2_128s(Box<slh_dsa_sha2_128s::PrivateKey>),
    SlhSha2_128f(Box<slh_dsa_sha2_128f::PrivateKey>),
}

/// A keypair. The secret key is zeroized on drop by the underlying library
/// and never printed.
#[derive(Clone)]
pub struct Keypair {
    public: PublicKey,
    secret: SecretKey,
}

impl fmt::Debug for Keypair {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Keypair({:?}, secret redacted)", self.public)
    }
}

fn context(domain: &str) -> Result<&[u8], CryptoError> {
    if domain.len() > 255 {
        Err(CryptoError::DomainTooLong)
    } else {
        Ok(domain.as_bytes())
    }
}

fn seed_part(seed: &[u8; 32], label: &str) -> [u8; 32] {
    hash_parts("fold/crypto/seed", &[label.as_bytes(), seed]).0
}

fn seed16(seed: &[u8; 32], label: &str) -> [u8; 16] {
    let mut out = [0u8; 16];
    out.copy_from_slice(&seed_part(seed, label)[..16]);
    out
}

impl Keypair {
    /// Derives a keypair deterministically from a 32-byte seed. Use a seed
    /// with full entropy; [`Keypair::generate`] does this from the OS.
    pub fn from_seed(scheme: Scheme, seed: &[u8; 32]) -> Keypair {
        match scheme {
            Scheme::MlDsa44 => {
                let xi = seed_part(seed, "ml-dsa-44");
                let (pk, sk) = ml_dsa_44::KG::keygen_from_seed(&xi);
                Keypair {
                    public: PublicKey {
                        scheme,
                        bytes: pk.into_bytes().to_vec(),
                    },
                    secret: SecretKey::MlDsa44(Box::new(sk)),
                }
            }
            Scheme::SlhDsaSha2_128s => {
                let (pk, sk) = slh_dsa_sha2_128s::KG::keygen_from_seed(
                    &seed16(seed, "slh/sk-seed"),
                    &seed16(seed, "slh/sk-prf"),
                    &seed16(seed, "slh/pk-seed"),
                );
                Keypair {
                    public: PublicKey {
                        scheme,
                        bytes: pk.into_bytes().to_vec(),
                    },
                    secret: SecretKey::SlhSha2_128s(Box::new(sk)),
                }
            }
            Scheme::SlhDsaSha2_128f => {
                let (pk, sk) = slh_dsa_sha2_128f::KG::keygen_from_seed(
                    &seed16(seed, "slh/sk-seed"),
                    &seed16(seed, "slh/sk-prf"),
                    &seed16(seed, "slh/pk-seed"),
                );
                Keypair {
                    public: PublicKey {
                        scheme,
                        bytes: pk.into_bytes().to_vec(),
                    },
                    secret: SecretKey::SlhSha2_128f(Box::new(sk)),
                }
            }
        }
    }

    /// Generates a keypair from the operating system's random generator.
    pub fn generate(scheme: Scheme) -> Keypair {
        Self::generate_with_rng(scheme, &mut OsRng)
    }

    /// Generates a keypair from the given generator.
    pub fn generate_with_rng(scheme: Scheme, rng: &mut impl CryptoRngCore) -> Keypair {
        let mut seed = [0u8; 32];
        rng.fill_bytes(&mut seed);
        Self::from_seed(scheme, &seed)
    }

    /// The public key.
    pub fn public(&self) -> &PublicKey {
        &self.public
    }

    /// The scheme.
    pub fn scheme(&self) -> Scheme {
        self.public.scheme
    }

    /// Signs with hedged randomness from the OS (the recommended mode:
    /// randomness protects against fault and side-channel attacks).
    pub fn sign(&self, domain: &str, message: &[u8]) -> Result<Signature, CryptoError> {
        self.sign_with_rng(&mut OsRng, domain, message)
    }

    /// Signs deterministically. Used by simulations so runs are reproducible.
    pub fn sign_deterministic(&self, domain: &str, message: &[u8]) -> Result<Signature, CryptoError> {
        let ctx = context(domain)?;
        let bytes = match &self.secret {
            SecretKey::MlDsa44(sk) => sk
                .try_sign_with_seed(&[0u8; 32], message, ctx)
                .map_err(CryptoError::Library)?
                .to_vec(),
            SecretKey::SlhSha2_128s(sk) => {
                let mut rng = ChaCha20Rng::from_seed([0u8; 32]);
                sk.try_sign_with_rng(&mut rng, message, ctx, false)
                    .map_err(CryptoError::Library)?
                    .to_vec()
            }
            SecretKey::SlhSha2_128f(sk) => {
                let mut rng = ChaCha20Rng::from_seed([0u8; 32]);
                sk.try_sign_with_rng(&mut rng, message, ctx, false)
                    .map_err(CryptoError::Library)?
                    .to_vec()
            }
        };
        Ok(Signature {
            scheme: self.scheme(),
            bytes,
        })
    }

    /// Signs with hedged randomness from the given generator.
    pub fn sign_with_rng(
        &self,
        rng: &mut impl CryptoRngCore,
        domain: &str,
        message: &[u8],
    ) -> Result<Signature, CryptoError> {
        let ctx = context(domain)?;
        let bytes = match &self.secret {
            SecretKey::MlDsa44(sk) => sk
                .try_sign_with_rng(rng, message, ctx)
                .map_err(CryptoError::Library)?
                .to_vec(),
            SecretKey::SlhSha2_128s(sk) => sk
                .try_sign_with_rng(rng, message, ctx, true)
                .map_err(CryptoError::Library)?
                .to_vec(),
            SecretKey::SlhSha2_128f(sk) => sk
                .try_sign_with_rng(rng, message, ctx, true)
                .map_err(CryptoError::Library)?
                .to_vec(),
        };
        Ok(Signature {
            scheme: self.scheme(),
            bytes,
        })
    }
}

fn fixed<const N: usize>(scheme: Scheme, bytes: &[u8]) -> Result<[u8; N], CryptoError> {
    bytes.try_into().map_err(|_| CryptoError::BadLength {
        scheme,
        expected: N,
        actual: bytes.len(),
    })
}

/// Verifies `signature` over `message` in `domain` under `key`.
pub fn verify(key: &PublicKey, domain: &str, message: &[u8], signature: &Signature) -> Result<(), CryptoError> {
    if key.scheme != signature.scheme {
        return Err(CryptoError::SchemeMismatch {
            key: key.scheme,
            signature: signature.scheme,
        });
    }
    let ctx = context(domain)?;
    let valid = match key.scheme {
        Scheme::MlDsa44 => {
            let pk = ml_dsa_44::PublicKey::try_from_bytes(fixed(key.scheme, &key.bytes)?)
                .map_err(CryptoError::Library)?;
            let sig: [u8; ml_dsa_44::SIG_LEN] = fixed(signature.scheme, &signature.bytes)?;
            pk.verify(message, &sig, ctx)
        }
        Scheme::SlhDsaSha2_128s => {
            let pk = slh_dsa_sha2_128s::PublicKey::try_from_bytes(fixed(key.scheme, &key.bytes)?)
                .map_err(CryptoError::Library)?;
            let sig: [u8; slh_dsa_sha2_128s::SIG_LEN] = fixed(signature.scheme, &signature.bytes)?;
            pk.verify(message, &sig, ctx)
        }
        Scheme::SlhDsaSha2_128f => {
            let pk = slh_dsa_sha2_128f::PublicKey::try_from_bytes(fixed(key.scheme, &key.bytes)?)
                .map_err(CryptoError::Library)?;
            let sig: [u8; slh_dsa_sha2_128f::SIG_LEN] = fixed(signature.scheme, &signature.bytes)?;
            pk.verify(message, &sig, ctx)
        }
    };
    if valid {
        Ok(())
    } else {
        Err(CryptoError::InvalidSignature)
    }
}

/// Hex encoding for byte vectors in JSON.
pub(crate) mod hex_bytes {
    use serde::{Deserialize, Deserializer, Serializer};

    pub(crate) fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&hex::encode(bytes))
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(deserializer)?;
        hex::decode(text).map_err(serde::de::Error::custom)
    }
}

/// Fills a fresh random 32-byte seed from the OS.
pub fn random_seed() -> [u8; 32] {
    let mut seed = [0u8; 32];
    OsRng.fill_bytes(&mut seed);
    seed
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOMAIN: &str = "fold/test/sign";

    #[test]
    fn every_scheme_signs_and_verifies() {
        for scheme in Scheme::ALL {
            let keypair = Keypair::from_seed(scheme, &[7u8; 32]);
            assert_eq!(keypair.public().bytes.len(), scheme.public_key_len());
            let signature = keypair.sign_deterministic(DOMAIN, b"pay 25,000").unwrap();
            assert_eq!(signature.bytes.len(), scheme.signature_len());
            verify(keypair.public(), DOMAIN, b"pay 25,000", &signature).unwrap();
            // Tampered message, wrong domain and flipped bytes all fail.
            assert!(verify(keypair.public(), DOMAIN, b"pay 26,000", &signature).is_err());
            assert!(verify(keypair.public(), "fold/test/other", b"pay 25,000", &signature).is_err());
            let mut flipped = signature.clone();
            flipped.bytes[10] ^= 1;
            assert!(verify(keypair.public(), DOMAIN, b"pay 25,000", &flipped).is_err());
        }
    }

    #[test]
    fn seeds_are_deterministic_and_distinct() {
        let a = Keypair::from_seed(Scheme::MlDsa44, &[1u8; 32]);
        let b = Keypair::from_seed(Scheme::MlDsa44, &[1u8; 32]);
        let c = Keypair::from_seed(Scheme::MlDsa44, &[2u8; 32]);
        assert_eq!(a.public(), b.public());
        assert_ne!(a.public(), c.public());
    }

    #[test]
    fn hedged_signatures_verify() {
        let keypair = Keypair::from_seed(Scheme::MlDsa44, &[3u8; 32]);
        let mut rng = fold_core::rng::seeded(9);
        let signature = keypair.sign_with_rng(&mut rng, DOMAIN, b"m").unwrap();
        verify(keypair.public(), DOMAIN, b"m", &signature).unwrap();
        let os = keypair.sign(DOMAIN, b"m").unwrap();
        verify(keypair.public(), DOMAIN, b"m", &os).unwrap();
    }

    #[test]
    fn mismatched_schemes_are_rejected() {
        let lattice = Keypair::from_seed(Scheme::MlDsa44, &[4u8; 32]);
        let hash = Keypair::from_seed(Scheme::SlhDsaSha2_128f, &[4u8; 32]);
        let signature = hash.sign_deterministic(DOMAIN, b"m").unwrap();
        assert!(matches!(
            verify(lattice.public(), DOMAIN, b"m", &signature),
            Err(CryptoError::SchemeMismatch { .. })
        ));
    }

    #[test]
    fn short_keys_are_rejected() {
        let mut key = Keypair::from_seed(Scheme::MlDsa44, &[5u8; 32]).public().clone();
        key.bytes.pop();
        let signature = Signature {
            scheme: Scheme::MlDsa44,
            bytes: vec![0; Scheme::MlDsa44.signature_len()],
        };
        assert!(matches!(
            verify(&key, DOMAIN, b"m", &signature),
            Err(CryptoError::BadLength { .. })
        ));
    }

    #[test]
    fn json_round_trip() {
        let keypair = Keypair::from_seed(Scheme::SlhDsaSha2_128f, &[6u8; 32]);
        let json = serde_json::to_string(keypair.public()).unwrap();
        let back: PublicKey = serde_json::from_str(&json).unwrap();
        assert_eq!(&back, keypair.public());
    }
}
