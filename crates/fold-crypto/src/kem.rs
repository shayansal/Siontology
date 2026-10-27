//! ML-KEM-768 key encapsulation and single-recipient sealed envelopes.
//!
//! Data harvested today and decrypted by a future quantum computer is the
//! main quantum risk to confidentiality, so every encrypted payload in Fold
//! (viewing keys, Travel Rule data, sealed intents) uses ML-KEM to agree on
//! a key and ChaCha20-Poly1305 to encrypt.

use std::fmt;

use borsh::{BorshDeserialize, BorshSerialize};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use fips203::ml_kem_768;
use fips203::traits::{Decaps, Encaps, KeyGen, SerDes};
use fold_core::hash::{hash_parts, Hash32};
use rand_core::CryptoRngCore;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::error::CryptoError;

/// An ML-KEM-768 encapsulation (public) key.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct KemPublicKey {
    /// The 1,184-byte encapsulation key.
    #[serde(with = "crate::keys::hex_bytes")]
    pub bytes: Vec<u8>,
}

impl KemPublicKey {
    /// A digest identifying this key.
    pub fn fingerprint(&self) -> Hash32 {
        hash_parts("fold/crypto/kem-fingerprint", &[&self.bytes])
    }
}

impl fmt::Debug for KemPublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "KemPublicKey({})", self.fingerprint().short())
    }
}

/// An ML-KEM-768 keypair.
#[derive(Clone)]
pub struct KemKeypair {
    public: KemPublicKey,
    secret: Box<ml_kem_768::DecapsKey>,
}

impl fmt::Debug for KemKeypair {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "KemKeypair({:?}, secret redacted)", self.public)
    }
}

impl KemKeypair {
    /// Derives a keypair from a 32-byte seed.
    pub fn from_seed(seed: &[u8; 32]) -> KemKeypair {
        let d = hash_parts("fold/crypto/kem-seed", &[b"d", seed]).0;
        let z = hash_parts("fold/crypto/kem-seed", &[b"z", seed]).0;
        let (ek, dk) = ml_kem_768::KG::keygen_from_seed(d, z);
        KemKeypair {
            public: KemPublicKey {
                bytes: ek.into_bytes().to_vec(),
            },
            secret: Box::new(dk),
        }
    }

    /// Generates a keypair from the given generator.
    pub fn generate_with_rng(rng: &mut impl CryptoRngCore) -> KemKeypair {
        let mut seed = [0u8; 32];
        rng.fill_bytes(&mut seed);
        Self::from_seed(&seed)
    }

    /// The public key.
    pub fn public(&self) -> &KemPublicKey {
        &self.public
    }

    fn decapsulate(&self, ciphertext: &[u8]) -> Result<Zeroizing<[u8; 32]>, CryptoError> {
        let array: [u8; ml_kem_768::CT_LEN] = ciphertext.try_into().map_err(|_| CryptoError::Decryption)?;
        let ct = ml_kem_768::CipherText::try_from_bytes(array).map_err(|_| CryptoError::Decryption)?;
        let shared = self.secret.try_decaps(&ct).map_err(|_| CryptoError::Decryption)?;
        Ok(Zeroizing::new(shared.into_bytes()))
    }

    /// Opens an envelope sealed to this key.
    pub fn open(&self, domain: &str, aad: &[u8], envelope: &SealedEnvelope) -> Result<Vec<u8>, CryptoError> {
        let shared = self.decapsulate(&envelope.kem_ciphertext)?;
        let key = derive_key(domain, &shared, &envelope.kem_ciphertext);
        aead_open(&key, &envelope.nonce, &associated(domain, aad), &envelope.ciphertext)
    }
}

/// A payload sealed to one ML-KEM public key.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct SealedEnvelope {
    /// The ML-KEM ciphertext (1,088 bytes).
    #[serde(with = "crate::keys::hex_bytes")]
    pub kem_ciphertext: Vec<u8>,
    /// AEAD nonce.
    pub nonce: [u8; 12],
    /// AEAD ciphertext with tag.
    #[serde(with = "crate::keys::hex_bytes")]
    pub ciphertext: Vec<u8>,
}

