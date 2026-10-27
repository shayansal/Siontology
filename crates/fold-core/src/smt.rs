//! Sparse Merkle tree over 256-bit keys.
//!
//! This is the state tree. A subtree that holds exactly one entry is
//! represented by that entry's leaf hash at whatever depth it sits (as in
//! Diem's Jellyfish tree), so paths stay about `log2(n)` long instead of 256.
//! Empty subtrees hash to [`Hash32::ZERO`].
//!
//! Entries are never deleted: consumed objects are overwritten with a
//! tombstone value. That keeps history auditable and lets a verifier apply
//! a batch of updates statelessly from proofs alone ([`PartialSmt`]), which is
//! what the Fold's provers and checkers do.

use std::collections::{BTreeMap, HashMap};

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};

use crate::error::CoreError;
use crate::hash::{Hash32, Hasher};

const LEAF_DOMAIN: &str = "fold/smt/leaf";
const NODE_DOMAIN: &str = "fold/smt/node";

/// Hash of a leaf entry.
pub fn leaf_hash(key: &Hash32, value: &Hash32) -> Hash32 {
    let mut hasher = Hasher::new(LEAF_DOMAIN);
    hasher.update(&key.0).update(&value.0);
    hasher.finalize()
}

/// Hash of an internal node.
pub fn node_hash(left: &Hash32, right: &Hash32) -> Hash32 {
    let mut hasher = Hasher::new(NODE_DOMAIN);
    hasher.update(&left.0).update(&right.0);
    hasher.finalize()
}

/// Keeps the first `depth` bits of `key` and zeroes the rest.
fn prefix(key: &Hash32, depth: usize) -> Hash32 {
    let mut out = [0u8; 32];
    let full = depth / 8;
    out[..full].copy_from_slice(&key.0[..full]);
    if depth % 8 != 0 && full < 32 {
        let mask = 0xffu8 << (8 - depth % 8);
        out[full] = key.0[full] & mask;
    }
    Hash32(out)
}

fn flip_bit(key: &Hash32, index: usize) -> Hash32 {
    let mut out = key.0;
    out[index / 8] ^= 1 << (7 - (index % 8));
    Hash32(out)
}

/// What a proof found at the bottom of a key's path.
#[derive(Clone, Copy, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub enum ProofLeaf {
    /// The subtree on the key's path is empty.
    Empty,
    /// The subtree on the key's path holds exactly this one entry.
    Leaf {
        /// The entry's key (equal to the queried key for inclusion).
        key: Hash32,
        /// The entry's value hash.
        value: Hash32,
    },
}

/// A Merkle proof for one key: the sibling hashes from the root downward,
/// and what sits at the bottom of the key's path.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct SmtProof {
    /// Sibling hashes, `siblings[d]` being the sibling at depth `d + 1`.
    pub siblings: Vec<Hash32>,
    /// The bottom of the path.
    pub leaf: ProofLeaf,
}

impl SmtProof {
    fn bottom_hash(&self) -> Hash32 {
        match self.leaf {
            ProofLeaf::Empty => Hash32::ZERO,
            ProofLeaf::Leaf { key, value } => leaf_hash(&key, &value),
        }
    }

    /// Hashes along `key`'s path from the bottom (index `len`) to the root
    /// (index 0).
    fn path_hashes(&self, key: &Hash32) -> Vec<Hash32> {
        let depth = self.siblings.len();
        let mut hashes = vec![Hash32::ZERO; depth + 1];
        hashes[depth] = self.bottom_hash();
        for d in (0..depth).rev() {
            let child = hashes[d + 1];
            let sibling = self.siblings[d];
            hashes[d] = if key.bit(d) {
                node_hash(&sibling, &child)
            } else {
                node_hash(&child, &sibling)
            };
        }
        hashes
    }

    /// The root this proof commits to for `key`.
    pub fn compute_root(&self, key: &Hash32) -> Hash32 {
        self.path_hashes(key)[0]
    }

