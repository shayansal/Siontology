//! Merkle R-trees packed by Sort-Tile-Recursive, and the tree over buckets.
//!
//! See the [`crate::index`] module documentation for the specification;
//! this module implements it.

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use sion_core::hash::{Hash32, Hasher};

use crate::atom::MemoryAtomId;
use crate::geom::{Box3, TimeRange};

/// Entries per node, at most.
pub const FANOUT: usize = 4;

/// Tag of a node whose entries are atoms.
pub const TAG_ATOMS: u8 = 0;
/// Tag of a node whose entries are other nodes.
pub const TAG_NODES: u8 = 1;

/// Domain of node digests.
pub(crate) const NODE_DOMAIN: &str = "sion/mem/index/node";

/// Bytes of one encoded [`Entry`].
pub(crate) const ENTRY_BYTES: usize = 104;

/// One entry of an index node: a digest and a summary of what it covers.
/// For an atom, the digest is the atom id, the box its box, the time its
/// valid interval and the count 1. For a node, the digest is the node's
/// digest, the box and time the unions over its entries, and the count the
/// sum of theirs.
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
pub struct Entry {
    /// Atom id or node digest.
    pub digest: Hash32,
    /// The box covered.
    pub bbox: Box3,
    /// The valid time covered.
    pub time: TimeRange,
    /// Atoms covered.
    pub count: u64,
}

impl Entry {
    /// The leaf entry of an atom.
    pub fn atom(id: MemoryAtomId, bbox: Box3, time: TimeRange) -> Entry {
        Entry {
            digest: id.0,
            bbox,
            time,
            count: 1,
        }
    }

    /// Whether the entry's box meets `region` and its time meets `time`.
    pub fn meets(&self, region: &Box3, time: &TimeRange) -> bool {
        self.bbox.intersects(region) && self.time.intersects(time)
    }

    /// The canonical (Borsh) encoding, without allocating.
    pub(crate) fn bytes(&self) -> [u8; ENTRY_BYTES] {
        let mut out = [0u8; ENTRY_BYTES];
        out[..32].copy_from_slice(&self.digest.0);
        for axis in 0..3 {
            let at = 32 + 8 * axis;
            out[at..at + 8].copy_from_slice(&self.bbox.min[axis].to_le_bytes());
            let at = 56 + 8 * axis;
            out[at..at + 8].copy_from_slice(&self.bbox.max[axis].to_le_bytes());
        }
        out[80..88].copy_from_slice(&self.time.start.to_le_bytes());
        out[88..96].copy_from_slice(&self.time.end.to_le_bytes());
        out[96..104].copy_from_slice(&self.count.to_le_bytes());
        out
    }
}

/// `H("sion/mem/index/node", borsh(tag: u8, entries: Vec<Entry>))`.
pub(crate) fn node_digest(tag: u8, entries: &[Entry]) -> Hash32 {
    let mut hasher = Hasher::new(NODE_DOMAIN);
    hasher.update(&[tag]);
    hasher.update(&(entries.len() as u32).to_le_bytes());
    for entry in entries {
        hasher.update(&entry.bytes());
    }
    hasher.finalize()
}

/// A node's own entry: its digest, the unions of its entries' boxes and
/// times, and the sum of their counts (`None` on overflow or no entries).
pub(crate) fn summarize(tag: u8, entries: &[Entry]) -> Option<Entry> {
    let first = entries.first()?;
    let mut bbox = first.bbox;
    let mut time = first.time;
    let mut count: u64 = 0;
    for entry in entries {
        bbox = bbox.union(&entry.bbox);
        time = time.union(&entry.time);
        count = count.checked_add(entry.count)?;
    }
    Some(Entry {
        digest: node_digest(tag, entries),
        bbox,
        time,
        count,
    })
}

/// A node in an arena.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Node {
    /// [`TAG_ATOMS`] or [`TAG_NODES`].
    pub(crate) tag: u8,
    /// One to [`FANOUT`] entries.
    pub(crate) entries: Vec<Entry>,
    /// For [`TAG_NODES`], the arena index of each entry's node.
    pub(crate) children: Vec<u32>,
}

