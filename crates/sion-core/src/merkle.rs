//! Append-only Merkle log (RFC 9162 tree shape) with inclusion and
//! consistency proofs.
//!
//! The knowledge layer is grow-only: claims are appended, never removed. A
//! consistency proof between two roots shows that the newer log extends the
//! older one, which is exactly the World Proof's "the knowledge root only
//! grew" obligation. Leaves are leaf hashes ([`leaf_hash`]); interior nodes
//! use a separate domain, so a leaf can never be mistaken for a node.

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};

use crate::error::CoreError;
use crate::hash::{hash_domain, Hash32, Hasher};

const LEAF_DOMAIN: &str = "sion/mth/leaf";
const NODE_DOMAIN: &str = "sion/mth/node";
const EMPTY_DOMAIN: &str = "sion/mth/empty";

/// Hashes leaf data.
pub fn leaf_hash(data: &[u8]) -> Hash32 {
    hash_domain(LEAF_DOMAIN, data)
}

/// Hashes two children into a parent.
pub fn node_hash(left: &Hash32, right: &Hash32) -> Hash32 {
    let mut hasher = Hasher::new(NODE_DOMAIN);
    hasher.update(&left.0).update(&right.0);
    hasher.finalize()
}

/// The root of the empty log.
pub fn empty_root() -> Hash32 {
    hash_domain(EMPTY_DOMAIN, b"")
}

/// Largest power of two strictly less than `n` (requires `n >= 2`).
fn split_point(n: usize) -> usize {
    debug_assert!(n >= 2);
    let mut k = 1;
    while k * 2 < n {
        k *= 2;
    }
    k
}

/// Merkle tree hash over leaf hashes.
pub fn root(leaves: &[Hash32]) -> Hash32 {
    match leaves.len() {
        0 => empty_root(),
        1 => leaves[0],
        n => {
            let k = split_point(n);
            node_hash(&root(&leaves[..k]), &root(&leaves[k..]))
        }
    }
}

/// Audit path for leaf `index`, ordered from the leaf upward.
pub fn inclusion_proof(leaves: &[Hash32], index: usize) -> Result<Vec<Hash32>, CoreError> {
    if index >= leaves.len() {
        return Err(CoreError::OutOfRange {
            index: index as u64,
            size: leaves.len() as u64,
        });
    }
    let mut path = Vec::new();
    inclusion_path(leaves, index, &mut path);
    Ok(path)
}

fn inclusion_path(leaves: &[Hash32], index: usize, path: &mut Vec<Hash32>) {
    let n = leaves.len();
    if n <= 1 {
        return;
    }
    let k = split_point(n);
    if index < k {
        inclusion_path(&leaves[..k], index, path);
        path.push(root(&leaves[k..]));
    } else {
        inclusion_path(&leaves[k..], index - k, path);
        path.push(root(&leaves[..k]));
    }
}

/// Verifies that `leaf` is at `index` in a log of `size` leaves with `root`.
pub fn verify_inclusion(
    root_hash: &Hash32,
    size: u64,
    index: u64,
    leaf: &Hash32,
    path: &[Hash32],
) -> Result<(), CoreError> {
    if index >= size {
        return Err(CoreError::InvalidProof("leaf index beyond log size"));
    }
    let mut fnode = index;
    let mut snode = size - 1;
    let mut acc = *leaf;
    for sibling in path {
        if snode == 0 {
            return Err(CoreError::InvalidProof("inclusion path too long"));
        }
        if fnode & 1 == 1 || fnode == snode {
            acc = node_hash(sibling, &acc);
            if fnode & 1 == 0 {
                while fnode & 1 == 0 && fnode != 0 {
                    fnode >>= 1;
                    snode >>= 1;
                }
            }
        } else {
            acc = node_hash(&acc, sibling);
        }
        fnode >>= 1;
        snode >>= 1;
    }
    if snode == 0 && acc == *root_hash {
        Ok(())
    } else {
        Err(CoreError::InvalidProof(
            "inclusion proof does not match root",
        ))
    }
}

/// Consistency proof that the first `old_size` leaves are a prefix of `leaves`.
pub fn consistency_proof(leaves: &[Hash32], old_size: usize) -> Result<Vec<Hash32>, CoreError> {
    if old_size > leaves.len() {
        return Err(CoreError::OutOfRange {
            index: old_size as u64,
            size: leaves.len() as u64,
        });
    }
    if old_size == 0 || old_size == leaves.len() {
        return Ok(Vec::new());
    }
    let mut proof = Vec::new();
    subproof(old_size, leaves, true, &mut proof);
    Ok(proof)
}

fn subproof(m: usize, leaves: &[Hash32], complete: bool, proof: &mut Vec<Hash32>) {
    let n = leaves.len();
    if m == n {
        if !complete {
            proof.push(root(leaves));
        }
        return;
    }
    let k = split_point(n);
    if m <= k {
        subproof(m, &leaves[..k], complete, proof);
        proof.push(root(&leaves[k..]));
    } else {
        subproof(m - k, &leaves[k..], false, proof);
        proof.push(root(&leaves[..k]));
    }
}