    /// The value proven for `key`: `Some(value)` for inclusion, `None` for
    /// non-inclusion. Errors if the proof is malformed for `key`.
    pub fn proven_value(&self, key: &Hash32) -> Result<Option<Hash32>, CoreError> {
        if self.siblings.len() > 256 {
            return Err(CoreError::InvalidProof("path longer than key"));
        }
        match self.leaf {
            ProofLeaf::Empty => Ok(None),
            ProofLeaf::Leaf { key: found, value } => {
                let depth = self.siblings.len();
                if prefix(&found, depth) != prefix(key, depth) {
                    return Err(CoreError::InvalidProof("leaf not on the key's path"));
                }
                if found == *key {
                    Ok(Some(value))
                } else {
                    Ok(None)
                }
            }
        }
    }
}

/// Verifies that `key` maps to `value` (`None` = absent) under `root`.
pub fn verify_proof(
    root: &Hash32,
    key: &Hash32,
    value: Option<&Hash32>,
    proof: &SmtProof,
) -> Result<(), CoreError> {
    let proven = proof.proven_value(key)?;
    if proven.as_ref() != value {
        return Err(CoreError::InvalidProof("proof shows a different value"));
    }
    if proof.compute_root(key) != *root {
        return Err(CoreError::InvalidProof("proof does not match root"));
    }
    Ok(())
}

/// A sparse Merkle tree held in full.
#[derive(Clone, Debug, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct SparseMerkleTree {
    entries: BTreeMap<Hash32, Hash32>,
}

impl SparseMerkleTree {
    /// An empty tree.
    pub fn new() -> Self {
        Self::default()
    }

    /// Inserts or replaces an entry.
    pub fn insert(&mut self, key: Hash32, value: Hash32) -> Option<Hash32> {
        self.entries.insert(key, value)
    }

    /// Looks up an entry.
    pub fn get(&self, key: &Hash32) -> Option<&Hash32> {
        self.entries.get(key)
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the tree is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Iterates entries in key order.
    pub fn iter(&self) -> impl Iterator<Item = (&Hash32, &Hash32)> {
        self.entries.iter()
    }

    /// The root hash. For many proofs against one root, use [`Self::snapshot`].
    pub fn root(&self) -> Hash32 {
        let entries: Vec<(Hash32, Hash32)> = self.entries.iter().map(|(k, v)| (*k, *v)).collect();
        subtree_hash(&entries, 0, None)
    }

    /// Builds a cached snapshot for computing the root and many proofs.
    pub fn snapshot(&self) -> SmtSnapshot {
        let entries: Vec<(Hash32, Hash32)> = self.entries.iter().map(|(k, v)| (*k, *v)).collect();
        let mut cache = HashMap::new();
        let root = subtree_hash(&entries, 0, Some(&mut cache));
        SmtSnapshot { root, cache }
    }
}

#[derive(Clone, Copy, Debug)]
enum CachedNode {
    Leaf { key: Hash32, value: Hash32 },
    Internal(Hash32),
}

fn subtree_hash(
    entries: &[(Hash32, Hash32)],
    depth: usize,
    mut cache: Option<&mut HashMap<(u16, Hash32), CachedNode>>,
) -> Hash32 {
    match entries.len() {
        0 => Hash32::ZERO,
        1 => {
            let (key, value) = entries[0];
            if let Some(cache) = cache {
                cache.insert((depth as u16, prefix(&key, depth)), CachedNode::Leaf { key, value });
            }
            leaf_hash(&key, &value)
        }
        _ => {
            debug_assert!(depth < 256, "distinct keys diverge before depth 256");
            let split = entries.partition_point(|(key, _)| !key.bit(depth));
            let left = subtree_hash(&entries[..split], depth + 1, cache.as_deref_mut());
            let right = subtree_hash(&entries[split..], depth + 1, cache.as_deref_mut());
            let hash = node_hash(&left, &right);
            if let Some(cache) = cache {
                cache.insert(
                    (depth as u16, prefix(&entries[0].0, depth)),
                    CachedNode::Internal(hash),
                );
            }
            hash
        }
    }
}

/// A tree snapshot with every node hash cached.
#[derive(Clone, Debug)]
pub struct SmtSnapshot {
    root: Hash32,
    cache: HashMap<(u16, Hash32), CachedNode>,
}

impl SmtSnapshot {
    /// The root hash.
    pub fn root(&self) -> Hash32 {
        self.root
    }

