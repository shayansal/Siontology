//! Sorted Merkle maps with presence and absence proofs.
//!
//! A map is a list of `(key, value)` pairs in strictly increasing key order,
//! committed as `MapCommitment { count, root }` where `root` is the RFC 9162
//! root over the leaf hashes
//! `hash_parts(leaf_domain, [borsh(key), value_digest])` and
//! `value_digest = H(value_domain, borsh(value))`.
//!
//! A [`MapLookup`] for a key reveals a contiguous run of leaves: the leaf
//! just below the key (unless the run starts at index 0), the key's own leaf
//! if present, and the leaf just above (unless the run ends at the last
//! index), with a range proof. Neighbours are revealed by key and value
//! digest only. The bracketing makes presence unique and absence provable:
//! no other leaf can hold the key in a sorted map, and a map that is not
//! sorted is malformed.

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use sion_core::encode::encode;
use sion_core::hash::{hash_domain, hash_parts, Hash32};
use sion_core::merkle::empty_root;

use crate::error::{MemoryError, Result};
use crate::merkle::{range_root, MerkleCache};

/// A map's commitment: its number of entries and its Merkle root.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Hash,
    BorshSerialize,
    BorshDeserialize,
    Serialize,
    Deserialize,
)]
pub struct MapCommitment {
    /// Number of entries.
    pub count: u64,
    /// RFC 9162 root of the leaf hashes (`sion_core::merkle::empty_root()` when empty).
    pub root: Hash32,
}

impl MapCommitment {
    /// The commitment of the empty map.
    pub fn empty() -> Self {
        MapCommitment {
            count: 0,
            root: empty_root(),
        }
    }
}

/// The domains of one map.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MapDomains {
    pub(crate) leaf: &'static str,
    pub(crate) value: &'static str,
}

impl MapDomains {
    pub(crate) fn value_digest<V: BorshSerialize + ?Sized>(&self, value: &V) -> Hash32 {
        hash_domain(self.value, &encode(value))
    }

    pub(crate) fn leaf<K: BorshSerialize>(&self, key: &K, value_digest: &Hash32) -> Hash32 {
        hash_parts(self.leaf, &[&encode(key), &value_digest.0])
    }
}

/// A proof that a key is present in a sorted map, or absent from it.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct MapLookup<K> {
    /// Index of the first revealed leaf.
    pub start: u64,
    /// The neighbour below the key: its key and value digest.
    pub left: Option<(K, Hash32)>,
    /// Whether the key's own leaf follows (its value is supplied by the caller).
    pub found: bool,
    /// The neighbour above the key: its key and value digest.
    pub right: Option<(K, Hash32)>,
    /// Range proof for the revealed leaves.
    pub path: Vec<Hash32>,
}

impl<K: Ord + BorshSerialize> MapLookup<K> {
    /// Checks the lookup for `key` against `commitment`. `value` is the
    /// digest of the value the caller holds for the key, or `None` if it
    /// holds none; the lookup must agree. Returns whether the key is present.
    pub(crate) fn verify(
        &self,
        domains: &MapDomains,
        commitment: &MapCommitment,
        key: &K,
        value: Option<&Hash32>,
    ) -> Result<bool> {
        if self.found != value.is_some() {
            return Err(MemoryError::InvalidProof(
                "map lookup disagrees with the value supplied",
            ));
        }
        if commitment.count == 0 {
            let empty = self.start == 0
                && self.left.is_none()
                && self.right.is_none()
                && !self.found
                && self.path.is_empty()
                && commitment.root == empty_root();
            return if empty {
                Ok(false)
            } else {
                Err(MemoryError::InvalidProof(
                    "malformed lookup in an empty map",
                ))
            };
        }
        let mut leaves = Vec::with_capacity(3);
        if let Some((below, digest)) = &self.left {
            if below >= key {
                return Err(MemoryError::InvalidProof(
                    "left neighbour not below the key",
                ));
            }
            leaves.push(domains.leaf(below, digest));
        }
        if let Some(digest) = value {
            leaves.push(domains.leaf(key, digest));
        }
        if let Some((above, digest)) = &self.right {
            if above <= key {
                return Err(MemoryError::InvalidProof(
                    "right neighbour not above the key",
                ));
            }
            leaves.push(domains.leaf(above, digest));
        }
        if leaves.is_empty() {
            return Err(MemoryError::InvalidProof("lookup reveals nothing"));
        }
        let end = self
            .start
            .checked_add(leaves.len() as u64)
            .ok_or(MemoryError::InvalidProof("lookup overflows"))?;
        if end > commitment.count {
            return Err(MemoryError::InvalidProof("lookup beyond the map"));
        }
        if self.left.is_none() && self.start != 0 {
            return Err(MemoryError::InvalidProof("missing left neighbour"));
        }
        if self.right.is_none() && end != commitment.count {
            return Err(MemoryError::InvalidProof("missing right neighbour"));
        }
        let root = range_root(commitment.count, self.start, &leaves, &self.path)?;
        if root != commitment.root {
            return Err(MemoryError::InvalidProof(
                "lookup does not match the map root",
            ));
        }
        Ok(self.found)
    }
}