/// Verifies that a log of `old_size` leaves with `old_root` is a prefix of a
/// log of `new_size` leaves with `new_root`.
pub fn verify_consistency(
    old_root: &Hash32,
    old_size: u64,
    new_root: &Hash32,
    new_size: u64,
    proof: &[Hash32],
) -> Result<(), CoreError> {
    if old_size > new_size {
        return Err(CoreError::InvalidProof("old log larger than new log"));
    }
    if old_size == 0 {
        return if proof.is_empty() {
            Ok(())
        } else {
            Err(CoreError::InvalidProof(
                "unexpected proof for empty old log",
            ))
        };
    }
    if old_size == new_size {
        return if proof.is_empty() && old_root == new_root {
            Ok(())
        } else {
            Err(CoreError::InvalidProof("equal sizes require equal roots"))
        };
    }
    if proof.is_empty() {
        return Err(CoreError::InvalidProof("empty consistency proof"));
    }
    let mut path: Vec<Hash32> = Vec::with_capacity(proof.len() + 1);
    if old_size.is_power_of_two() {
        path.push(*old_root);
    }
    path.extend_from_slice(proof);

    let mut fnode = old_size - 1;
    let mut snode = new_size - 1;
    while fnode & 1 == 1 {
        fnode >>= 1;
        snode >>= 1;
    }
    let mut first = path[0];
    let mut second = path[0];
    for sibling in &path[1..] {
        if snode == 0 {
            return Err(CoreError::InvalidProof("consistency path too long"));
        }
        if fnode & 1 == 1 || fnode == snode {
            first = node_hash(sibling, &first);
            second = node_hash(sibling, &second);
            if fnode & 1 == 0 {
                while fnode & 1 == 0 && fnode != 0 {
                    fnode >>= 1;
                    snode >>= 1;
                }
            }
        } else {
            second = node_hash(&second, sibling);
        }
        fnode >>= 1;
        snode >>= 1;
    }
    if first == *old_root && second == *new_root && snode == 0 {
        Ok(())
    } else {
        Err(CoreError::InvalidProof(
            "consistency proof does not match roots",
        ))
    }
}

/// An append-only log of leaf hashes.
#[derive(
    Clone, Debug, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize,
)]
pub struct AppendLog {
    leaves: Vec<Hash32>,
}

impl AppendLog {
    /// An empty log.
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends a leaf hash and returns its index.
    pub fn append(&mut self, leaf: Hash32) -> u64 {
        self.leaves.push(leaf);
        (self.leaves.len() - 1) as u64
    }

    /// Number of leaves.
    pub fn len(&self) -> u64 {
        self.leaves.len() as u64
    }

    /// Whether the log is empty.
    pub fn is_empty(&self) -> bool {
        self.leaves.is_empty()
    }

    /// The current root.
    pub fn root(&self) -> Hash32 {
        root(&self.leaves)
    }

    /// The root of the first `size` leaves.
    pub fn root_at(&self, size: u64) -> Result<Hash32, CoreError> {
        let size = usize::try_from(size).map_err(|_| CoreError::Overflow("log size"))?;
        if size > self.leaves.len() {
            return Err(CoreError::OutOfRange {
                index: size as u64,
                size: self.len(),
            });
        }
        Ok(root(&self.leaves[..size]))
    }

    /// The leaf at `index`.
    pub fn leaf(&self, index: u64) -> Option<&Hash32> {
        usize::try_from(index).ok().and_then(|i| self.leaves.get(i))
    }

    /// Inclusion proof for `index` against the current root.
    pub fn inclusion_proof(&self, index: u64) -> Result<Vec<Hash32>, CoreError> {
        let index = usize::try_from(index).map_err(|_| CoreError::Overflow("log index"))?;
        inclusion_proof(&self.leaves, index)
    }

    /// Consistency proof from an earlier size to the current size.
    pub fn consistency_proof(&self, old_size: u64) -> Result<Vec<Hash32>, CoreError> {
        let old = usize::try_from(old_size).map_err(|_| CoreError::Overflow("log size"))?;
        consistency_proof(&self.leaves, old)
    }
}

/// The right edge of an append-only log: the roots of its perfect subtrees,
/// largest first. Enough to append leaves and recompute the root without
/// the leaves themselves, so a stateless verifier can extend the knowledge
/// log from a witness of at most `log2(n)` hashes.
#[derive(
    Clone, Debug, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize,
)]
pub struct Frontier {
    /// Number of leaves.
    pub size: u64,
    /// Roots of the perfect subtrees, one per set bit of `size`, largest first.
    pub peaks: Vec<Hash32>,
}