fn derive_key(domain: &str, shared: &[u8; 32], kem_ciphertext: &[u8]) -> Zeroizing<[u8; 32]> {
    Zeroizing::new(hash_parts("fold/crypto/seal-key", &[domain.as_bytes(), shared, kem_ciphertext]).0)
}

fn associated(domain: &str, aad: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(domain.len() + aad.len() + 8);
    out.extend_from_slice(&(domain.len() as u64).to_be_bytes());
    out.extend_from_slice(domain.as_bytes());
    out.extend_from_slice(aad);
    out
}

pub(crate) fn aead_seal(key: &[u8; 32], nonce: &[u8; 12], aad: &[u8], plaintext: &[u8]) -> Vec<u8> {
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key));
    cipher
        .encrypt(Nonce::from_slice(nonce), Payload { msg: plaintext, aad })
        .expect("encryption into a Vec cannot fail")
}

pub(crate) fn aead_open(key: &[u8; 32], nonce: &[u8; 12], aad: &[u8], ciphertext: &[u8]) -> Result<Vec<u8>, CryptoError> {
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key));
    cipher
        .decrypt(Nonce::from_slice(nonce), Payload { msg: ciphertext, aad })
        .map_err(|_| CryptoError::Decryption)
}

/// Seals `plaintext` to `recipient`, binding `domain` and `aad`.
pub fn seal(
    recipient: &KemPublicKey,
    domain: &str,
    aad: &[u8],
    plaintext: &[u8],
    rng: &mut impl CryptoRngCore,
) -> Result<SealedEnvelope, CryptoError> {
    let array: [u8; ml_kem_768::EK_LEN] = recipient
        .bytes
        .as_slice()
        .try_into()
        .map_err(|_| CryptoError::Library("bad ML-KEM encapsulation key length"))?;
    let ek = ml_kem_768::EncapsKey::try_from_bytes(array).map_err(CryptoError::Library)?;
    let (shared, ct) = ek.try_encaps_with_rng(rng).map_err(CryptoError::Library)?;
    let shared = Zeroizing::new(shared.into_bytes());
    let kem_ciphertext = ct.into_bytes().to_vec();
    let key = derive_key(domain, &shared, &kem_ciphertext);
    let mut nonce = [0u8; 12];
    rng.fill_bytes(&mut nonce);
    let ciphertext = aead_seal(&key, &nonce, &associated(domain, aad), plaintext);
    Ok(SealedEnvelope {
        kem_ciphertext,
        nonce,
        ciphertext,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seal_and_open() {
        let recipient = KemKeypair::from_seed(&[1; 32]);
        let mut rng = fold_core::rng::seeded(1);
        let envelope = seal(recipient.public(), "fold/test/seal", b"aad", b"originator: ACME", &mut rng).unwrap();
        assert_eq!(envelope.kem_ciphertext.len(), 1088);
        let opened = recipient.open("fold/test/seal", b"aad", &envelope).unwrap();
        assert_eq!(opened, b"originator: ACME");
    }

    #[test]
    fn wrong_key_domain_aad_or_tamper_fails() {
        let recipient = KemKeypair::from_seed(&[1; 32]);
        let other = KemKeypair::from_seed(&[2; 32]);
        let mut rng = fold_core::rng::seeded(2);
        let envelope = seal(recipient.public(), "fold/test/seal", b"aad", b"secret", &mut rng).unwrap();
        assert!(other.open("fold/test/seal", b"aad", &envelope).is_err());
        assert!(recipient.open("fold/test/other", b"aad", &envelope).is_err());
        assert!(recipient.open("fold/test/seal", b"bad", &envelope).is_err());
        let mut tampered = envelope.clone();
        tampered.ciphertext[0] ^= 1;
        assert!(recipient.open("fold/test/seal", b"aad", &tampered).is_err());
    }
}