/// A sorted map's keys, value digests and Merkle tree.
#[derive(Clone, Debug)]
pub(crate) struct SortedMap<K> {
    keys: Vec<K>,
    digests: Vec<Hash32>,
    tree: MerkleCache,
}

impl<K: Ord + Clone + BorshSerialize> SortedMap<K> {
    /// A map from entries in strictly increasing key order.
    pub(crate) fn new(domains: MapDomains, entries: Vec<(K, Hash32)>) -> Self {
        debug_assert!(entries.windows(2).all(|pair| pair[0].0 < pair[1].0));
        let leaves = entries
            .iter()
            .map(|(key, digest)| domains.leaf(key, digest))
            .collect();
        let (keys, digests) = entries.into_iter().unzip();
        SortedMap {
            keys,
            digests,
            tree: MerkleCache::from_leaves(leaves),
        }
    }

    /// The commitment.
    pub(crate) fn commitment(&self) -> MapCommitment {
        MapCommitment {
            count: self.keys.len() as u64,
            root: self.tree.root_at(self.tree.len()),
        }
    }

    /// The keys, in order.
    pub(crate) fn keys(&self) -> &[K] {
        &self.keys
    }

    /// The position of `key`, or where it would be inserted.
    pub(crate) fn position(&self, key: &K) -> std::result::Result<usize, usize> {
        self.keys.binary_search(key)
    }

