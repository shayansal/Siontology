//! RFC 9162 Merkle trees with cached subtree roots.
//!
//! The tree shape and hashes are exactly `sion_core::merkle`'s (interior
//! nodes `node_hash`, the empty tree `empty_root`), so every root and proof
//! here equals the one `sion_core` computes. Caching the roots of aligned
//! perfect subtrees makes roots, inclusion proofs and consistency proofs
//! `O(log² n)` instead of `O(n)`, which matters for logs of millions of
//! atoms.
//!
//! It adds one proof `sion_core` lacks: a **range proof** for a contiguous
//! run of leaves `[lo, hi)`, the roots of the maximal subtrees outside the
//! run in left-to-right order. The sorted maps of the index use it to reveal
//! a key together with its neighbours.

use sion_core::hash::Hash32;
use sion_core::merkle::{empty_root, node_hash, root};

use crate::error::{MemoryError, Result};

/// Largest power of two strictly less than `n` (`n >= 2`).
pub(crate) fn split_point(n: u64) -> u64 {
    debug_assert!(n >= 2);
    1u64 << (63 - (n - 1).leading_zeros())
}

/// Leaves of an RFC 9162 tree with the roots of all aligned perfect subtrees.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct MerkleCache {
    /// `levels[l][j]` is the root of leaves `[j·2^l, (j+1)·2^l)`.
    levels: Vec<Vec<Hash32>>,
}

impl MerkleCache {
    /// An empty tree.
    pub(crate) fn new() -> Self {
        MerkleCache {
            levels: vec![Vec::new()],
        }
    }

    /// A tree over `leaves`.
    pub(crate) fn from_leaves(leaves: Vec<Hash32>) -> Self {
        let mut levels = vec![leaves];
        while levels.last().is_some_and(|level| level.len() >= 2) {
            let below = levels.last().expect("non-empty");
            let above: Vec<Hash32> = below
                .chunks_exact(2)
                .map(|pair| node_hash(&pair[0], &pair[1]))
                .collect();
            levels.push(above);
        }
        MerkleCache { levels }
    }

    /// Appends a leaf.
    pub(crate) fn push(&mut self, leaf: Hash32) {
        if self.levels.is_empty() {
            self.levels.push(Vec::new());
        }
        self.levels[0].push(leaf);
        let mut level = 0;
        while self.levels[level].len() % 2 == 0 {
            let len = self.levels[level].len();
            let parent = node_hash(&self.levels[level][len - 2], &self.levels[level][len - 1]);
            if self.levels.len() == level + 1 {
                self.levels.push(Vec::new());
            }
            self.levels[level + 1].push(parent);
            level += 1;
        }
    }

    /// Number of leaves.
    pub(crate) fn len(&self) -> u64 {
        self.levels.first().map_or(0, |leaves| leaves.len() as u64)
    }

    /// The leaves.
    pub(crate) fn leaves(&self) -> &[Hash32] {
        self.levels.first().map_or(&[], |leaves| leaves.as_slice())
    }

    /// The RFC 9162 root of leaves `[offset, offset + size)`.
    pub(crate) fn subtree_root(&self, offset: u64, size: u64) -> Hash32 {
        if size == 0 {
            return empty_root();
        }
        if size.is_power_of_two() && offset % size == 0 {
            let level = size.trailing_zeros() as usize;
            if let Some(hash) = self
                .levels
                .get(level)
                .and_then(|row| row.get((offset >> level) as usize))
            {
                return *hash;
            }
        }
        if size == 1 {
            return self.leaves()[offset as usize];
        }
        let k = split_point(size);
        node_hash(
            &self.subtree_root(offset, k),
            &self.subtree_root(offset + k, size - k),
        )
    }

    /// The root of the first `size` leaves (`size <= len`).
    pub(crate) fn root_at(&self, size: u64) -> Hash32 {
        debug_assert!(size <= self.len());
        self.subtree_root(0, size)
    }

    /// Audit path for leaf `index` in the tree of the first `size` leaves,
    /// ordered from the leaf upward (as `sion_core::merkle::inclusion_proof`).
    pub(crate) fn inclusion_proof(&self, index: u64, size: u64) -> Result<Vec<Hash32>> {
        if index >= size || size > self.len() {
            return Err(MemoryError::OutOfRange {
                index,
                size: size.min(self.len()),
            });
        }
        let mut path = Vec::new();
        self.path(0, size, index, &mut path);
        Ok(path)
    }