impl Frontier {
    /// The frontier of a list of leaves.
    pub fn from_leaves(leaves: &[Hash32]) -> Frontier {
        let mut frontier = Frontier::default();
        for leaf in leaves {
            frontier.append(*leaf);
        }
        frontier
    }

    /// Appends a leaf.
    pub fn append(&mut self, leaf: Hash32) {
        let mut carry = leaf;
        let mut size = self.size;
        while size & 1 == 1 {
            let left = self.peaks.pop().expect("one peak per set bit");
            carry = node_hash(&left, &carry);
            size >>= 1;
        }
        self.peaks.push(carry);
        self.size += 1;
    }

    /// The log root.
    pub fn root(&self) -> Hash32 {
        let mut peaks = self.peaks.iter().rev();
        match peaks.next() {
            None => empty_root(),
            Some(last) => peaks.fold(*last, |acc, peak| node_hash(peak, &acc)),
        }
    }

    /// Whether the frontier is well formed (one peak per set bit).
    pub fn is_well_formed(&self) -> bool {
        self.peaks.len() == self.size.count_ones() as usize
    }
}

impl AppendLog {
    /// The log's frontier.
    pub fn frontier(&self) -> Frontier {
        Frontier::from_leaves(&self.leaves)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn leaves(n: usize) -> Vec<Hash32> {
        (0..n)
            .map(|i| leaf_hash(&(i as u64).to_be_bytes()))
            .collect()
    }

    #[test]
    fn exhaustive_inclusion_small_trees() {
        for n in 1..=40 {
            let ls = leaves(n);
            let r = root(&ls);
            for i in 0..n {
                let proof = inclusion_proof(&ls, i).unwrap();
                verify_inclusion(&r, n as u64, i as u64, &ls[i], &proof).unwrap();
                // Wrong leaf, wrong index and wrong size must all fail.
                let other = leaf_hash(b"other");
                assert!(verify_inclusion(&r, n as u64, i as u64, &other, &proof).is_err());
                if n > 1 {
                    let j = (i + 1) % n;
                    if ls[j] != ls[i] {
                        assert!(verify_inclusion(&r, n as u64, j as u64, &ls[i], &proof).is_err());
                    }
                }
                // The log size is not bound by the proof itself (some paths have
                // the same shape at sizes n and n + 1); it is authenticated with
                // the root, as RFC 9162 does with signed tree heads.
            }
        }
    }

    #[test]
    fn exhaustive_consistency_small_trees() {
        for n in 1..=40 {
            let ls = leaves(n);
            let new_root = root(&ls);
            for m in 0..=n {
                let old_root = root(&ls[..m]);
                let proof = consistency_proof(&ls, m).unwrap();
                verify_consistency(&old_root, m as u64, &new_root, n as u64, &proof).unwrap();
            }
        }
    }

    #[test]
    fn consistency_rejects_rewritten_history() {
        let ls = leaves(13);
        let mut forged = ls.clone();
        forged[3] = leaf_hash(b"rewritten");
        for m in 4..13 {
            let old_root = root(&ls[..m]);
            let proof = consistency_proof(&forged, m).unwrap();
            assert!(
                verify_consistency(&old_root, m as u64, &root(&forged), 13, &proof).is_err(),
                "rewriting leaf 3 must break consistency from size {m}"
            );
        }
    }

    #[test]
    fn append_log_tracks_roots() {
        let mut log = AppendLog::new();
        assert_eq!(log.root(), empty_root());
        let mut roots = vec![log.root()];
        for i in 0..20u64 {
            log.append(leaf_hash(&i.to_be_bytes()));
            roots.push(log.root());
        }
        for (size, expected) in roots.iter().enumerate() {
            assert_eq!(&log.root_at(size as u64).unwrap(), expected);
            let proof = log.consistency_proof(size as u64).unwrap();
            verify_consistency(expected, size as u64, &log.root(), log.len(), &proof).unwrap();
        }
    }

    #[test]
    fn frontier_matches_full_log() {
        let mut frontier = Frontier::default();
        assert_eq!(frontier.root(), empty_root());
        let mut all = Vec::new();
        for i in 0..130u64 {
            let leaf = leaf_hash(&i.to_be_bytes());
            frontier.append(leaf);
            all.push(leaf);
            assert_eq!(frontier.root(), root(&all), "size {}", all.len());
            assert!(frontier.is_well_formed());
        }
        assert_eq!(Frontier::from_leaves(&all), frontier);
    }

    proptest! {
        #[test]
        fn random_consistency(n in 1usize..300, m_frac in 0.0f64..=1.0) {
            let ls = leaves(n);
            let m = ((n as f64) * m_frac) as usize;
            let proof = consistency_proof(&ls, m).unwrap();
            prop_assert!(verify_consistency(&root(&ls[..m]), m as u64, &root(&ls), n as u64, &proof).is_ok());
        }
    }
}
