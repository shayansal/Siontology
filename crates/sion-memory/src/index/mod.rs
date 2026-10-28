//! The 4D index of a writer's log, and its proofs.
//!
//! The index root of a log prefix is a deterministic function of the atoms
//! in that prefix and of nothing else: not of insertion history, timing,
//! hash-map order or the machine. Anyone holding the atoms can rebuild it,
//! so there is exactly one correct index root for each prefix, and a writer
//! who commits another can be caught ([`omission`]). This documentation is
//! the specification; two implementations that follow it compute the same
//! root.
//!
//! # Notation
//!
//! - `borsh(x)` is the Borsh encoding: integers little-endian at their
//!   width, `Vec`/`String` as a `u32` little-endian length then the items,
//!   `Option` as one byte (0 or 1) then the value, enums as a one-byte
//!   variant index then the fields, structs as their fields in order.
//! - `H(d, m)` is `sion_core::hash::hash_domain`:
//!   `SHA3-256(u16_be(len(d)) ‖ d ‖ m)`.
//! - `Hp(d, [a, b, …])` is `sion_core::hash::hash_parts`:
//!   `SHA3-256(u16_be(len(d)) ‖ d ‖ u64_be(len(a)) ‖ a ‖ u64_be(len(b)) ‖ b ‖ …)`.
//! - `MTH(leaves)` is the RFC 9162 Merkle tree hash of `sion_core::merkle`:
//!   interior nodes `H("sion/mth/node", left ‖ right)`, split at the largest
//!   power of two below the size, and `H("sion/mth/empty", "")` for no leaves.
//!
//! # Atoms as index entries
//!
//! Every tree entry is an [`Entry`] `{ digest: [u8; 32], bbox: Box3, time:
//! TimeRange, count: u64 }`, 104 bytes in Borsh: the digest, `min[0..3]` and
//! `max[0..3]` as `i64`, `time.start` and `time.end` as `u64`, then `count`.
//!
//! An atom's leaf entry has `digest` = its [`MemoryAtomId`]
//! (`H("sion/mem/atom", borsh(atom))`, with the fields in the order
//! [`MemoryAtom`] declares them), `bbox` = its `space.bbox` (or
//! `[0,0,0]–[0,0,0]` without space), `time` = `[valid.start, valid.end)`
//! with an open end written as `u64::MAX`, and `count = 1`. Its frame is
//! `space.frame`, or the reserved null frame `FrameId([0; 32])` without
//! space; atoms with space never use the null frame, and a time-only query
//! asks the null frame with the box covering every coordinate.
//!
//! An atom is **short-lived** if its valid interval is closed and
//! `valid.start / B == (valid.end − 1) / B` with `B` = [`BUCKET_NS`] =
//! 3,600 × 10⁹ (one hour); its bucket is `valid.start / B`. Every other atom
//! (open-ended, or spanning buckets) is **long-lived**.
//!
//! # Nodes
//!
//! A node has a tag (0: its entries are atoms; 1: its entries are nodes) and
//! one to [`FANOUT`] = 4 entries. Its digest is
//! `H("sion/mem/index/node", borsh(tag: u8, entries: Vec<Entry>))`. The
//! node's own entry has that digest, the union of its entries' boxes
//! (componentwise minimum of `min`, maximum of `max`), the union of their
//! times (minimum start, maximum end) and the sum of their counts.
//!
//! # Packing: Sort-Tile-Recursive
//!
//! To pack `n ≥ 1` entries: let `P = ⌈n / 4⌉` and `S` the least integer
//! with `S³ ≥ P`. Sort all entries by key 0; cut the sorted list into
//! consecutive slabs of `S²·4` entries; sort each slab by key 1; cut each
//! slab into consecutive columns of `S·4`; sort each column by key 2. The
//! nodes of the next level are the consecutive runs of 4 of the result (the
//! last may be shorter). The key on axis `a` is the pair
//! `(min[a] + max[a] as an exact integer, digest)`, compared
//! lexicographically, the digest as 32 unsigned bytes: ties in position are
//! broken by atom id (or node digest). Everything is integer arithmetic.
//!
//! An **R-tree** over a set of atoms: pack the atom entries into tag-0
//! nodes; while the current level has more than one node, pack the level's
//! node entries into tag-1 nodes. Its root entry is the entry of the one
//! node at the top (so a single atom still has a tag-0 root node).
//!
//! # Per frame
//!
//! For each frame that holds at least one atom of the prefix:
//!
//! 1. **The forest.** For each bucket holding short-lived atoms, an R-tree
//!    over them. The bucket roots, in increasing bucket order, are packed
//!    without sorting: while more than one entry remains, consecutive runs
//!    of 4 become tag-1 nodes. One bucket is its own root. The entries of
//!    this tree over buckets carry each bucket's box and time range, so one
//!    traversal prunes by time and space together.
//! 2. **The long-lived tree.** An R-tree over the frame's long-lived atoms.
//!
//! The frame's value is [`FrameValue`] `{ forest: Option<Entry>,
//! long_lived: Option<Entry> }`, the two root entries (`None` when empty).
//!
//! # Maps
//!
//! A map is a list of `(key, value)` in strictly increasing key order,
//! committed as [`MapCommitment`] `{ count: u64, root: MTH(leaves) }` with
//! `leaf = Hp(leaf_domain, [borsh(key), H(value_domain, borsh(value))])`.
//!
//! | Map | Key, order | Value | Domains |
//! |---|---|---|---|
//! | frames | `FrameId`, by its 32 bytes | `FrameValue` | `sion/mem/index/frame/leaf`, `…/frame/value` |
//! | subjects | `(subject, predicate)` of claim statements, by subject bytes then predicate bytes | `Vec<MemoryAtomId>`: the claims' ids in log order | `sion/mem/index/subject/leaf`, `…/subject/value` |
//! | links | a target `MemoryAtomId`, by its bytes | `Vec<MemoryAtomId>`: in log order and without repeats, every atom of the prefix that names the target as a revision target, a conflict member or a link target | `sion/mem/index/link/leaf`, `…/link/value` |
//!
//! Encodings: a `FrameId` or `MemoryAtomId` key is its 32 bytes; a subject
//! key is `u32_le(len(subject)) ‖ subject ‖ u32_le(len(predicate)) ‖
//! predicate`; a `Vec<MemoryAtomId>` is `u32_le(count)` then the 32-byte ids;
//! a `FrameValue` is the forest's `Option<Entry>` then the long-lived tree's
//! (each one byte, 0 or 1, then the 104-byte entry when present). Byte
//! strings compare as unsigned bytes, shorter first on a common prefix.
//!
//! # The index root
//!
//! `index_root = H("sion/mem/index", borsh(IndexHeader { version: 1, size,
//! frames, subjects, links }))`, where `size` is the number of atoms in the
//! prefix and the other fields are the three map commitments: 129 bytes,
//! `u8` version, `u64_le` size, then for each map `u64_le(count) ‖ root`.
//! The empty prefix has empty maps (`count = 0`, `root = MTH([])`).
//!
//! A test vector of six atoms with its atom ids, log root and index root is
//! pinned in this module's tests (`spec_vector`), and
//! `tests/spec_reference.rs` re-implements this section from the text alone
//! and checks that it agrees on random logs.
//!
//! # Proofs
//!
//! - [`range`]: everything in a box of a frame during a time range, complete
//!   (MR-tree style: pruned subtrees with their summaries, the misses in
//!   visited leaves), including proofs that nothing was recorded.
//! - [`keyed`]: the complete atom list for a `(subject, predicate)` or a
//!   link target, with presence and absence proofs.
//! - [`omission`]: fraud proofs that an index leaves out an atom of its log.