    fn path(&self, offset: u64, size: u64, index: u64, out: &mut Vec<Hash32>) {
        if size <= 1 {
            return;
        }
        let k = split_point(size);
        if index < k {
            self.path(offset, k, index, out);
            out.push(self.subtree_root(offset + k, size - k));
        } else {
            self.path(offset + k, size - k, index - k, out);
            out.push(self.subtree_root(offset, k));
        }
    }

    /// Consistency proof from size `old` to size `new` (as
    /// `sion_core::merkle::consistency_proof` over the first `new` leaves).
    pub(crate) fn consistency_proof(&self, old: u64, new: u64) -> Result<Vec<Hash32>> {
        if old > new || new > self.len() {
            return Err(MemoryError::OutOfRange {
                index: old.max(new),
                size: self.len(),
            });
        }
        let mut proof = Vec::new();
        if old != 0 && old != new {
            self.subproof(0, old, new, true, &mut proof);
        }
        Ok(proof)
    }

    fn subproof(&self, offset: u64, m: u64, n: u64, complete: bool, out: &mut Vec<Hash32>) {
        if m == n {
            if !complete {
                out.push(self.subtree_root(offset, n));
            }
            return;
        }
        let k = split_point(n);
        if m <= k {
            self.subproof(offset, m, k, complete, out);
            out.push(self.subtree_root(offset + k, n - k));
        } else {
            self.subproof(offset + k, m - k, n - k, false, out);
            out.push(self.subtree_root(offset, k));
        }
    }

    /// Range proof for leaves `[lo, hi)` in the tree of the first `size`
    /// leaves: the roots of the maximal subtrees outside the range, left to
    /// right. Verify with [`range_root`].
    pub(crate) fn range_proof(&self, lo: u64, hi: u64, size: u64) -> Result<Vec<Hash32>> {
        if lo >= hi || hi > size || size > self.len() {
            return Err(MemoryError::OutOfRange {
                index: hi,
                size: size.min(self.len()),
            });
        }
        let mut out = Vec::new();
        self.range_walk(0, size, lo, hi, &mut out);
        Ok(out)
    }

    fn range_walk(&self, offset: u64, n: u64, lo: u64, hi: u64, out: &mut Vec<Hash32>) {
        let end = offset + n;
        if hi <= offset || lo >= end {
            out.push(self.subtree_root(offset, n));
            return;
        }
        if lo <= offset && end <= hi {
            return;
        }
        let k = split_point(n);
        self.range_walk(offset, k, lo, hi, out);
        self.range_walk(offset + k, n - k, lo, hi, out);
    }
}

/// Recomputes the root of a tree of `size` leaves from the contiguous leaves
/// `leaves` starting at `lo` and the range proof for them.
pub(crate) fn range_root(
    size: u64,
    lo: u64,
    leaves: &[Hash32],
    proof: &[Hash32],
) -> Result<Hash32> {
    let count = leaves.len() as u64;
    let hi = lo
        .checked_add(count)
        .ok_or(MemoryError::InvalidProof("range overflows"))?;
    if count == 0 || hi > size {
        return Err(MemoryError::InvalidProof("range outside the tree"));
    }
    let mut cursor = 0usize;
    let hash = range_rebuild(0, size, lo, hi, leaves, proof, &mut cursor)?;
    if cursor != proof.len() {
        return Err(MemoryError::InvalidProof("range proof too long"));
    }
    Ok(hash)
}

