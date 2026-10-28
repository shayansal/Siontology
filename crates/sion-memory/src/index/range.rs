//! Range queries with proofs of completeness.
//!
//! A [`RangeQuery`] asks for every atom of one frame whose box meets a
//! region and whose valid interval meets a time range. [`prove_range`]
//! answers with the matching atoms' leaf entries ([`AtomEntry`]) and a
//! [`RangeProof`]; [`verify_range`] recomputes the index root from them and
//! checks that the answer is exactly the set of matching atoms, so an empty
//! answer proves that nothing was recorded there.
//!
//! # Proof format
//!
//! A range proof carries the [`IndexHeader`], a lookup of the query's frame
//! in the frame map (presence with the frame's [`FrameValue`], or absence),
//! and for each of the frame's two trees whose root entry meets the query, a
//! [`TreeProof`]: the visited nodes in pre-order. Each visited node lists
//! its entries in order, each as one of:
//!
//! - `Expand`: a child node that meets the query; it is the next node of the
//!   pre-order, and its entry is recomputed from it;
//! - `Prune(entry)`: a child subtree that misses the query, with its entry;
//! - `Hit`: an atom that matches; its entry is the next result;
//! - `Miss(atom entry)`: an atom that misses.
//!
//! The verifier accepts only the canonical proof: every expanded or hit
//! entry meets the query and every pruned or missed one misses it, so each
//! query has exactly one valid proof and one valid answer. The results come
//! in the order of the traversal: the forest first, then the long-lived
//! tree. The kind filter is not part of the proof: the caller fetches the
//! atoms, checks them against the results ([`select_atoms`]) and filters.

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use sion_core::hash::Hash32;

use crate::atom::{FrameId, MemoryAtom, MemoryAtomId, MemoryKind};
use crate::error::{MemoryError, Result};
use crate::geom::{Box3, TimeRange};
use crate::index::map::MapLookup;
use crate::index::tree::{summarize, Node, FANOUT, TAG_ATOMS, TAG_NODES};
use crate::index::{Entry, FrameValue, IndexHeader, MemoryIndex, FRAME_MAP};

/// Deepest tree a verifier accepts (an honest tree over 2⁶⁴ atoms is far shallower).
pub const MAX_PROOF_DEPTH: usize = 96;

/// Everything of one frame in a region during a time range.
#[derive(
    Clone, Debug, PartialEq, Eq, Hash, BorshSerialize, BorshDeserialize, Serialize, Deserialize,
)]
pub struct RangeQuery {
    /// The frame ([`FrameId::NULL`] for atoms without space).
    pub frame: FrameId,
    /// The region (closed box).
    pub region: Box3,
    /// The time range `[start, end)`.
    pub time: TimeRange,
    /// Kinds to keep; empty keeps every kind. Applied to fetched atoms, not proved.
    #[serde(default)]
    pub kinds: Vec<MemoryKind>,
}

impl RangeQuery {
    /// A query over a region and time range of a frame.
    pub fn new(frame: FrameId, region: Box3, time: TimeRange) -> Self {
        RangeQuery {
            frame,
            region,
            time,
            kinds: Vec::new(),
        }
    }

    /// Everything without space during a time range.
    pub fn time_only(time: TimeRange) -> Self {
        RangeQuery::new(FrameId::NULL, Box3::ALL, time)
    }

    /// The query around an atom's own frame, box and interval, which a
    /// complete index must answer with the atom among the results.
    pub fn around(atom: &MemoryAtom) -> Self {
        RangeQuery::new(atom.frame(), atom.bbox(), atom.time_range())
    }

    /// Keeps only these kinds.
    pub fn with_kinds(mut self, kinds: impl IntoIterator<Item = MemoryKind>) -> Self {
        self.kinds = kinds.into_iter().collect();
        self
    }

