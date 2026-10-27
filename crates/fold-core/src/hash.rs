//! Domain-separated SHA3-256 hashing.
//!
//! Every hash in Fold is computed as
//! `SHA3-256( u16_be(len(domain)) || domain || data )`, so a digest computed
//! for one purpose (say, a claim) can never be replayed as a digest for
//! another (say, a transaction). SHA3 is a hash-based primitive and is not
//! weakened by Shor's algorithm; Grover only halves its preimage margin.

use std::fmt;

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha3::{Digest, Sha3_256};

use crate::error::CoreError;

/// A 32-byte digest.
#[derive(
    Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, BorshSerialize, BorshDeserialize,
)]
pub struct Hash32(pub [u8; 32]);

impl Hash32 {
    /// The all-zero digest, used as the hash of an empty subtree.
    pub const ZERO: Hash32 = Hash32([0u8; 32]);

    /// Returns the raw bytes.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Lowercase hex encoding of all 32 bytes.
    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }

    /// Parses a 64-character hex string (an optional `0x` prefix is accepted).
    pub fn from_hex(text: &str) -> Result<Self, CoreError> {
        let trimmed = text.strip_prefix("0x").unwrap_or(text);
        let bytes = hex::decode(trimmed).map_err(|e| CoreError::InvalidHex(e.to_string()))?;
        let array: [u8; 32] = bytes
            .try_into()
            .map_err(|_| CoreError::InvalidHex(format!("expected 32 bytes, got {}", text.len() / 2)))?;
        Ok(Hash32(array))
    }

    /// The first eight hex characters, for logs and human-readable output.
    pub fn short(&self) -> String {
        hex::encode(&self.0[..4])
    }

    /// Returns bit `index` (0 = most significant bit of byte 0).
    pub fn bit(&self, index: usize) -> bool {
        debug_assert!(index < 256);
        (self.0[index / 8] >> (7 - (index % 8))) & 1 == 1
    }

    /// Interprets the first eight bytes as a big-endian integer; handy for
    /// deterministic sampling from a digest.
    pub fn prefix_u64(&self) -> u64 {
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(&self.0[..8]);
        u64::from_be_bytes(bytes)
    }
}

impl fmt::Debug for Hash32 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Hash32({})", self.short())
    }
}

impl fmt::Display for Hash32 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl Serialize for Hash32 {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for Hash32 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = <String as Deserialize>::deserialize(deserializer)?;
        Hash32::from_hex(&text).map_err(serde::de::Error::custom)
    }
}

impl From<[u8; 32]> for Hash32 {
    fn from(bytes: [u8; 32]) -> Self {
        Hash32(bytes)
    }
}

/// An incremental domain-separated hasher.
#[derive(Clone)]
pub struct Hasher {
    inner: Sha3_256,
}

impl fmt::Debug for Hasher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Hasher(SHA3-256)")
    }
}

impl Hasher {
    /// Starts a hash in `domain`.
    pub fn new(domain: &str) -> Self {
        let mut inner = Sha3_256::new();
        let length = u16::try_from(domain.len()).expect("domain tags are short");
        inner.update(length.to_be_bytes());
        inner.update(domain.as_bytes());
        Hasher { inner }
    }

    /// Absorbs raw bytes.
    pub fn update(&mut self, data: &[u8]) -> &mut Self {
        self.inner.update(data);
        self
    }

    /// Absorbs a length-prefixed byte string, so that consecutive parts can
    /// never be re-split into a different sequence with the same hash.
    pub fn update_part(&mut self, data: &[u8]) -> &mut Self {
        self.inner.update((data.len() as u64).to_be_bytes());
        self.inner.update(data);
        self
    }

    /// Finishes the hash.
    pub fn finalize(self) -> Hash32 {
        Hash32(self.inner.finalize().into())
    }
}

/// Hashes `data` in `domain`.
pub fn hash_domain(domain: &str, data: &[u8]) -> Hash32 {
    let mut hasher = Hasher::new(domain);
    hasher.update(data);
    hasher.finalize()
}

/// Hashes a sequence of length-prefixed parts in `domain`.
pub fn hash_parts(domain: &str, parts: &[&[u8]]) -> Hash32 {
    let mut hasher = Hasher::new(domain);
    for part in parts {
        hasher.update_part(part);
    }
    hasher.finalize()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domains_separate_digests() {
        assert_ne!(hash_domain("fold/a", b"x"), hash_domain("fold/b", b"x"));
        assert_eq!(hash_domain("fold/a", b"x"), hash_domain("fold/a", b"x"));
    }

    #[test]
    fn parts_are_unambiguous() {
        let left = hash_parts("fold/t", &[b"ab", b"c"]);
        let right = hash_parts("fold/t", &[b"a", b"bc"]);
        assert_ne!(left, right);
    }

    #[test]
    fn hex_round_trip() {
        let digest = hash_domain("fold/t", b"hello");
        let parsed = Hash32::from_hex(&digest.to_hex()).unwrap();
        assert_eq!(digest, parsed);
        let prefixed = Hash32::from_hex(&format!("0x{}", digest.to_hex())).unwrap();
        assert_eq!(digest, prefixed);
        assert!(Hash32::from_hex("abcd").is_err());
    }

    #[test]
    fn bits_are_msb_first() {
        let mut bytes = [0u8; 32];
        bytes[0] = 0b1000_0001;
        let digest = Hash32(bytes);
        assert!(digest.bit(0));
        assert!(!digest.bit(1));
        assert!(digest.bit(7));
        assert!(!digest.bit(8));
    }

    #[test]
    fn serde_uses_hex() {
        let digest = hash_domain("fold/t", b"x");
        let json = serde_json::to_string(&digest).unwrap();
        assert_eq!(json, format!("\"{}\"", digest.to_hex()));
        let back: Hash32 = serde_json::from_str(&json).unwrap();
        assert_eq!(back, digest);
    }
}