fn range_rebuild(
    offset: u64,
    n: u64,
    lo: u64,
    hi: u64,
    leaves: &[Hash32],
    proof: &[Hash32],
    cursor: &mut usize,
) -> Result<Hash32> {
    let end = offset + n;
    if hi <= offset || lo >= end {
        let hash = proof
            .get(*cursor)
            .copied()
            .ok_or(MemoryError::InvalidProof("range proof too short"))?;
        *cursor += 1;
        return Ok(hash);
    }
    if lo <= offset && end <= hi {
        let start = (offset - lo) as usize;
        return Ok(root(&leaves[start..start + n as usize]));
    }
    let k = split_point(n);
    let left = range_rebuild(offset, k, lo, hi, leaves, proof, cursor)?;
    let right = range_rebuild(offset + k, n - k, lo, hi, leaves, proof, cursor)?;
    Ok(node_hash(&left, &right))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use sion_core::merkle;

    fn leaves(n: u64) -> Vec<Hash32> {
        (0..n)
            .map(|i| merkle::leaf_hash(&i.to_be_bytes()))
            .collect()
    }

    #[test]
    fn split_points_match_rfc_9162() {
        assert_eq!(split_point(2), 1);
        assert_eq!(split_point(3), 2);
        assert_eq!(split_point(4), 2);
        assert_eq!(split_point(5), 4);
        assert_eq!(split_point(1 << 40), 1 << 39);
        assert_eq!(split_point((1 << 40) + 1), 1 << 40);
    }

    #[test]
    fn roots_and_proofs_equal_sion_core_exhaustively() {
        for n in 0..=33u64 {
            let ls = leaves(n);
            let mut pushed = MerkleCache::new();
            for leaf in &ls {
                pushed.push(*leaf);
            }
            let built = MerkleCache::from_leaves(ls.clone());
            assert_eq!(pushed, built, "push and batch build agree at {n}");
            assert_eq!(built.len(), n);
            assert_eq!(built.leaves(), ls.as_slice());
            for size in 0..=n {
                let prefix = &ls[..size as usize];
                assert_eq!(built.root_at(size), merkle::root(prefix));
                for index in 0..size {
                    assert_eq!(
                        built.inclusion_proof(index, size).unwrap(),
                        merkle::inclusion_proof(prefix, index as usize).unwrap()
                    );
                }
                for old in 0..=size {
                    assert_eq!(
                        built.consistency_proof(old, size).unwrap(),
                        merkle::consistency_proof(prefix, old as usize).unwrap()
                    );
                }
            }
        }
        let empty = MerkleCache::default();
        assert_eq!(empty.len(), 0);
        assert_eq!(empty.root_at(0), merkle::empty_root());
    }

    #[test]
    fn out_of_range_requests_fail() {
        let tree = MerkleCache::from_leaves(leaves(5));
        assert!(tree.inclusion_proof(5, 5).is_err());
        assert!(tree.inclusion_proof(0, 6).is_err());
        assert!(tree.consistency_proof(4, 3).is_err());
        assert!(tree.consistency_proof(1, 6).is_err());
        assert!(tree.range_proof(2, 2, 5).is_err());
        assert!(tree.range_proof(0, 6, 6).is_err());
    }

    #[test]
    fn range_proofs_rebuild_every_root() {
        for n in 1..=24u64 {
            let ls = leaves(n);
            let tree = MerkleCache::from_leaves(ls.clone());
            let expected = merkle::root(&ls);
            for lo in 0..n {
                for hi in lo + 1..=n {
                    let proof = tree.range_proof(lo, hi, n).unwrap();
                    let part = &ls[lo as usize..hi as usize];
                    assert_eq!(range_root(n, lo, part, &proof).unwrap(), expected);
                    // A shifted range, a wrong leaf or a truncated proof fails.
                    if hi < n {
                        assert_ne!(range_root(n, lo + 1, part, &proof).ok(), Some(expected));
                    }
                    let mut wrong = part.to_vec();
                    wrong[0] = merkle::leaf_hash(b"forged");
                    assert_ne!(range_root(n, lo, &wrong, &proof).ok(), Some(expected));
                    if !proof.is_empty() {
                        assert!(
                            range_root(n, lo, part, &proof[1..]).is_err()
                                || range_root(n, lo, part, &proof[1..]).unwrap() != expected
                        );
                        let mut longer = proof.clone();
                        longer.push(Hash32::ZERO);
                        assert!(range_root(n, lo, part, &longer).is_err());
                    }
                }
            }
        }
        assert!(range_root(4, 0, &[], &[]).is_err());
        assert!(range_root(4, 3, &leaves(2), &[]).is_err());
        assert!(range_root(4, u64::MAX, &leaves(2), &[]).is_err());
    }

    proptest! {
        #![proptest_config(crate::testing::config(128, 0x5eed_3e71))]

        #[test]
        fn random_proofs_match_sion_core(n in 1u64..400, a in any::<u64>(), b in any::<u64>()) {
            let ls = leaves(n);
            let tree = MerkleCache::from_leaves(ls.clone());
            let size = 1 + a % n;
            let index = b % size;
            let prefix = &ls[..size as usize];
            prop_assert_eq!(tree.root_at(size), merkle::root(prefix));
            let path = tree.inclusion_proof(index, size).unwrap();
            prop_assert!(merkle::verify_inclusion(&merkle::root(prefix), size, index, &ls[index as usize], &path).is_ok());
            let old = b % (size + 1);
            let proof = tree.consistency_proof(old, size).unwrap();
            prop_assert!(merkle::verify_consistency(&merkle::root(&ls[..old as usize]), old, &merkle::root(prefix), size, &proof).is_ok());
        }
    }
}