    /// Checks that the region and the time range are non-empty.
    pub fn validate(&self) -> Result<()> {
        if !self.region.is_valid() {
            return Err(MemoryError::InvalidQuery("region has min greater than max"));
        }
        if !self.time.is_valid() {
            return Err(MemoryError::InvalidQuery("time range is empty"));
        }
        Ok(())
    }

    /// Whether an atom matches in space and time (kinds aside).
    pub fn matches(&self, atom: &MemoryAtom) -> bool {
        atom.frame() == self.frame
            && atom.bbox().intersects(&self.region)
            && atom.time_range().intersects(&self.time)
    }

    /// Whether the kind filter keeps `kind`.
    pub fn keeps_kind(&self, kind: MemoryKind) -> bool {
        self.kinds.is_empty() || self.kinds.contains(&kind)
    }

    fn meets(&self, entry: &Entry) -> bool {
        entry.meets(&self.region, &self.time)
    }
}

/// An atom's leaf entry: what a range answer reports for each atom.
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
pub struct AtomEntry {
    /// The atom's id.
    pub id: MemoryAtomId,
    /// Its box.
    pub bbox: Box3,
    /// Its valid time.
    pub time: TimeRange,
}

impl AtomEntry {
    /// The entry of an atom.
    pub fn of(atom: &MemoryAtom) -> Self {
        AtomEntry {
            id: atom.id(),
            bbox: atom.bbox(),
            time: atom.time_range(),
        }
    }

    fn entry(&self) -> Entry {
        Entry::atom(self.id, self.bbox, self.time)
    }

    fn from_entry(entry: &Entry) -> Self {
        AtomEntry {
            id: MemoryAtomId(entry.digest),
            bbox: entry.bbox,
            time: entry.time,
        }
    }

    fn is_valid(&self) -> bool {
        self.bbox.is_valid() && self.time.is_valid()
    }
}

/// One entry of a visited node.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProofEntry {
    /// A child node that meets the query: the next node of the pre-order.
    Expand,
    /// A child subtree that misses the query.
    Prune(Entry),
    /// An atom that matches: the next result.
    Hit,
    /// An atom that misses the query.
    Miss(AtomEntry),
}

/// A visited node.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct ProofNode {
    /// The node's tag.
    pub tag: u8,
    /// Its entries, in order.
    pub entries: Vec<ProofEntry>,
}

/// The visited part of one tree, in pre-order.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct TreeProof {
    /// Visited nodes; the first is the root.
    pub nodes: Vec<ProofNode>,
}

/// A proof that a range answer is complete.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct RangeProof {
    /// What the index root commits to.
    pub header: IndexHeader,
    /// The frame's presence or absence in the frame map.
    pub frame: MapLookup<FrameId>,
    /// The frame's value, when present.
    pub frame_value: Option<FrameValue>,
    /// The forest's visited part, when its root meets the query.
    pub forest: Option<TreeProof>,
    /// The long-lived tree's visited part, when its root meets the query.
    pub long_lived: Option<TreeProof>,
}

impl RangeProof {
    /// Number of hashes a verifier computes: node digests, map leaves and
    /// range-proof nodes (an estimate of verification work).
    pub fn hash_count(&self) -> usize {
        let nodes = self.forest.as_ref().map_or(0, |t| t.nodes.len())
            + self.long_lived.as_ref().map_or(0, |t| t.nodes.len());
        nodes + 2 * self.frame.path.len() + 6
    }
}

/// Answers a range query from an index.
pub fn prove_range(
    index: &MemoryIndex,
    query: &RangeQuery,
) -> Result<(Vec<AtomEntry>, RangeProof)> {
    query.validate()?;
    let mut hits = Vec::new();
    let lookup = index.frame_map().lookup(&query.frame);
    let (frame_value, forest, long_lived) = match index.frame_index(&query.frame) {
        None => (None, None, None),
        Some(frame) => {
            let mut prove = |root: Option<u32>, entry: Option<Entry>| match (root, entry) {
                (Some(root), Some(entry)) if query.meets(&entry) => {
                    Some(prove_tree(&frame.nodes, root, query, &mut hits))
                }
                _ => None,
            };
            let forest = prove(frame.forest, frame.value.forest);
            let long_lived = prove(frame.long_lived, frame.value.long_lived);
            (Some(frame.value), forest, long_lived)
        }
    };
    let proof = RangeProof {
        header: *index.header(),
        frame: lookup,
        frame_value,
        forest,
        long_lived,
    };
    Ok((hits, proof))
}

