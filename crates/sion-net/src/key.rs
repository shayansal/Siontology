//! Network keys: ML-DSA-44 key pairs that authenticate a node's transport.
//!
//! A validator's network key is separate from its dual consensus keys. It
//! lives in the TLS stack, so stealing it lets an attacker impersonate the
//! node at the transport layer (delay, drop, eclipse) but not sign a single
//! consensus message, which needs both of the validator's signatures. A
//! [`crate::record::SignedRecord`], signed with the dual keys, binds the
//! network key to the validator.
//!
//! The key pair is held by aws-lc-rs, which signs the TLS 1.3
//! CertificateVerify; it is derived from a 32-byte seed by FIPS 204's
//! seeded key generation, so the same seed gives the same key in any
//! implementation of the standard (the workspace's pure-Rust `fips204`
//! verifies its signatures; see the tests).

use std::fmt;
use std::sync::Arc;

use aws_lc_rs::signature::{KeyPair, PqdsaKeyPair, ML_DSA_44_SIGNING};
use borsh::{BorshDeserialize, BorshSerialize};
use rustls::pki_types::alg_id;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sion_core::hash::{hash_domain, Hash32};
use zeroize::Zeroizing;

use crate::error::NetError;
use crate::id::NodeId;

/// Bytes in an ML-DSA-44 public key (FIPS 204).
pub const NETWORK_KEY_BYTES: usize = 1_312;

/// Bytes in an ML-DSA-44 signature (FIPS 204).
pub const NETWORK_SIGNATURE_BYTES: usize = 2_420;

/// A node's ML-DSA-44 public network key.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, BorshSerialize, BorshDeserialize)]
pub struct NetworkPublicKey(pub Vec<u8>);

impl NetworkPublicKey {
    /// Checks the key's length.
    pub fn check(&self) -> Result<(), NetError> {
        if self.0.len() == NETWORK_KEY_BYTES {
            Ok(())
        } else {
            Err(NetError::Key(format!(
                "a network key has {NETWORK_KEY_BYTES} bytes, not {}",
                self.0.len()
            )))
        }
    }

    /// The digest that names a node other than a validator by its key.
    pub fn digest(&self) -> Hash32 {
        hash_domain("sion/net/node", &self.0)
    }

    /// The identity of a non-validator node with this key.
    pub fn node_id(&self) -> NodeId {
        NodeId::Node(self.digest())
    }

    /// The key as a DER SubjectPublicKeyInfo with the ML-DSA-44 algorithm
    /// identifier (RFC 9881): what a TLS raw public key carries.
    pub fn spki(&self) -> Vec<u8> {
        rustls::sign::public_key_to_spki(&alg_id::ML_DSA_44, &self.0)
            .as_ref()
            .to_vec()
    }
}

impl fmt::Debug for NetworkPublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "NetworkPublicKey({})", self.digest().short())
    }
}

/// Hex in JSON.
impl Serialize for NetworkPublicKey {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&hex::encode(&self.0))
    }
}

impl<'de> Deserialize<'de> for NetworkPublicKey {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = <String as Deserialize>::deserialize(deserializer)?;
        hex::decode(text.trim())
            .map(NetworkPublicKey)
            .map_err(serde::de::Error::custom)
    }
}

/// A node's network key pair. The seed is zeroized on drop and never
/// printed.
#[derive(Clone)]
pub struct NetworkKey {
    seed: Zeroizing<[u8; 32]>,
    pair: Arc<PqdsaKeyPair>,
    public: NetworkPublicKey,
}

impl fmt::Debug for NetworkKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "NetworkKey({:?}, secret redacted)", self.public)
    }
}

impl NetworkKey {
    /// A fresh key from the operating system's generator.
    pub fn generate() -> NetworkKey {
        NetworkKey::from_seed(&sion_crypto::keys::random_seed())
    }