    /// A lookup proof for `key`.
    pub(crate) fn lookup(&self, key: &K) -> MapLookup<K> {
        let n = self.keys.len();
        if n == 0 {
            return MapLookup {
                start: 0,
                left: None,
                found: false,
                right: None,
                path: Vec::new(),
            };
        }
        let (found, below, above) = match self.position(key) {
            Ok(i) => (true, i.checked_sub(1), (i + 1 < n).then_some(i + 1)),
            Err(i) => (false, i.checked_sub(1), (i < n).then_some(i)),
        };
        // Without a left neighbour the run starts at the first leaf.
        let start = below.unwrap_or(0);
        let neighbour = |i: usize| (self.keys[i].clone(), self.digests[i]);
        let count = below.is_some() as u64 + found as u64 + above.is_some() as u64;
        let path = self
            .tree
            .range_proof(start as u64, start as u64 + count, n as u64)
            .expect("the revealed run lies inside the map");
        MapLookup {
            start: start as u64,
            left: below.map(neighbour),
            found,
            right: above.map(neighbour),
            path,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const TEST: MapDomains = MapDomains {
        leaf: "sion/mem/test/leaf",
        value: "sion/mem/test/value",
    };

    fn map(keys: &[u32]) -> SortedMap<u32> {
        let entries = keys
            .iter()
            .map(|k| (*k, TEST.value_digest(&(k * 10))))
            .collect();
        SortedMap::new(TEST, entries)
    }

    fn value(k: u32) -> Hash32 {
        TEST.value_digest(&(k * 10))
    }

    #[test]
    fn presence_and_absence_verify() {
        let m = map(&[10, 20, 30, 40, 50]);
        let c = m.commitment();
        assert_eq!(c.count, 5);
        for k in [10u32, 20, 30, 40, 50] {
            let lookup = m.lookup(&k);
            assert!(lookup.found);
            assert!(lookup.verify(&TEST, &c, &k, Some(&value(k))).unwrap());
            assert!(lookup.verify(&TEST, &c, &k, Some(&value(k + 1))).is_err());
            assert!(lookup.verify(&TEST, &c, &k, None).is_err());
        }
        for k in [0u32, 15, 35, 55] {
            let lookup = m.lookup(&k);
            assert!(!lookup.found);
            assert!(!lookup.verify(&TEST, &c, &k, None).unwrap());
            assert!(lookup.verify(&TEST, &c, &k, Some(&value(k))).is_err());
        }
        // An absence proof for 15 does not prove the absence of 20.
        assert!(m.lookup(&15).verify(&TEST, &c, &20, None).is_err());
        assert_eq!(m.keys(), &[10, 20, 30, 40, 50]);
        assert_eq!(m.position(&30), Ok(2));
    }

    #[test]
    fn empty_and_single_maps() {
        let empty = map(&[]);
        let c = empty.commitment();
        assert_eq!(c, MapCommitment::empty());
        let lookup = empty.lookup(&5);
        assert!(!lookup.verify(&TEST, &c, &5, None).unwrap());
        let one = map(&[7]);
        let c1 = one.commitment();
        assert!(one
            .lookup(&7)
            .verify(&TEST, &c1, &7, Some(&value(7)))
            .unwrap());
        assert!(!one.lookup(&3).verify(&TEST, &c1, &3, None).unwrap());
        assert!(!one.lookup(&9).verify(&TEST, &c1, &9, None).unwrap());
        // A lookup from the empty map does not verify against a non-empty one.
        assert!(lookup.verify(&TEST, &c1, &5, None).is_err());
    }

    #[test]
    fn dropped_neighbours_are_caught() {
        let m = map(&[10, 20, 30]);
        let c = m.commitment();
        let mut lookup = m.lookup(&25);
        lookup.left = None;
        assert!(lookup.verify(&TEST, &c, &25, None).is_err());
        let mut lookup = m.lookup(&25);
        lookup.right = None;
        assert!(lookup.verify(&TEST, &c, &25, None).is_err());
        let mut lookup = m.lookup(&20);
        lookup.start += 1;
        assert!(lookup.verify(&TEST, &c, &20, Some(&value(20))).is_err());
    }

    proptest! {
        #![proptest_config(crate::testing::config(256, 0x5eed_3a95))]

        #[test]
        fn lookups_verify_and_tampering_fails(
            keys in proptest::collection::btree_set(0u32..200, 0..40),
            probe in 0u32..210,
            flip in any::<usize>(),
        ) {
            let keys: Vec<u32> = keys.into_iter().collect();
            let m = map(&keys);
            let c = m.commitment();
            let lookup = m.lookup(&probe);
            let present = keys.contains(&probe);
            let held = present.then(|| value(probe));
            prop_assert_eq!(lookup.verify(&TEST, &c, &probe, held.as_ref()).unwrap(), present);
            // Flip one bit of the encoded lookup: it must fail to decode or to verify.
            let bytes = encode(&lookup);
            let mut tampered = bytes.clone();
            let bit = flip % (bytes.len() * 8);
            tampered[bit / 8] ^= 1 << (bit % 8);
            if let Ok(forged) = borsh::from_slice::<MapLookup<u32>>(&tampered) {
                prop_assert!(forged.verify(&TEST, &c, &probe, held.as_ref()).is_err());
            }
        }
    }
}