fn prove_tree(
    nodes: &[Node],
    root: u32,
    query: &RangeQuery,
    hits: &mut Vec<AtomEntry>,
) -> TreeProof {
    let mut out = Vec::new();
    visit(nodes, root, query, hits, &mut out);
    TreeProof { nodes: out }
}

fn visit(
    nodes: &[Node],
    index: u32,
    query: &RangeQuery,
    hits: &mut Vec<AtomEntry>,
    out: &mut Vec<ProofNode>,
) {
    let node = &nodes[index as usize];
    let position = out.len();
    out.push(ProofNode {
        tag: node.tag,
        entries: Vec::new(),
    });
    let mut entries = Vec::with_capacity(node.entries.len());
    for (i, entry) in node.entries.iter().enumerate() {
        let meets = query.meets(entry);
        if node.tag == TAG_ATOMS {
            if meets {
                hits.push(AtomEntry::from_entry(entry));
                entries.push(ProofEntry::Hit);
            } else {
                entries.push(ProofEntry::Miss(AtomEntry::from_entry(entry)));
            }
        } else if meets {
            entries.push(ProofEntry::Expand);
            visit(nodes, node.children[i], query, hits, out);
        } else {
            entries.push(ProofEntry::Prune(*entry));
        }
    }
    out[position].entries = entries;
}

/// Verifies a range answer against an index root: `Ok(())` means `results`
/// are exactly the atoms of the indexed prefix in `query.frame` whose boxes
/// meet `query.region` and whose valid times meet `query.time`, in
/// traversal order.
pub fn verify_range(
    index_root: &Hash32,
    query: &RangeQuery,
    results: &[AtomEntry],
    proof: &RangeProof,
) -> Result<()> {
    query.validate()?;
    proof.header.check(index_root)?;
    let value_digest = proof
        .frame_value
        .as_ref()
        .map(|v| FRAME_MAP.value_digest(v));
    let present = proof.frame.verify(
        &FRAME_MAP,
        &proof.header.frames,
        &query.frame,
        value_digest.as_ref(),
    )?;
    let mut cursor = 0usize;
    match (present, proof.frame_value) {
        (false, _) => {
            if proof.forest.is_some() || proof.long_lived.is_some() {
                return Err(MemoryError::InvalidProof("trees for an absent frame"));
            }
        }
        (true, Some(value)) => {
            if value.forest.is_none() && value.long_lived.is_none() {
                return Err(MemoryError::InvalidProof("a present frame without trees"));
            }
            check_tree(
                query,
                value.forest,
                proof.forest.as_ref(),
                results,
                &mut cursor,
            )?;
            check_tree(
                query,
                value.long_lived,
                proof.long_lived.as_ref(),
                results,
                &mut cursor,
            )?;
        }
        (true, None) => return Err(MemoryError::InvalidProof("present frame without a value")),
    }
    if cursor != results.len() {
        return Err(MemoryError::InvalidProof(
            "results the proof does not account for",
        ));
    }
    Ok(())
}