/// Smallest `s >= 1` with `s³ >= p`.
pub(crate) fn cube_root_ceil(p: u64) -> u64 {
    let (mut lo, mut hi) = (1u64, 1u64 << 22);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if u128::from(mid).pow(3) >= u128::from(p) {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    lo
}

fn axis_key<T>(axis: usize) -> impl Fn(&(Entry, T)) -> (i128, Hash32) {
    move |(entry, _)| (entry.bbox.centre2(axis), entry.digest)
}

/// Sort-Tile-Recursive order: after it, consecutive runs of [`FANOUT`]
/// items are the nodes of the next level up.
pub(crate) fn str_sort<T>(items: &mut [(Entry, T)]) {
    let n = items.len() as u64;
    let pages = n.div_ceil(FANOUT as u64);
    let slices = cube_root_ceil(pages);
    let fanout = FANOUT as u64;
    let x_slab = (slices * slices * fanout) as usize;
    let y_slab = (slices * fanout) as usize;
    let by_x = axis_key::<T>(0);
    let by_y = axis_key::<T>(1);
    let by_z = axis_key::<T>(2);
    items.sort_unstable_by_key(|item| by_x(item));
    for slab in items.chunks_mut(x_slab) {
        slab.sort_unstable_by_key(|item| by_y(item));
        for column in slab.chunks_mut(y_slab) {
            column.sort_unstable_by_key(|item| by_z(item));
        }
    }
}

fn push_node(nodes: &mut Vec<Node>, tag: u8, group: &[(Entry, u32)]) -> (Entry, u32) {
    let entries: Vec<Entry> = group.iter().map(|(entry, _)| *entry).collect();
    let children = if tag == TAG_NODES {
        group.iter().map(|(_, child)| *child).collect()
    } else {
        Vec::new()
    };
    let summary = summarize(tag, &entries).expect("a node has entries and counts fit in u64");
    let index = u32::try_from(nodes.len()).expect("fewer than 2^32 nodes per frame");
    nodes.push(Node {
        tag,
        entries,
        children,
    });
    (summary, index)
}

/// Builds an R-tree over atom entries (at least one) into `nodes` and
/// returns its root entry and arena index. The atoms are packed into
/// [`TAG_ATOMS`] nodes by STR; while a level has more than one node, its
/// nodes are packed into [`TAG_NODES`] nodes by STR.
pub(crate) fn build_rtree(nodes: &mut Vec<Node>, atoms: Vec<Entry>) -> (Entry, u32) {
    assert!(!atoms.is_empty(), "an R-tree has at least one atom");
    let mut items: Vec<(Entry, u32)> = atoms.into_iter().map(|entry| (entry, u32::MAX)).collect();
    str_sort(&mut items);
    let mut level: Vec<(Entry, u32)> = items
        .chunks(FANOUT)
        .map(|group| push_node(nodes, TAG_ATOMS, group))
        .collect();
    while level.len() > 1 {
        str_sort(&mut level);
        level = level
            .chunks(FANOUT)
            .map(|group| push_node(nodes, TAG_NODES, group))
            .collect();
    }
    level[0]
}

/// Builds the tree over bucket roots (given in bucket order, at least one):
/// while more than one entry remains, consecutive runs of [`FANOUT`] become
/// [`TAG_NODES`] nodes. One bucket is its own root.
pub(crate) fn build_top(nodes: &mut Vec<Node>, roots: Vec<(Entry, u32)>) -> (Entry, u32) {
    assert!(!roots.is_empty(), "a forest has at least one bucket");
    let mut level = roots;
    while level.len() > 1 {
        level = level
            .chunks(FANOUT)
            .map(|group| push_node(nodes, TAG_NODES, group))
            .collect();
    }
    level[0]
}

#[cfg(test)]
mod tests {
    use super::*;
    use sion_core::encode::encode;
    use sion_core::hash::hash_domain;

    #[derive(BorshSerialize)]
    struct NodeBody {
        tag: u8,
        entries: Vec<Entry>,
    }

    fn entry(n: u8, point: [i64; 3]) -> Entry {
        Entry::atom(
            MemoryAtomId(Hash32([n; 32])),
            Box3::point(point),
            TimeRange::new(u64::from(n), u64::from(n) + 1),
        )
    }

    #[test]
    fn entry_bytes_are_borsh() {
        let e = Entry {
            digest: Hash32([7; 32]),
            bbox: Box3::new([-1, 2, -3], [4, i64::MAX, 6]),
            time: TimeRange::new(9, u64::MAX),
            count: 12,
        };
        assert_eq!(e.bytes().to_vec(), encode(&e));
        assert_eq!(encode(&e).len(), ENTRY_BYTES);
    }

    #[test]
    fn node_digest_is_domain_hash_of_borsh_body() {
        let entries = vec![entry(1, [0, 0, 0]), entry(2, [5, 5, 5])];
        let expected = hash_domain(
            NODE_DOMAIN,
            &encode(&NodeBody {
                tag: TAG_NODES,
                entries: entries.clone(),
            }),
        );
        assert_eq!(node_digest(TAG_NODES, &entries), expected);
        assert_ne!(
            node_digest(TAG_ATOMS, &entries),
            expected,
            "tags separate digests"
        );
    }

    #[test]
    fn summaries_union_and_sum() {
        let entries = vec![entry(1, [0, 0, 0]), entry(3, [5, -5, 9])];
        let s = summarize(TAG_ATOMS, &entries).unwrap();
        assert_eq!(s.bbox, Box3::new([0, -5, 0], [5, 0, 9]));
        assert_eq!(s.time, TimeRange::new(1, 4));
        assert_eq!(s.count, 2);
        assert!(summarize(TAG_ATOMS, &[]).is_none());
        let mut big = entries.clone();
        big[0].count = u64::MAX;
        assert!(summarize(TAG_NODES, &big).is_none());
        assert!(s.meets(&Box3::point([5, -5, 9]), &TimeRange::new(3, 4)));
        assert!(!s.meets(&Box3::point([6, 0, 0]), &TimeRange::ALL));
    }

    #[test]
    fn cube_roots_are_exact() {
        assert_eq!(cube_root_ceil(0), 1);
        assert_eq!(cube_root_ceil(1), 1);
        assert_eq!(cube_root_ceil(2), 2);
        assert_eq!(cube_root_ceil(8), 2);
        assert_eq!(cube_root_ceil(9), 3);
        assert_eq!(cube_root_ceil(1_000_000), 100);
        assert_eq!(cube_root_ceil(1_000_001), 101);
        assert_eq!(cube_root_ceil(u64::MAX), 2_642_246);
    }

    #[test]
    fn str_sort_tiles_by_x_then_y_then_z() {
        // 16 points on a 4×4 grid in x and y: pages = 4, slices = 2, x-slabs
        // of 16 (everything), y-slabs of 8.
        let mut items: Vec<(Entry, u32)> = (0..16u8)
            .map(|n| (entry(n, [i64::from(n % 4), i64::from(n / 4), 0]), 0))
            .collect();
        items.reverse();
        str_sort(&mut items);
        let ys: Vec<i64> = items.iter().map(|(e, _)| e.bbox.min[1]).collect();
        assert_eq!(&ys[..8], &[0, 0, 0, 0, 1, 1, 1, 1]);
        // Ties on every axis fall back to the digest.
        let mut ties: Vec<(Entry, u32)> =
            (0..5u8).rev().map(|n| (entry(n, [0, 0, 0]), 0)).collect();
        str_sort(&mut ties);
        let digests: Vec<u8> = ties.iter().map(|(e, _)| e.digest.0[0]).collect();
        assert_eq!(digests, vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn trees_are_independent_of_input_order() {
        let atoms: Vec<Entry> = (0..50u8)
            .map(|n| {
                entry(
                    n,
                    [
                        i64::from(n) * 7 % 13,
                        i64::from(n) * 3 % 11,
                        i64::from(n) % 5,
                    ],
                )
            })
            .collect();
        let mut a = Vec::new();
        let (root_a, _) = build_rtree(&mut a, atoms.clone());
        let mut reversed = atoms.clone();
        reversed.reverse();
        let mut b = Vec::new();
        let (root_b, _) = build_rtree(&mut b, reversed);
        assert_eq!(root_a, root_b);
        assert_eq!(root_a.count, 50);
        assert!(a
            .iter()
            .all(|node| !node.entries.is_empty() && node.entries.len() <= FANOUT));
    }

    #[test]
    fn single_atoms_and_single_buckets_have_node_roots() {
        let mut nodes = Vec::new();
        let (root, index) = build_rtree(&mut nodes, vec![entry(1, [1, 1, 1])]);
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[index as usize].tag, TAG_ATOMS);
        assert_eq!(root.digest, node_digest(TAG_ATOMS, &[entry(1, [1, 1, 1])]));
        let (top, top_index) = build_top(&mut nodes, vec![(root, index)]);
        assert_eq!((top, top_index), (root, index));
        let (other, other_index) = build_rtree(&mut nodes, vec![entry(2, [2, 2, 2])]);
        let (forest, forest_index) =
            build_top(&mut nodes, vec![(root, index), (other, other_index)]);
        assert_eq!(forest.count, 2);
        assert_eq!(
            nodes[forest_index as usize].children,
            vec![index, other_index]
        );
    }
}