    fn node_at(&self, depth: usize, key: &Hash32) -> Option<CachedNode> {
        self.cache.get(&(depth as u16, prefix(key, depth))).copied()
    }

    /// Proves `key`'s presence or absence.
    pub fn prove(&self, key: &Hash32) -> SmtProof {
        let mut siblings = Vec::new();
        let mut depth = 0usize;
        loop {
            match self.node_at(depth, key) {
                None => {
                    return SmtProof {
                        siblings,
                        leaf: ProofLeaf::Empty,
                    }
                }
                Some(CachedNode::Leaf { key: found, value }) => {
                    return SmtProof {
                        siblings,
                        leaf: ProofLeaf::Leaf { key: found, value },
                    }
                }
                Some(CachedNode::Internal(_)) => {
                    let sibling_key = flip_bit(&prefix(key, depth + 1), depth);
                    let sibling = match self.node_at(depth + 1, &sibling_key) {
                        None => Hash32::ZERO,
                        Some(CachedNode::Leaf { key, value }) => leaf_hash(&key, &value),
                        Some(CachedNode::Internal(hash)) => hash,
                    };
                    siblings.push(sibling);
                    depth += 1;
                }
            }
        }
    }
}

/// A partial tree reconstructed from proofs against a trusted root. It can
/// apply inserts and updates to the proven keys and compute the new root
/// without the rest of the tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PartialSmt {
    node: PNode,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum PNode {
    /// A subtree known only by its hash.
    Opaque(Hash32),
    Empty,
    Leaf(Hash32, Hash32),
    Internal(Box<PNode>, Box<PNode>),
}

impl PNode {
    fn hash(&self) -> Hash32 {
        match self {
            PNode::Opaque(hash) => *hash,
            PNode::Empty => Hash32::ZERO,
            PNode::Leaf(key, value) => leaf_hash(key, value),
            PNode::Internal(left, right) => node_hash(&left.hash(), &right.hash()),
        }
    }
}

impl PartialSmt {
    /// Starts from a trusted root.
    pub fn new(root: Hash32) -> Self {
        let node = if root == Hash32::ZERO {
            PNode::Empty
        } else {
            PNode::Opaque(root)
        };
        PartialSmt { node }
    }

    /// Reconstructs a partial tree from proofs, checking each against `root`.
    pub fn from_proofs<'a>(
        root: Hash32,
        proofs: impl IntoIterator<Item = (&'a Hash32, &'a SmtProof)>,
    ) -> Result<Self, CoreError> {
        let mut tree = PartialSmt::new(root);
        for (key, proof) in proofs {
            tree.add_proof(key, proof)?;
        }
        Ok(tree)
    }

    /// Adds one proof, expanding the opaque nodes along the key's path.
    pub fn add_proof(&mut self, key: &Hash32, proof: &SmtProof) -> Result<(), CoreError> {
        proof.proven_value(key)?;
        let hashes = proof.path_hashes(key);
        if hashes[0] != self.node.hash() {
            return Err(CoreError::InvalidProof("proof root does not match partial tree"));
        }
        let bottom = match proof.leaf {
            ProofLeaf::Empty => PNode::Empty,
            ProofLeaf::Leaf { key, value } => PNode::Leaf(key, value),
        };
        expand(&mut self.node, key, 0, &hashes, &proof.siblings, bottom)
    }

    /// The current root.
    pub fn root(&self) -> Hash32 {
        self.node.hash()
    }