pub mod keyed;
pub mod map;
pub mod omission;
pub mod range;
pub mod tree;

use std::collections::{BTreeMap, BTreeSet};

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use sion_core::encode::encode;
use sion_core::hash::{hash_domain, Hash32};

use crate::atom::{FrameId, MemoryAtom, MemoryAtomId};
use crate::error::{MemoryError, Result};
use map::{MapCommitment, MapDomains, SortedMap};
use tree::{build_rtree, build_top, Node};
pub use tree::{Entry, FANOUT, TAG_ATOMS, TAG_NODES};

/// Width of a time bucket of the forest: one hour, in nanoseconds.
pub const BUCKET_NS: u64 = 3_600 * 1_000_000_000;

/// Version of the index construction, bound into the root.
pub const INDEX_VERSION: u8 = 1;

/// Domain of the index root.
pub const INDEX_DOMAIN: &str = "sion/mem/index";

pub(crate) const FRAME_MAP: MapDomains = MapDomains {
    leaf: "sion/mem/index/frame/leaf",
    value: "sion/mem/index/frame/value",
};
pub(crate) const SUBJECT_MAP: MapDomains = MapDomains {
    leaf: "sion/mem/index/subject/leaf",
    value: "sion/mem/index/subject/value",
};
pub(crate) const LINK_MAP: MapDomains = MapDomains {
    leaf: "sion/mem/index/link/leaf",
    value: "sion/mem/index/link/value",
};