    /// The key derived from `seed` (use a seed with full entropy).
    pub fn from_seed(seed: &[u8; 32]) -> NetworkKey {
        let xi = hash_domain("sion/net/network-key", seed);
        let pair = PqdsaKeyPair::from_seed(&ML_DSA_44_SIGNING, &xi.0)
            .expect("a 32-byte seed always yields an ML-DSA-44 key");
        let public = NetworkPublicKey(pair.public_key().as_ref().to_vec());
        NetworkKey {
            seed: Zeroizing::new(*seed),
            pair: Arc::new(pair),
            public,
        }
    }

    /// The seed, to store the key.
    pub fn seed(&self) -> [u8; 32] {
        *self.seed
    }

    /// The public key.
    pub fn public(&self) -> &NetworkPublicKey {
        &self.public
    }

    /// The private key as PKCS#8 (seed form), for the TLS stack.
    pub(crate) fn pkcs8(&self) -> Result<Zeroizing<Vec<u8>>, NetError> {
        self.pair
            .to_pkcs8v1()
            .map(|document| Zeroizing::new(document.as_ref().to_vec()))
            .map_err(|_| NetError::Key("the network key does not serialize".into()))
    }

    /// An ML-DSA-44 signature over `message` with the empty context, as TLS
    /// makes them.
    pub fn sign(&self, message: &[u8]) -> Vec<u8> {
        let mut signature = vec![0u8; NETWORK_SIGNATURE_BYTES];
        let len = self
            .pair
            .sign(message, &mut signature)
            .expect("signing with a valid key cannot fail");
        signature.truncate(len);
        signature
    }
}

#[cfg(test)]
mod tests {
    use sion_crypto::{PublicKey, Scheme, Signature};

    use super::*;

    #[test]
    fn keys_derive_from_their_seed_and_name_their_node() {
        let a = NetworkKey::from_seed(&[1; 32]);
        let b = NetworkKey::from_seed(&[1; 32]);
        let c = NetworkKey::from_seed(&[2; 32]);
        assert_eq!(a.public(), b.public());
        assert_ne!(a.public(), c.public());
        a.public().check().unwrap();
        assert!(NetworkPublicKey(vec![0; 10]).check().is_err());
        assert_eq!(a.seed(), [1; 32]);
        assert!(matches!(a.public().node_id(), NodeId::Node(_)));
        assert!(!format!("{a:?}").contains("0101"));
        let json = serde_json::to_string(a.public()).unwrap();
        assert_eq!(
            &serde_json::from_str::<NetworkPublicKey>(&json).unwrap(),
            a.public()
        );
    }

    /// The C implementation (aws-lc) and the workspace's pure-Rust one
    /// (`fips204`) agree on ML-DSA-44: what the network key signs, the
    /// consensus code's verifier accepts.
    #[test]
    fn network_signatures_verify_with_the_workspace_implementation() {
        let key = NetworkKey::from_seed(&[9; 32]);
        let signature = key.sign(b"transcript");
        assert_eq!(signature.len(), NETWORK_SIGNATURE_BYTES);
        let public = PublicKey {
            scheme: Scheme::MlDsa44,
            bytes: key.public().0.clone(),
        };
        let signature = Signature {
            scheme: Scheme::MlDsa44,
            bytes: signature,
        };
        sion_crypto::verify(&public, "", b"transcript", &signature).unwrap();
        assert!(sion_crypto::verify(&public, "", b"other", &signature).is_err());
    }

    #[test]
    fn the_spki_carries_the_ml_dsa_44_identifier() {
        let key = NetworkKey::from_seed(&[3; 32]);
        let spki = key.public().spki();
        // SEQUENCE { SEQUENCE { OID 2.16.840.1.101.3.4.3.17 }, BIT STRING }.
        let oid = [
            0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x03, 0x11,
        ];
        assert!(spki.windows(oid.len()).any(|w| w == oid));
        assert!(spki.ends_with(&key.public().0));
    }
}
