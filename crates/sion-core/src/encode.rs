//! Canonical binary encoding.
//!
//! Siontology hashes and signs the Borsh encoding of values. Borsh is
//! deterministic for a given type: fixed-width little-endian integers,
//! length-prefixed sequences, and sorted maps (use `BTreeMap`, never
//! `HashMap`, in consensus-critical types).

use borsh::{BorshDeserialize, BorshSerialize};

use crate::error::CoreError;
use crate::hash::{Hash32, Hasher};

/// Encodes a value canonically.
pub fn encode<T: BorshSerialize + ?Sized>(value: &T) -> Vec<u8> {
    borsh::to_vec(value).expect("encoding into a Vec cannot fail")
}

/// Decodes a value, rejecting trailing bytes.
pub fn decode<T: BorshDeserialize>(bytes: &[u8]) -> Result<T, CoreError> {
    borsh::from_slice(bytes).map_err(|e| CoreError::Decode(e.to_string()))
}

/// Values that have a canonical encoding and therefore a canonical digest.
pub trait Canonical: BorshSerialize {
    /// The canonical encoding.
    fn canonical_bytes(&self) -> Vec<u8> {
        encode(self)
    }

    /// The digest of the canonical encoding in `domain`.
    fn digest(&self, domain: &str) -> Hash32 {
        let mut hasher = Hasher::new(domain);
        hasher.update(&self.canonical_bytes());
        hasher.finalize()
    }
}

impl<T: BorshSerialize> Canonical for T {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[derive(BorshSerialize, BorshDeserialize, Debug, PartialEq)]
    struct Sample {
        id: u64,
        name: String,
        tags: BTreeMap<String, u128>,
    }

    #[test]
    fn round_trip_and_determinism() {
        let mut tags = BTreeMap::new();
        tags.insert("b".to_owned(), 2);
        tags.insert("a".to_owned(), 1);
        let sample = Sample {
            id: 7,
            name: "sion".to_owned(),
            tags,
        };
        let bytes = encode(&sample);
        let back: Sample = decode(&bytes).unwrap();
        assert_eq!(back, sample);
        assert_eq!(sample.digest("sion/test"), back.digest("sion/test"));
    }

    #[test]
    fn rejects_trailing_bytes() {
        let mut bytes = encode(&7u64);
        bytes.push(0);
        assert!(decode::<u64>(&bytes).is_err());
    }
}