/// What the index root commits to.
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
pub struct IndexHeader {
    /// [`INDEX_VERSION`].
    pub version: u8,
    /// Atoms in the prefix.
    pub size: u64,
    /// The frame map.
    pub frames: MapCommitment,
    /// The subject map.
    pub subjects: MapCommitment,
    /// The link map.
    pub links: MapCommitment,
}

impl IndexHeader {
    /// The index root: `H("sion/mem/index", borsh(header))`.
    pub fn root(&self) -> Hash32 {
        hash_domain(INDEX_DOMAIN, &encode(self))
    }

    pub(crate) fn check(&self, index_root: &Hash32) -> Result<()> {
        if self.version != INDEX_VERSION {
            return Err(MemoryError::InvalidProof("unknown index version"));
        }
        if self.root() != *index_root {
            return Err(MemoryError::InvalidProof(
                "header does not match the index root",
            ));
        }
        Ok(())
    }
}

/// A frame's entry in the frame map: the roots of its two trees.
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
pub struct FrameValue {
    /// Root entry of the time-bucketed forest of short-lived atoms.
    pub forest: Option<Entry>,
    /// Root entry of the tree of long-lived atoms.
    pub long_lived: Option<Entry>,
}

/// A frame's trees.
#[derive(Clone, Debug)]
pub(crate) struct FrameIndex {
    pub(crate) value: FrameValue,
    pub(crate) nodes: Vec<Node>,
    pub(crate) forest: Option<u32>,
    pub(crate) long_lived: Option<u32>,
}

/// The index of a log prefix: built by [`MemoryIndex::build`] or an
/// [`IndexBuilder`], queried by the functions of [`range`], [`keyed`] and
/// [`omission`].
#[derive(Clone, Debug)]
pub struct MemoryIndex {
    header: IndexHeader,
    root: Hash32,
    frames: Vec<FrameIndex>,
    frame_map: SortedMap<FrameId>,
    subject_lists: Vec<Vec<MemoryAtomId>>,
    subject_map: SortedMap<(String, String)>,
    link_lists: Vec<Vec<MemoryAtomId>>,
    link_map: SortedMap<MemoryAtomId>,
}

impl MemoryIndex {
    /// Indexes a log prefix: `atoms[i].seq` must be `i`.
    pub fn build(atoms: &[MemoryAtom]) -> Result<MemoryIndex> {
        let mut builder = IndexBuilder::new();
        for atom in atoms {
            builder.push(atom)?;
        }
        Ok(builder.finish())
    }

    /// The index of the empty log.
    pub fn empty() -> MemoryIndex {
        IndexBuilder::new().finish()
    }