    /// Reads a proven key: `Ok(Some)` present, `Ok(None)` absent, `Err` if
    /// the key's path was not covered by a proof.
    pub fn get(&self, key: &Hash32) -> Result<Option<Hash32>, CoreError> {
        let mut node = &self.node;
        let mut depth = 0usize;
        loop {
            match node {
                PNode::Opaque(_) => return Err(CoreError::InvalidProof("key not covered by witness")),
                PNode::Empty => return Ok(None),
                PNode::Leaf(found, value) => {
                    return Ok(if found == key { Some(*value) } else { None })
                }
                PNode::Internal(left, right) => {
                    node = if key.bit(depth) { right } else { left };
                    depth += 1;
                }
            }
        }
    }

    /// Inserts or updates a key whose path is covered by a proof.
    pub fn insert(&mut self, key: Hash32, value: Hash32) -> Result<(), CoreError> {
        insert_into(&mut self.node, key, value, 0)
    }
}

fn expand(
    node: &mut PNode,
    key: &Hash32,
    depth: usize,
    hashes: &[Hash32],
    siblings: &[Hash32],
    bottom: PNode,
) -> Result<(), CoreError> {
    if depth == siblings.len() {
        return match node {
            PNode::Opaque(hash) => {
                if *hash != bottom.hash() {
                    return Err(CoreError::InvalidProof("bottom hash mismatch"));
                }
                *node = bottom;
                Ok(())
            }
            other => {
                if *other == bottom {
                    Ok(())
                } else {
                    Err(CoreError::InvalidProof("conflicting proofs"))
                }
            }
        };
    }
    if let PNode::Opaque(hash) = node {
        if *hash != hashes[depth] {
            return Err(CoreError::InvalidProof("path hash mismatch"));
        }
        let child = Box::new(PNode::Opaque(hashes[depth + 1]));
        let sibling = Box::new(PNode::Opaque(siblings[depth]));
        *node = if key.bit(depth) {
            PNode::Internal(sibling, child)
        } else {
            PNode::Internal(child, sibling)
        };
    }
    match node {
        PNode::Internal(left, right) => {
            let (next, other) = if key.bit(depth) {
                (right, left)
            } else {
                (left, right)
            };
            if other.hash() != siblings[depth] {
                return Err(CoreError::InvalidProof("sibling hash mismatch"));
            }
            expand(next, key, depth + 1, hashes, siblings, bottom)
        }
        _ => Err(CoreError::InvalidProof("proof longer than the known path")),
    }
}

fn insert_into(node: &mut PNode, key: Hash32, value: Hash32, depth: usize) -> Result<(), CoreError> {
    match node {
        PNode::Opaque(_) => Err(CoreError::InvalidProof("key not covered by witness")),
        PNode::Empty => {
            *node = PNode::Leaf(key, value);
            Ok(())
        }
        PNode::Leaf(existing, existing_value) => {
            if *existing == key {
                *existing_value = value;
                return Ok(());
            }
            let mut pair = [(*existing, *existing_value), (key, value)];
            pair.sort();
            *node = build_subtree(&pair, depth);
            Ok(())
        }
        PNode::Internal(left, right) => {
            if key.bit(depth) {
                insert_into(right, key, value, depth + 1)
            } else {
                insert_into(left, key, value, depth + 1)
            }
        }
    }
}

