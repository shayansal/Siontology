//! A second implementation of the index root, written from the
//! specification in `sion_memory::index`'s documentation alone: raw bytes
//! and `sion_core`'s hash and Merkle primitives, none of this crate's index
//! code. It must agree with `sion_memory::index_root` on every log.

use std::collections::{BTreeMap, BTreeSet};

use proptest::prelude::*;
use proptest::test_runner::{Config, RngSeed};
use rand_core::RngCore;
use sion_core::encode::encode;
use sion_core::hash::{hash_domain, hash_parts, Hash32};
use sion_core::ids::AccountId;
use sion_core::merkle;
use sion_memory::{
    index_root, AtomClass, BeliefState, Box3, ConflictKind, FrameId, Interval, LinkKind,
    MemoryAtom, MemoryAtomId, MemoryKind, Statement, Value,
};

const B: u64 = 3_600 * 1_000_000_000;

#[derive(Clone, Copy)]
struct E {
    digest: [u8; 32],
    min: [i64; 3],
    max: [i64; 3],
    start: u64,
    end: u64,
    count: u64,
}

impl E {
    fn bytes(&self) -> Vec<u8> {
        let mut out = self.digest.to_vec();
        for v in self.min.iter().chain(&self.max) {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend_from_slice(&self.start.to_le_bytes());
        out.extend_from_slice(&self.end.to_le_bytes());
        out.extend_from_slice(&self.count.to_le_bytes());
        out
    }

    fn key(&self, axis: usize) -> (i128, [u8; 32]) {
        (
            i128::from(self.min[axis]) + i128::from(self.max[axis]),
            self.digest,
        )
    }
}

/// A node's own entry, per "Nodes".
fn node(tag: u8, entries: &[E]) -> E {
    let mut body = vec![tag];
    body.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    for e in entries {
        body.extend_from_slice(&e.bytes());
    }
    let mut out = E {
        digest: hash_domain("sion/mem/index/node", &body).0,
        min: entries[0].min,
        max: entries[0].max,
        start: entries[0].start,
        end: entries[0].end,
        count: 0,
    };
    for e in entries {
        for axis in 0..3 {
            out.min[axis] = out.min[axis].min(e.min[axis]);
            out.max[axis] = out.max[axis].max(e.max[axis]);
        }
        out.start = out.start.min(e.start);
        out.end = out.end.max(e.end);
        out.count += e.count;
    }
    out
}

/// "Packing: Sort-Tile-Recursive".
fn pack(mut entries: Vec<E>, tag: u8) -> Vec<E> {
    let n = entries.len() as u64;
    let pages = n.div_ceil(4);
    let mut s = 1u64;
    while s * s * s < pages {
        s += 1;
    }
    entries.sort_by_key(|e| e.key(0));
    for slab in entries.chunks_mut((s * s * 4) as usize) {
        slab.sort_by_key(|e| e.key(1));
        for column in slab.chunks_mut((s * 4) as usize) {
            column.sort_by_key(|e| e.key(2));
        }
    }
    entries.chunks(4).map(|run| node(tag, run)).collect()
}

fn rtree(atoms: Vec<E>) -> E {
    let mut level = pack(atoms, 0);
    while level.len() > 1 {
        level = pack(level, 1);
    }
    level[0]
}

fn option_entry(e: Option<E>) -> Vec<u8> {
    match e {
        None => vec![0],
        Some(e) => {
            let mut out = vec![1];
            out.extend_from_slice(&e.bytes());
            out
        }
    }
}

fn ids_bytes(ids: &[[u8; 32]]) -> Vec<u8> {
    let mut out = (ids.len() as u32).to_le_bytes().to_vec();
    for id in ids {
        out.extend_from_slice(id);
    }
    out
}

fn string_bytes(s: &str) -> Vec<u8> {
    let mut out = (s.len() as u32).to_le_bytes().to_vec();
    out.extend_from_slice(s.as_bytes());
    out
}

/// "Maps": the commitment `(count, MTH(leaves))` as header bytes.
fn map(domain: &str, entries: &[(Vec<u8>, Vec<u8>)]) -> Vec<u8> {
    let leaves: Vec<Hash32> = entries
        .iter()
        .map(|(key, value)| {
            let digest = hash_domain(&format!("sion/mem/index/{domain}/value"), value);
            hash_parts(&format!("sion/mem/index/{domain}/leaf"), &[key, &digest.0])
        })
        .collect();
    let mut out = (entries.len() as u64).to_le_bytes().to_vec();
    out.extend_from_slice(&merkle::root(&leaves).0);
    out
}

/// A frame's short-lived atoms by bucket, and its long-lived atoms.
type Frame = (BTreeMap<u64, Vec<E>>, Vec<E>);

fn reference_root(atoms: &[MemoryAtom]) -> Hash32 {
    let mut frames: BTreeMap<[u8; 32], Frame> = BTreeMap::new();
    let mut subjects: BTreeMap<(String, String), Vec<[u8; 32]>> = BTreeMap::new();
    let mut links: BTreeMap<[u8; 32], Vec<[u8; 32]>> = BTreeMap::new();
    for atom in atoms {
        let id = hash_domain("sion/mem/atom", &encode(atom)).0;
        let (frame, min, max) = match atom.space {
            Some(space) => (space.frame.0 .0, space.bbox.min, space.bbox.max),
            None => ([0u8; 32], [0; 3], [0; 3]),
        };
        let entry = E {
            digest: id,
            min,
            max,
            start: atom.valid.start,
            end: atom.valid.end.unwrap_or(u64::MAX),
            count: 1,
        };
        let slot = frames.entry(frame).or_default();
        match atom.valid.end {
            Some(end) if atom.valid.start / B == (end - 1) / B => {
                slot.0.entry(atom.valid.start / B).or_default().push(entry)
            }
            _ => slot.1.push(entry),
        }
        let mut named: BTreeSet<[u8; 32]> = atom.links.iter().map(|l| l.target.0 .0).collect();
        match &atom.statement {
            Statement::Claim {
                subject, predicate, ..
            } => subjects
                .entry((subject.clone(), predicate.clone()))
                .or_default()
                .push(id),
            Statement::Revision { targets, .. } => named.extend(targets.iter().map(|t| t.0 .0)),
            Statement::Conflict { members, .. } => named.extend(members.iter().map(|m| m.0 .0)),
            _ => {}
        }
        for target in named {
            links.entry(target).or_default().push(id);
        }
    }
    let frame_entries: Vec<(Vec<u8>, Vec<u8>)> = frames
        .into_iter()
        .map(|(frame, (buckets, long))| {
            let mut roots: Vec<E> = buckets.into_values().map(rtree).collect();
            let forest = if roots.is_empty() {
                None
            } else {
                while roots.len() > 1 {
                    roots = roots.chunks(4).map(|run| node(1, run)).collect();
                }
                Some(roots[0])
            };
            let long_lived = (!long.is_empty()).then(|| rtree(long));
            let mut value = option_entry(forest);
            value.extend_from_slice(&option_entry(long_lived));
            (frame.to_vec(), value)
        })
        .collect();
    let subject_entries: Vec<(Vec<u8>, Vec<u8>)> = subjects
        .into_iter()
        .map(|((s, p), ids)| {
            let mut key = string_bytes(&s);
            key.extend_from_slice(&string_bytes(&p));
            (key, ids_bytes(&ids))
        })
        .collect();
    let link_entries: Vec<(Vec<u8>, Vec<u8>)> = links
        .into_iter()
        .map(|(target, ids)| (target.to_vec(), ids_bytes(&ids)))
        .collect();
    let mut header = vec![1u8];
    header.extend_from_slice(&(atoms.len() as u64).to_le_bytes());
    header.extend_from_slice(&map("frame", &frame_entries));
    header.extend_from_slice(&map("subject", &subject_entries));
    header.extend_from_slice(&map("link", &link_entries));
    hash_domain("sion/mem/index", &header)
}

fn pick(rng: &mut impl RngCore, atoms: &[MemoryAtom]) -> MemoryAtomId {
    atoms[(rng.next_u64() % atoms.len() as u64) as usize].id()
}

fn coordinate(rng: &mut impl RngCore) -> i64 {
    (rng.next_u64() % 400) as i64 - 200
}

fn random_log(seed: u64, n: usize) -> Vec<MemoryAtom> {
    let writer = AccountId::derive(b"spec-reference");
    let frames = [
        FrameId::named("a"),
        FrameId::named("b"),
        FrameId::named("c"),
    ];
    let mut rng = sion_core::rng::seeded(seed);
    let mut atoms: Vec<MemoryAtom> = Vec::new();
    for seq in 0..n as u64 {
        let start = rng.next_u64() % (4 * B);
        let valid = match rng.next_u32() % 6 {
            0 => Interval::starting(start),
            1 => Interval::closed(start, start + 1 + rng.next_u64() % (2 * B)),
            _ => Interval::closed(start, start + 1 + rng.next_u64() % 1_000),
        };
        let roll = rng.next_u32() % 10;
        let (class, statement) = if roll == 0 && !atoms.is_empty() {
            let targets = [pick(&mut rng, &atoms), pick(&mut rng, &atoms)];
            (
                AtomClass::Revision,
                Statement::revision(targets, BeliefState::Refuted, "r"),
            )
        } else if roll == 1 && atoms.len() > 1 {
            let a = pick(&mut rng, &atoms);
            let mut b = pick(&mut rng, &atoms);
            if a == b {
                b = MemoryAtomId(Hash32([7; 32]));
            }
            (
                AtomClass::Conflict,
                Statement::conflict([a, b], ConflictKind::Geometric),
            )
        } else {
            (
                AtomClass::Observation,
                Statement::claim(
                    format!("subject-{}", rng.next_u32() % 5),
                    ["at", "is", "colour"][(rng.next_u32() % 3) as usize],
                    Value::Integer(i128::from(seq)),
                ),
            )
        };
        let mut builder =
            MemoryAtom::builder(writer, seq, MemoryKind::Observation, class, statement)
                .valid(valid);
        if rng.next_u32() % 5 != 0 {
            let min = [
                coordinate(&mut rng),
                coordinate(&mut rng),
                coordinate(&mut rng),
            ];
            let size = (rng.next_u64() % 30) as i64;
            builder = builder.space(
                frames[(rng.next_u32() % 3) as usize],
                Box3::new(min, [min[0] + size, min[1] + size / 2, min[2]]),
                0,
            );
        }
        if rng.next_u32() % 4 == 0 && !atoms.is_empty() {
            builder = builder.link(LinkKind::DerivedFrom, pick(&mut rng, &atoms), 1_000);
        }
        atoms.push(builder.build().unwrap());
    }
    atoms
}

#[test]
fn reference_agrees_on_edge_cases() {
    assert_eq!(reference_root(&[]), index_root(&[]).unwrap());
    for n in [1, 2, 3, 4, 5, 15, 16, 17, 63, 64, 65, 300] {
        let atoms = random_log(n as u64, n);
        assert_eq!(
            reference_root(&atoms),
            index_root(&atoms).unwrap(),
            "log of {n}"
        );
    }
}

#[test]
fn reference_agrees_on_a_big_log() {
    // Thousands of atoms in one frame and bucket give trees several levels deep.
    let writer = AccountId::derive(b"spec-reference-big");
    let frame = FrameId::named("big");
    let mut rng = sion_core::rng::seeded(99);
    let atoms: Vec<MemoryAtom> = (0..3_000u64)
        .map(|seq| {
            let start = rng.next_u64() % (B / 2);
            MemoryAtom::builder(
                writer,
                seq,
                MemoryKind::Pose,
                AtomClass::Observation,
                Statement::claim("robot", "pose", Value::Integer(i128::from(seq))),
            )
            .valid(Interval::closed(start, start + 1))
            .at(
                frame,
                [
                    (rng.next_u64() % 1_000) as i64,
                    (rng.next_u64() % 1_000) as i64,
                    (rng.next_u64() % 3) as i64,
                ],
            )
            .build()
            .unwrap()
        })
        .collect();
    assert_eq!(reference_root(&atoms), index_root(&atoms).unwrap());
}

proptest! {
    #![proptest_config(Config { cases: 64, rng_seed: RngSeed::Fixed(0x5eed_5bec), ..Config::default() })]

    #[test]
    fn reference_agrees_on_random_logs(seed in any::<u64>(), n in 0usize..200) {
        let atoms = random_log(seed, n);
        prop_assert_eq!(reference_root(&atoms), index_root(&atoms).unwrap());
    }
}