    /// Builds a deliberately wrong index that leaves out the atoms whose
    /// sequence numbers are in `omit` while claiming to cover every atom of
    /// `atoms`. **For tests and demonstrations of fraud proofs only**: a
    /// writer that commits its root can be convicted with
    /// [`omission::prove_omission`].
    pub fn forge_omitting(atoms: &[MemoryAtom], omit: &BTreeSet<u64>) -> Result<MemoryIndex> {
        let mut builder = IndexBuilder::new();
        for atom in atoms {
            if omit.contains(&atom.seq) {
                builder.skip(atom)?;
            } else {
                builder.push(atom)?;
            }
        }
        Ok(builder.finish())
    }

    /// The index root.
    pub fn root(&self) -> Hash32 {
        self.root
    }

    /// What the root commits to.
    pub fn header(&self) -> &IndexHeader {
        &self.header
    }

    /// Atoms in the indexed prefix.
    pub fn size(&self) -> u64 {
        self.header.size
    }

    /// The frames holding atoms, in key order.
    pub fn frames(&self) -> &[FrameId] {
        self.frame_map.keys()
    }

    /// A frame's tree roots.
    pub fn frame_value(&self, frame: &FrameId) -> Option<&FrameValue> {
        self.frame_map
            .position(frame)
            .ok()
            .map(|i| &self.frames[i].value)
    }

    /// The claims with this subject and predicate, in log order.
    pub fn subject(&self, subject: &str, predicate: &str) -> &[MemoryAtomId] {
        let key = (subject.to_owned(), predicate.to_owned());
        self.subject_map
            .position(&key)
            .map_or(&[], |i| self.subject_lists[i].as_slice())
    }

    /// The atoms that name `target`, in log order.
    pub fn linkers(&self, target: &MemoryAtomId) -> &[MemoryAtomId] {
        self.link_map
            .position(target)
            .map_or(&[], |i| self.link_lists[i].as_slice())
    }

    /// Number of index nodes in every frame's trees.
    pub fn node_count(&self) -> usize {
        self.frames.iter().map(|frame| frame.nodes.len()).sum()
    }

    pub(crate) fn frame_index(&self, frame: &FrameId) -> Option<&FrameIndex> {
        self.frame_map.position(frame).ok().map(|i| &self.frames[i])
    }

    pub(crate) fn frame_map(&self) -> &SortedMap<FrameId> {
        &self.frame_map
    }

    pub(crate) fn subject_map(&self) -> &SortedMap<(String, String)> {
        &self.subject_map
    }

    pub(crate) fn link_map(&self) -> &SortedMap<MemoryAtomId> {
        &self.link_map
    }

    pub(crate) fn subject_list(&self, position: usize) -> &[MemoryAtomId] {
        &self.subject_lists[position]
    }

    pub(crate) fn link_list(&self, position: usize) -> &[MemoryAtomId] {
        &self.link_lists[position]
    }
}

/// The index root of a log prefix (`atoms[i].seq == i`).
pub fn index_root(atoms: &[MemoryAtom]) -> Result<Hash32> {
    Ok(MemoryIndex::build(atoms)?.root())
}

#[derive(Clone, Debug, Default)]
struct FrameItems {
    buckets: BTreeMap<u64, Vec<Entry>>,
    long_lived: Vec<Entry>,
}

/// Builds an index from atoms pushed in log order, without keeping the atoms
/// (so a log of millions of atoms can be indexed in bounded memory).
#[derive(Clone, Debug, Default)]
pub struct IndexBuilder {
    size: u64,
    frames: BTreeMap<FrameId, FrameItems>,
    subjects: BTreeMap<(String, String), Vec<MemoryAtomId>>,
    links: BTreeMap<MemoryAtomId, Vec<MemoryAtomId>>,
}

impl IndexBuilder {
    /// An empty builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Atoms pushed so far.
    pub fn len(&self) -> u64 {
        self.size
    }

    /// Whether nothing was pushed.
    pub fn is_empty(&self) -> bool {
        self.size == 0
    }

    /// Validates and indexes the next atom of the log; returns its id.
    pub fn push(&mut self, atom: &MemoryAtom) -> Result<MemoryAtomId> {
        let id = atom.id();
        self.push_with_id(atom, id)?;
        Ok(id)
    }