fn build_subtree(entries: &[(Hash32, Hash32)], depth: usize) -> PNode {
    match entries.len() {
        0 => PNode::Empty,
        1 => PNode::Leaf(entries[0].0, entries[0].1),
        _ => {
            let split = entries.partition_point(|(key, _)| !key.bit(depth));
            PNode::Internal(
                Box::new(build_subtree(&entries[..split], depth + 1)),
                Box::new(build_subtree(&entries[split..], depth + 1)),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::hash_domain;
    use proptest::prelude::*;

    fn key(i: u64) -> Hash32 {
        hash_domain("test/key", &i.to_be_bytes())
    }
    fn value(i: u64) -> Hash32 {
        hash_domain("test/value", &i.to_be_bytes())
    }

    #[test]
    fn empty_and_single() {
        let mut tree = SparseMerkleTree::new();
        assert_eq!(tree.root(), Hash32::ZERO);
        let proof = tree.snapshot().prove(&key(1));
        verify_proof(&Hash32::ZERO, &key(1), None, &proof).unwrap();
        tree.insert(key(1), value(1));
        assert_eq!(tree.root(), leaf_hash(&key(1), &value(1)));
        let snap = tree.snapshot();
        verify_proof(&snap.root(), &key(1), Some(&value(1)), &snap.prove(&key(1))).unwrap();
        verify_proof(&snap.root(), &key(2), None, &snap.prove(&key(2))).unwrap();
    }

    #[test]
    fn inclusion_and_absence_proofs() {
        let mut tree = SparseMerkleTree::new();
        for i in 0..200 {
            tree.insert(key(i), value(i));
        }
        let snap = tree.snapshot();
        assert_eq!(snap.root(), tree.root());
        for i in 0..200 {
            let proof = snap.prove(&key(i));
            verify_proof(&snap.root(), &key(i), Some(&value(i)), &proof).unwrap();
            assert!(verify_proof(&snap.root(), &key(i), Some(&value(i + 1)), &proof).is_err());
            assert!(verify_proof(&snap.root(), &key(i), None, &proof).is_err());
        }
        for i in 200..260 {
            let proof = snap.prove(&key(i));
            verify_proof(&snap.root(), &key(i), None, &proof).unwrap();
        }
    }

    #[test]
    fn root_is_order_independent() {
        let mut a = SparseMerkleTree::new();
        let mut b = SparseMerkleTree::new();
        for i in 0..50 {
            a.insert(key(i), value(i));
        }
        for i in (0..50).rev() {
            b.insert(key(i), value(i));
        }
        assert_eq!(a.root(), b.root());
    }

    #[test]
    fn stateless_updates_match_full_tree() {
        let mut tree = SparseMerkleTree::new();
        for i in 0..100 {
            tree.insert(key(i), value(i));
        }
        let snap = tree.snapshot();
        // Update ten existing keys and insert ten new ones.
        let touched: Vec<u64> = (0..10).map(|i| i * 7).chain(100..110).collect();
        let proofs: Vec<(Hash32, SmtProof)> =
            touched.iter().map(|i| (key(*i), snap.prove(&key(*i)))).collect();
        let mut partial =
            PartialSmt::from_proofs(snap.root(), proofs.iter().map(|(k, p)| (k, p))).unwrap();
        assert_eq!(partial.root(), snap.root());
        for i in &touched {
            assert_eq!(partial.get(&key(*i)).unwrap(), tree.get(&key(*i)).copied());
            partial.insert(key(*i), value(*i + 1000)).unwrap();
            tree.insert(key(*i), value(*i + 1000));
        }
        assert_eq!(partial.root(), tree.root());
        // A key without a witness cannot be touched.
        assert!(partial.insert(key(55), value(1)).is_err() || touched.contains(&55));
    }

    #[test]
    fn forged_proof_is_rejected_by_partial_tree() {
        let mut tree = SparseMerkleTree::new();
        for i in 0..20 {
            tree.insert(key(i), value(i));
        }
        let snap = tree.snapshot();
        let mut proof = snap.prove(&key(3));
        proof.leaf = ProofLeaf::Leaf {
            key: key(3),
            value: value(99),
        };
        assert!(PartialSmt::from_proofs(snap.root(), [(&key(3), &proof)]).is_err());
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(48))]
        #[test]
        fn random_batches_update_statelessly(
            initial in proptest::collection::btree_set(0u64..500, 0..80),
            updates in proptest::collection::btree_set(0u64..600, 1..25),
        ) {
            let mut tree = SparseMerkleTree::new();
            for i in &initial {
                tree.insert(key(*i), value(*i));
            }
            let snap = tree.snapshot();
            let proofs: Vec<(Hash32, SmtProof)> =
                updates.iter().map(|i| (key(*i), snap.prove(&key(*i)))).collect();
            let mut partial = PartialSmt::from_proofs(snap.root(), proofs.iter().map(|(k, p)| (k, p))).unwrap();
            for i in &updates {
                partial.insert(key(*i), value(i + 7)).unwrap();
                tree.insert(key(*i), value(i + 7));
            }
            prop_assert_eq!(partial.root(), tree.root());
        }
    }
}