fn check_tree(
    query: &RangeQuery,
    root: Option<Entry>,
    proof: Option<&TreeProof>,
    results: &[AtomEntry],
    cursor: &mut usize,
) -> Result<()> {
    match (root, proof) {
        (None, None) => Ok(()),
        (None, Some(_)) => Err(MemoryError::InvalidProof("proof for an empty tree")),
        (Some(root), None) => {
            if query.meets(&root) {
                Err(MemoryError::InvalidProof(
                    "tree meets the query but was not searched",
                ))
            } else {
                Ok(())
            }
        }
        (Some(root), Some(tree)) => {
            if !query.meets(&root) {
                return Err(MemoryError::InvalidProof(
                    "searched a tree that misses the query",
                ));
            }
            let computed = verify_tree(query, tree, results, cursor)?;
            if computed != root {
                return Err(MemoryError::InvalidProof(
                    "tree does not match its root entry",
                ));
            }
            Ok(())
        }
    }
}

struct Open {
    node: usize,
    next: usize,
    entries: Vec<Entry>,
}

/// Recomputes a tree's root entry from its proof, checking every entry
/// against the query and consuming hits from `results`.
fn verify_tree(
    query: &RangeQuery,
    tree: &TreeProof,
    results: &[AtomEntry],
    cursor: &mut usize,
) -> Result<Entry> {
    let first = tree
        .nodes
        .first()
        .ok_or(MemoryError::InvalidProof("empty tree proof"))?;
    check_node_shape(first)?;
    let mut next_node = 1usize;
    let mut stack = vec![Open {
        node: 0,
        next: 0,
        entries: Vec::with_capacity(first.entries.len()),
    }];
    loop {
        let top = stack.len() - 1;
        let (node_index, next) = (stack[top].node, stack[top].next);
        let node = &tree.nodes[node_index];
        if next == node.entries.len() {
            let finished = stack.pop().expect("non-empty stack");
            let summary = summarize(node.tag, &finished.entries)
                .ok_or(MemoryError::InvalidProof("node counts overflow"))?;
            match stack.last_mut() {
                None => {
                    if next_node != tree.nodes.len() {
                        return Err(MemoryError::InvalidProof("unused nodes in tree proof"));
                    }
                    return Ok(summary);
                }
                Some(parent) => {
                    if !query.meets(&summary) {
                        return Err(MemoryError::InvalidProof(
                            "expanded a subtree that misses the query",
                        ));
                    }
                    parent.entries.push(summary);
                    parent.next += 1;
                }
            }
            continue;
        }
        match &node.entries[next] {
            ProofEntry::Expand => {
                if node.tag != TAG_NODES {
                    return Err(MemoryError::InvalidProof("expand in an atom node"));
                }
                if stack.len() >= MAX_PROOF_DEPTH {
                    return Err(MemoryError::InvalidProof("tree proof too deep"));
                }
                let child = tree
                    .nodes
                    .get(next_node)
                    .ok_or(MemoryError::InvalidProof("missing expanded node"))?;
                check_node_shape(child)?;
                stack.push(Open {
                    node: next_node,
                    next: 0,
                    entries: Vec::with_capacity(child.entries.len()),
                });
                next_node += 1;
            }
            ProofEntry::Prune(entry) => {
                if node.tag != TAG_NODES {
                    return Err(MemoryError::InvalidProof("pruned entry in an atom node"));
                }
                if query.meets(entry) {
                    return Err(MemoryError::InvalidProof(
                        "pruned a subtree that meets the query",
                    ));
                }
                if entry.count == 0 || !entry.bbox.is_valid() || !entry.time.is_valid() {
                    return Err(MemoryError::InvalidProof("malformed pruned entry"));
                }
                stack[top].entries.push(*entry);
                stack[top].next += 1;
            }
            ProofEntry::Hit => {
                if node.tag != TAG_ATOMS {
                    return Err(MemoryError::InvalidProof("hit in a node of nodes"));
                }
                let hit = results
                    .get(*cursor)
                    .ok_or(MemoryError::InvalidProof("fewer results than hits"))?;
                *cursor += 1;
                let entry = hit.entry();
                if !hit.is_valid() || !query.meets(&entry) {
                    return Err(MemoryError::InvalidProof("result does not match the query"));
                }
                stack[top].entries.push(entry);
                stack[top].next += 1;
            }
            ProofEntry::Miss(atom) => {
                if node.tag != TAG_ATOMS {
                    return Err(MemoryError::InvalidProof("miss in a node of nodes"));
                }
                let entry = atom.entry();
                if !atom.is_valid() || query.meets(&entry) {
                    return Err(MemoryError::InvalidProof("missed atom matches the query"));
                }
                stack[top].entries.push(entry);
                stack[top].next += 1;
            }
        }
    }
}