    /// Indexes the next atom under an id the caller already computed (it must
    /// be `atom.id()`; the log checks that).
    pub fn push_with_id(&mut self, atom: &MemoryAtom, id: MemoryAtomId) -> Result<()> {
        self.check_next(atom)?;
        atom.validate()?;
        let entry = Entry::atom(id, atom.bbox(), atom.time_range());
        let items = self.frames.entry(atom.frame()).or_default();
        if atom.is_short_lived() {
            items
                .buckets
                .entry(atom.valid.start / BUCKET_NS)
                .or_default()
                .push(entry);
        } else {
            items.long_lived.push(entry);
        }
        if let Some((subject, predicate)) = atom.claim_key() {
            self.subjects
                .entry((subject.to_owned(), predicate.to_owned()))
                .or_default()
                .push(id);
        }
        for target in atom.link_targets() {
            self.links.entry(target).or_default().push(id);
        }
        self.size += 1;
        Ok(())
    }

    fn check_next(&self, atom: &MemoryAtom) -> Result<()> {
        if atom.seq != self.size {
            return Err(MemoryError::WrongSeq {
                expected: self.size,
                actual: atom.seq,
            });
        }
        Ok(())
    }

    /// Counts an atom without indexing it (for [`MemoryIndex::forge_omitting`]).
    fn skip(&mut self, atom: &MemoryAtom) -> Result<()> {
        self.check_next(atom)?;
        self.size += 1;
        Ok(())
    }

    /// Builds the index.
    pub fn finish(self) -> MemoryIndex {
        let mut frame_ids = Vec::with_capacity(self.frames.len());
        let mut frames = Vec::with_capacity(self.frames.len());
        for (frame, items) in self.frames {
            frame_ids.push(frame);
            frames.push(build_frame(items));
        }
        let frame_map = SortedMap::new(
            FRAME_MAP,
            frame_ids
                .into_iter()
                .zip(&frames)
                .map(|(id, frame)| (id, FRAME_MAP.value_digest(&frame.value)))
                .collect(),
        );
        let (subject_keys, subject_lists): (Vec<_>, Vec<_>) = self.subjects.into_iter().unzip();
        let subject_map = SortedMap::new(
            SUBJECT_MAP,
            subject_keys
                .into_iter()
                .zip(&subject_lists)
                .map(|(key, list)| (key, SUBJECT_MAP.value_digest(list)))
                .collect(),
        );
        let (link_keys, link_lists): (Vec<_>, Vec<_>) = self.links.into_iter().unzip();
        let link_map = SortedMap::new(
            LINK_MAP,
            link_keys
                .into_iter()
                .zip(&link_lists)
                .map(|(key, list)| (key, LINK_MAP.value_digest(list)))
                .collect(),
        );
        let header = IndexHeader {
            version: INDEX_VERSION,
            size: self.size,
            frames: frame_map.commitment(),
            subjects: subject_map.commitment(),
            links: link_map.commitment(),
        };
        MemoryIndex {
            root: header.root(),
            header,
            frames,
            frame_map,
            subject_lists,
            subject_map,
            link_lists,
            link_map,
        }
    }
}

fn build_frame(items: FrameItems) -> FrameIndex {
    let mut nodes = Vec::new();
    let mut roots = Vec::with_capacity(items.buckets.len());
    for (_, atoms) in items.buckets {
        roots.push(build_rtree(&mut nodes, atoms));
    }
    let forest = (!roots.is_empty()).then(|| build_top(&mut nodes, roots));
    let long_lived =
        (!items.long_lived.is_empty()).then(|| build_rtree(&mut nodes, items.long_lived));
    FrameIndex {
        value: FrameValue {
            forest: forest.map(|(entry, _)| entry),
            long_lived: long_lived.map(|(entry, _)| entry),
        },
        nodes,
        forest: forest.map(|(_, index)| index),
        long_lived: long_lived.map(|(_, index)| index),
    }
}

#[cfg(test)]
pub(crate) mod testing {
    //! Random logs for the index tests.

    use rand_core::RngCore;
    use sion_core::ids::AccountId;

    use crate::atom::{AtomClass, FrameId, LinkKind, MemoryAtom, MemoryKind, Statement, Value};
    use crate::belief::BeliefState;
    use crate::geom::{Box3, Interval};
    use crate::index::BUCKET_NS;

    /// The writer of test logs.
    pub(crate) fn writer() -> AccountId {
        AccountId::derive(b"index-test-writer")
    }

    /// The frames of test logs.
    pub(crate) fn frames() -> [FrameId; 2] {
        [FrameId::named("warehouse"), FrameId::named("dock")]
    }

    /// A random log of `n` atoms in a small world (coordinates 0..100, three
    /// buckets of time), so that boxes and intervals overlap often. A few
    /// atoms are spaceless, open-ended, revisions, conflicts or linked.
    pub(crate) fn random_log(seed: u64, n: usize) -> Vec<MemoryAtom> {
        let mut rng = sion_core::rng::seeded(seed);
        let mut atoms: Vec<MemoryAtom> = Vec::with_capacity(n);
        for seq in 0..n as u64 {
            let roll = rng.next_u32() % 100;
            let start = rng.next_u64() % (3 * BUCKET_NS);
            let valid = match rng.next_u32() % 10 {
                0 => Interval::starting(start),
                1 => Interval::closed(start, start + BUCKET_NS + rng.next_u64() % BUCKET_NS),
                _ => {
                    let bucket_end = (start / BUCKET_NS + 1) * BUCKET_NS;
                    Interval::closed(
                        start,
                        (start + 1 + rng.next_u64() % (BUCKET_NS / 4)).min(bucket_end),
                    )
                }
            };
            let (class, kind, statement) = if roll < 6 && !atoms.is_empty() {
                let target = atoms[(rng.next_u64() % atoms.len() as u64) as usize].id();
                (
                    AtomClass::Revision,
                    MemoryKind::Correction,
                    Statement::revision([target], BeliefState::Retracted, "test"),
                )
            } else if roll < 9 && atoms.len() >= 2 {
                let a = atoms[(rng.next_u64() % atoms.len() as u64) as usize].id();
                let b = atoms[(rng.next_u64() % atoms.len() as u64) as usize].id();
                if a == b {
                    (
                        AtomClass::Observation,
                        MemoryKind::Observation,
                        Statement::claim("s0", "p0", Value::Integer(i128::from(seq))),
                    )
                } else {
                    (
                        AtomClass::Conflict,
                        MemoryKind::WorldState,
                        Statement::conflict([a, b], crate::atom::ConflictKind::Existence),
                    )
                }
            } else {
                let subject = format!("s{}", rng.next_u32() % 4);
                let predicate = format!("p{}", rng.next_u32() % 2);
                (
                    AtomClass::Observation,
                    MemoryKind::ALL[(rng.next_u32() % 17) as usize],
                    Statement::claim(subject, predicate, Value::Integer(i128::from(seq))),
                )
            };
            let mut builder = MemoryAtom::builder(writer(), seq, kind, class, statement)
                .valid(valid)
                .observed_at(start);
            if rng.next_u32() % 8 != 0 {
                let frame = frames()[(rng.next_u32() % 2) as usize];
                let min = [
                    (rng.next_u64() % 100) as i64,
                    (rng.next_u64() % 100) as i64,
                    (rng.next_u64() % 10) as i64,
                ];
                let max = [
                    min[0] + (rng.next_u64() % 6) as i64,
                    min[1] + (rng.next_u64() % 6) as i64,
                    min[2] + (rng.next_u64() % 2) as i64,
                ];
                builder = builder.space(frame, Box3::new(min, max), 0);
            }
            if rng.next_u32() % 10 == 0 && !atoms.is_empty() {
                let target = atoms[(rng.next_u64() % atoms.len() as u64) as usize].id();
                builder = builder.link(LinkKind::Supports, target, 500);
            }
            atoms.push(builder.build().expect("test atoms are valid"));
        }
        atoms
    }
}