fn check_node_shape(node: &ProofNode) -> Result<()> {
    if node.tag != TAG_ATOMS && node.tag != TAG_NODES {
        return Err(MemoryError::InvalidProof("unknown node tag"));
    }
    if node.entries.is_empty() || node.entries.len() > FANOUT {
        return Err(MemoryError::InvalidProof(
            "node with too few or too many entries",
        ));
    }
    Ok(())
}

/// Checks fetched atoms against a verified answer and applies the query's
/// kind filter: `atoms` must be exactly the atoms of `results`, in order;
/// returns those whose kind the query keeps.
pub fn select_atoms<'a>(
    query: &RangeQuery,
    results: &[AtomEntry],
    atoms: &'a [MemoryAtom],
) -> Result<Vec<&'a MemoryAtom>> {
    if atoms.len() != results.len() {
        return Err(MemoryError::InvalidProof("atoms do not match the results"));
    }
    let mut kept = Vec::new();
    for (atom, result) in atoms.iter().zip(results) {
        if AtomEntry::of(atom) != *result {
            return Err(MemoryError::InvalidProof("atom does not match its result"));
        }
        if query.keeps_kind(atom.kind) {
            kept.push(atom);
        }
    }
    Ok(kept)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::testing::{frames, random_log};
    use crate::index::BUCKET_NS;
    use proptest::prelude::*;
    use rand_core::RngCore;
    use sion_core::encode::encode;
    use std::collections::BTreeSet;

    fn brute_force(atoms: &[MemoryAtom], query: &RangeQuery) -> BTreeSet<MemoryAtomId> {
        atoms
            .iter()
            .filter(|a| query.matches(a))
            .map(|a| a.id())
            .collect()
    }

    fn random_query(seed: u64) -> RangeQuery {
        let mut rng = sion_core::rng::seeded(seed ^ 0x5eed);
        let frame = match rng.next_u32() % 5 {
            0 => FrameId::NULL,
            1 => FrameId::named("nowhere"),
            n => frames()[(n % 2) as usize],
        };
        let x = (rng.next_u64() % 110) as i64 - 5;
        let y = (rng.next_u64() % 110) as i64 - 5;
        let z = (rng.next_u64() % 12) as i64 - 1;
        let w = (rng.next_u64() % 40) as i64;
        let region = if frame == FrameId::NULL {
            Box3::ALL
        } else {
            Box3::new([x, y, z], [x + w, y + w, z + (rng.next_u64() % 4) as i64])
        };
        let start = rng.next_u64() % (3 * BUCKET_NS);
        let end = start + 1 + rng.next_u64() % BUCKET_NS;
        RangeQuery::new(frame, region, TimeRange::new(start, end))
    }

    #[test]
    fn answers_match_brute_force_and_verify() {
        let atoms = random_log(11, 300);
        let index = MemoryIndex::build(&atoms).unwrap();
        let mut nonempty = 0;
        for q in 0..200 {
            let query = random_query(q);
            let (results, proof) = prove_range(&index, &query).unwrap();
            verify_range(&index.root(), &query, &results, &proof).unwrap();
            let got: BTreeSet<MemoryAtomId> = results.iter().map(|r| r.id).collect();
            assert_eq!(got.len(), results.len(), "no duplicates");
            assert_eq!(got, brute_force(&atoms, &query));
            nonempty += usize::from(!results.is_empty());
        }
        assert!(nonempty > 20, "the test queries find atoms");
    }

    #[test]
    fn empty_answers_are_proved() {
        let atoms = random_log(12, 100);
        let index = MemoryIndex::build(&atoms).unwrap();
        // A frame that holds nothing: proved absent.
        let absent = RangeQuery::new(FrameId::named("mars"), Box3::ALL, TimeRange::ALL);
        let (results, proof) = prove_range(&index, &absent).unwrap();
        assert!(results.is_empty() && proof.frame_value.is_none());
        verify_range(&index.root(), &absent, &results, &proof).unwrap();
        // A region far from every atom of a present frame.
        let far = RangeQuery::new(
            frames()[0],
            Box3::point([10_000, 10_000, 10_000]),
            TimeRange::ALL,
        );
        let (results, proof) = prove_range(&index, &far).unwrap();
        assert!(results.is_empty());
        verify_range(&index.root(), &far, &results, &proof).unwrap();
        // The empty index.
        let empty = MemoryIndex::empty();
        let (results, proof) = prove_range(&empty, &far).unwrap();
        verify_range(&empty.root(), &far, &results, &proof).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn time_only_queries_find_spaceless_atoms() {
        let atoms = random_log(13, 150);
        let index = MemoryIndex::build(&atoms).unwrap();
        let query = RangeQuery::time_only(TimeRange::ALL);
        let (results, proof) = prove_range(&index, &query).unwrap();
        verify_range(&index.root(), &query, &results, &proof).unwrap();
        let spaceless = atoms.iter().filter(|a| a.space.is_none()).count();
        assert!(spaceless > 0);
        assert_eq!(results.len(), spaceless);
    }

    #[test]
    fn wrong_answers_fail() {
        let atoms = random_log(14, 200);
        let index = MemoryIndex::build(&atoms).unwrap();
        let root = index.root();
        let query = RangeQuery::new(
            frames()[0],
            Box3::new([0, 0, 0], [60, 60, 10]),
            TimeRange::ALL,
        );
        let (results, proof) = prove_range(&index, &query).unwrap();
        assert!(results.len() > 3);
        verify_range(&root, &query, &results, &proof).unwrap();
        // Dropping, adding, reordering or altering a result fails.
        assert!(verify_range(&root, &query, &results[1..], &proof).is_err());
        let mut extra = results.clone();
        extra.push(results[0]);
        assert!(verify_range(&root, &query, &extra, &proof).is_err());
        let mut swapped = results.clone();
        swapped.swap(0, 1);
        assert!(verify_range(&root, &query, &swapped, &proof).is_err());
        let mut moved = results.clone();
        moved[0].bbox.max[0] += 1;
        assert!(verify_range(&root, &query, &moved, &proof).is_err());
        // A proof answers exactly the queries that classify every entry it
        // reveals the same way, and for those its answer is right too; a
        // query with a different answer fails.
        let ids: BTreeSet<MemoryAtomId> = results.iter().map(|r| r.id).collect();
        for x in 55..70 {
            let other = RangeQuery::new(
                frames()[0],
                Box3::new([0, 0, 0], [x, 60, 10]),
                TimeRange::ALL,
            );
            if verify_range(&root, &other, &results, &proof).is_ok() {
                assert_eq!(brute_force(&atoms, &other), ids);
            } else {
                assert!(x != 60);
            }
        }
        let other = RangeQuery::new(
            frames()[0],
            Box3::new([0, 0, 0], [30, 30, 10]),
            TimeRange::ALL,
        );
        assert_ne!(brute_force(&atoms, &other), ids);
        assert!(verify_range(&root, &other, &results, &proof).is_err());
        let mut other_frame = query.clone();
        other_frame.frame = frames()[1];
        assert!(verify_range(&root, &other_frame, &results, &proof).is_err());
        // Nor a different root.
        assert!(verify_range(&Hash32::ZERO, &query, &results, &proof).is_err());
        // Invalid queries are refused.
        let mut bad = query.clone();
        bad.time = TimeRange::new(5, 5);
        assert!(prove_range(&index, &bad).is_err());
        assert!(verify_range(&root, &bad, &results, &proof).is_err());
        bad = query.clone();
        bad.region = Box3::new([1, 0, 0], [0, 0, 0]);
        assert!(bad.validate().is_err());
        assert!(proof.hash_count() > 0);
    }

    #[test]
    fn kind_filter_applies_to_fetched_atoms() {
        let atoms = random_log(15, 200);
        let index = MemoryIndex::build(&atoms).unwrap();
        let query = RangeQuery::new(frames()[1], Box3::ALL, TimeRange::ALL)
            .with_kinds([MemoryKind::Observation, MemoryKind::Pose]);
        let (results, proof) = prove_range(&index, &query).unwrap();
        verify_range(&index.root(), &query, &results, &proof).unwrap();
        let fetched: Vec<MemoryAtom> = results
            .iter()
            .map(|r| atoms.iter().find(|a| a.id() == r.id).unwrap().clone())
            .collect();
        let kept = select_atoms(&query, &results, &fetched).unwrap();
        assert!(kept.iter().all(|a| query.keeps_kind(a.kind)));
        let expected = fetched.iter().filter(|a| query.keeps_kind(a.kind)).count();
        assert_eq!(kept.len(), expected);
        assert!(select_atoms(&query, &results, &fetched[1..]).is_err());
        if fetched.len() > 1 {
            let mut swapped = fetched.clone();
            swapped.swap(0, 1);
            assert!(select_atoms(&query, &results, &swapped).is_err());
        }
        assert!(
            RangeQuery::new(FrameId::NULL, Box3::ALL, TimeRange::ALL).keeps_kind(MemoryKind::Skill)
        );
    }

    proptest! {
        #![proptest_config(crate::testing::config(128, 0x5eed_7a6e))]

        #[test]
        fn range_answers_are_complete_and_tamper_evident(
            seed in any::<u64>(),
            n in 0usize..160,
            q in any::<u64>(),
            flip in any::<usize>(),
        ) {
            let atoms = random_log(seed, n);
            let index = MemoryIndex::build(&atoms).unwrap();
            let root = index.root();
            let query = random_query(q);
            let (results, proof) = prove_range(&index, &query).unwrap();
            prop_assert!(verify_range(&root, &query, &results, &proof).is_ok());
            let got: BTreeSet<MemoryAtomId> = results.iter().map(|r| r.id).collect();
            prop_assert_eq!(&got, &brute_force(&atoms, &query));

            // Removing any one result fails.
            if !results.is_empty() {
                let i = flip % results.len();
                let mut fewer = results.clone();
                fewer.remove(i);
                prop_assert!(verify_range(&root, &query, &fewer, &proof).is_err());
            }
            // Adding a result fails: a non-matching atom, or a matching one left out.
            if let Some(other) = atoms.iter().find(|a| !got.contains(&a.id())) {
                let mut more = results.clone();
                more.insert(flip % (results.len() + 1), AtomEntry::of(other));
                prop_assert!(verify_range(&root, &query, &more, &proof).is_err());
            }
            // Flipping any bit of an encoded result or of the proof fails.
            if !results.is_empty() {
                let mut bytes = encode(&results);
                let bit = 32 + flip % ((bytes.len() - 4) * 8);
                bytes[bit / 8] ^= 1 << (bit % 8);
                if let Ok(forged) = borsh::from_slice::<Vec<AtomEntry>>(&bytes) {
                    prop_assert!(verify_range(&root, &query, &forged, &proof).is_err());
                }
            }
            let bytes = encode(&proof);
            let mut tampered = bytes.clone();
            let bit = flip % (bytes.len() * 8);
            tampered[bit / 8] ^= 1 << (bit % 8);
            if let Ok(forged) = borsh::from_slice::<RangeProof>(&tampered) {
                prop_assert!(verify_range(&root, &query, &results, &forged).is_err());
            }
        }
    }
}