#[cfg(test)]
mod tests {
    use super::testing::random_log;
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn empty_index_has_empty_maps() {
        let index = MemoryIndex::empty();
        assert_eq!(index.size(), 0);
        assert_eq!(index.header().frames, MapCommitment::empty());
        assert_eq!(index.root(), index.header().root());
        assert_eq!(index_root(&[]).unwrap(), index.root());
        assert!(index.frames().is_empty());
        assert_eq!(index.node_count(), 0);
    }

    #[test]
    fn builds_frames_buckets_and_maps() {
        let atoms = random_log(7, 120);
        let index = MemoryIndex::build(&atoms).unwrap();
        assert_eq!(index.size(), 120);
        let filed: u64 = index
            .frames()
            .iter()
            .map(|frame| {
                let value = index.frame_value(frame).unwrap();
                value.forest.map_or(0, |e| e.count) + value.long_lived.map_or(0, |e| e.count)
            })
            .sum();
        assert_eq!(filed, 120, "every atom is in exactly one tree");
        for atom in &atoms {
            if let Some((s, p)) = atom.claim_key() {
                assert!(index.subject(s, p).contains(&atom.id()));
            }
            for target in atom.link_targets() {
                assert!(index.linkers(&target).contains(&atom.id()));
            }
        }
        assert!(index.subject("nobody", "nothing").is_empty());
        assert!(index.node_count() > 0);
        assert!(index.frame_value(&FrameId::named("elsewhere")).is_none());
    }

    #[test]
    fn rejects_out_of_order_atoms() {
        let atoms = random_log(1, 3);
        let mut builder = IndexBuilder::new();
        assert!(builder.is_empty());
        assert!(matches!(
            builder.push(&atoms[1]),
            Err(MemoryError::WrongSeq {
                expected: 0,
                actual: 1
            })
        ));
        builder.push(&atoms[0]).unwrap();
        assert_eq!(builder.len(), 1);
        let mut invalid = atoms[1].clone();
        invalid.confidence = 2_000;
        assert!(matches!(
            builder.push(&invalid),
            Err(MemoryError::InvalidAtom(_))
        ));
    }

    #[test]
    fn golden_root_pins_the_construction() {
        // A fixed log; if this root changes, the construction changed and
        // the specification (and every committed index root) changes with it.
        let atoms = random_log(2026, 40);
        let root = index_root(&atoms).unwrap();
        assert_eq!(root.to_hex(), GOLDEN_ROOT);
    }

    /// Index root of `random_log(2026, 40)`.
    const GOLDEN_ROOT: &str = "28f4f69c984a1d2bc467e70d26adea7fe5f38a40e23d6a7b0d34ddd0c16c8060";

    /// The specification's test vector: six atoms written out by hand, so an
    /// independent implementation can check its atom ids, log root and
    /// index root without this crate's test generators.
    pub(crate) fn spec_vector() -> Vec<MemoryAtom> {
        use crate::atom::{AtomClass, MemoryKind, Statement, Value};
        use crate::belief::BeliefState;
        use crate::geom::{Box3, Interval};
        use sion_core::ids::AccountId;

        let writer = AccountId::derive(b"sion-memory/spec-vector");
        let frame = FrameId::named("spec-frame");
        let claim = |seq: u64, subject: &str| {
            MemoryAtom::builder(
                writer,
                seq,
                MemoryKind::Observation,
                AtomClass::Observation,
                Statement::claim(subject, "at", Value::Integer(i128::from(seq))),
            )
        };
        let h = BUCKET_NS;
        let a0 = claim(0, "a")
            .valid(Interval::closed(10, 20))
            .at(frame, [0, 0, 0])
            .build()
            .unwrap();
        let a1 = claim(1, "b")
            .valid(Interval::closed(30, 40))
            .space(frame, Box3::new([5, 5, 5], [6, 7, 8]), 0)
            .build()
            .unwrap();
        let a2 = claim(2, "a")
            .valid(Interval::closed(h + 1, h + 2))
            .at(frame, [-3, 4, 0])
            .build()
            .unwrap();
        let a3 = claim(3, "c")
            .valid(Interval::starting(50))
            .at(frame, [100, 100, 100])
            .build()
            .unwrap();
        let a4 = claim(4, "d")
            .valid(Interval::closed(60, 2 * h))
            .build()
            .unwrap();
        let a5 = MemoryAtom::builder(
            writer,
            5,
            MemoryKind::Correction,
            AtomClass::Revision,
            Statement::revision([a0.id()], BeliefState::Retracted, "wrong"),
        )
        .valid(Interval::starting(70))
        .build()
        .unwrap();
        vec![a0, a1, a2, a3, a4, a5]
    }

    #[test]
    fn spec_vector_pins_ids_and_roots() {
        let atoms = spec_vector();
        let index = MemoryIndex::build(&atoms).unwrap();
        let leaves: Vec<Hash32> = atoms.iter().map(|a| a.leaf()).collect();
        let got = (
            atoms[0].id().to_string(),
            sion_core::merkle::root(&leaves).to_string(),
            index.root().to_string(),
        );
        let expected = (
            SPEC_ATOM0_ID.to_string(),
            SPEC_LOG_ROOT.to_string(),
            SPEC_INDEX_ROOT.to_string(),
        );
        assert_eq!(got, expected);
        // Its shape: the null frame sorts first; the frame has two buckets
        // and one long-lived atom; the spaceless atoms are long-lived.
        assert_eq!(
            index.frames(),
            &[FrameId::NULL, FrameId::named("spec-frame")]
        );
        let frame = index.frame_value(&FrameId::named("spec-frame")).unwrap();
        assert_eq!(frame.forest.map(|e| e.count), Some(3));
        assert_eq!(frame.long_lived.map(|e| e.count), Some(1));
        let null = index.frame_value(&FrameId::NULL).unwrap();
        assert_eq!(
            (null.forest, null.long_lived.map(|e| e.count)),
            (None, Some(2))
        );
        assert_eq!(index.subject("a", "at"), &[atoms[0].id(), atoms[2].id()]);
        assert_eq!(index.linkers(&atoms[0].id()), &[atoms[5].id()]);
    }

    const SPEC_ATOM0_ID: &str = "34c4886abeb06ae2650fe3f45bfe62e81fcd6c663c6a87d3488f68ee2acdc008";
    const SPEC_LOG_ROOT: &str = "3d1ffda53cafd7ef22656335616df03001258f5cf9fd7c026b07819422420755";
    const SPEC_INDEX_ROOT: &str =
        "d8d4d96c0c055a954bafe790dc5277510a6e7d9501181de3ff442ec0646e0f82";

    #[test]
    fn forged_index_claims_full_size() {
        let atoms = random_log(3, 30);
        let honest = MemoryIndex::build(&atoms).unwrap();
        let forged = MemoryIndex::forge_omitting(&atoms, &BTreeSet::from([4])).unwrap();
        assert_eq!(forged.size(), honest.size());
        assert_ne!(forged.root(), honest.root());
    }

    proptest! {
        #![proptest_config(crate::testing::config(64, 0x5eed_ca70))]

        #[test]
        fn index_is_canonical(seed in any::<u64>(), n in 0usize..80, cut in any::<usize>()) {
            let atoms = random_log(seed, n);
            let a = MemoryIndex::build(&atoms).unwrap();
            let b = MemoryIndex::build(&atoms).unwrap();
            prop_assert_eq!(a.root(), b.root());
            // Streaming with precomputed ids gives the same root.
            let mut builder = IndexBuilder::new();
            for atom in &atoms {
                builder.push_with_id(atom, atom.id()).unwrap();
            }
            prop_assert_eq!(builder.finish().root(), a.root());
            // The root at size m does not depend on later atoms.
            let m = if n == 0 { 0 } else { cut % (n + 1) };
            let prefix = MemoryIndex::build(&atoms[..m]).unwrap();
            let longer = random_log(seed, n + 5);
            prop_assert_eq!(MemoryIndex::build(&longer[..m]).unwrap().root(), prefix.root());
            if m < n {
                prop_assert_ne!(prefix.root(), a.root());
            }
        }
    }
}
