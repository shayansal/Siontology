//! Shared world cells: a building block for curated memory of the world.
//!
//! This version grids a **named frame** (a warehouse map, say), not the
//! Earth: a cell at level `L` is a cube of edge `10 m × 2^L` aligned to the
//! frame's origin, `L` from 0 to [`MAX_CELL_LEVEL`]. In millimetres, with
//! `e = 10,000 << L`, the cell of a point `p` has coordinates
//! `floor(p / e)` on each axis (Euclidean division, so negative coordinates
//! fall into negative cells), and covers `[c·e, (c+1)·e − 1]` on each axis.
//! A cell's id is `H("sion/mem/cell", borsh(CellKey))`.
//!
//! An atom is **tagged** to a cell when it has space in the cell's frame and
//! its box meets the cell. A curator publishes a [`CellDigest`] for a cell:
//! the heads it read and a sorted Merkle list of `(writer, atom id)` for the
//! tagged atoms in those heads' prefixes (map domains
//! `sion/mem/cell/leaf` and `sion/mem/cell/value`, value `()`). An omission
//! is proved by the atom's inclusion in a writer's log at the head the
//! digest lists, its tag (recomputed from its box), and the absence of
//! `(writer, atom id)` from the list.
//!
//! Valid time and epochs are not part of cells yet, and neither is the
//! Earth-centred grid; both are for a later version.

use std::collections::BTreeSet;

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use sion_core::encode::encode;
use sion_core::hash::{hash_domain, Hash32};
use sion_core::ids::{AccountId, Pulse};
use sion_core::merkle::verify_inclusion;

use crate::atom::{FrameId, MemoryAtom, MemoryAtomId};
use crate::error::{MemoryError, Result};
use crate::geom::Box3;
use crate::index::map::{MapCommitment, MapDomains, MapLookup, SortedMap};
use crate::log::{Head, WriterLog};

/// Edge of a level-0 cell, in millimetres (10 m).
pub const CELL_EDGE_MM: i64 = 10_000;
/// Coarsest level: edges of 10 m × 2^20, about 10,500 km.
pub const MAX_CELL_LEVEL: u8 = 20;
/// Most cells [`CellKey::covering`] returns for one box.
pub const MAX_CELLS_PER_BOX: usize = 4_096;

const CELL_DOMAIN: &str = "sion/mem/cell";
const DIGEST_DOMAIN: &str = "sion/mem/cell/digest";
const CELL_MAP: MapDomains = MapDomains {
    leaf: "sion/mem/cell/leaf",
    value: "sion/mem/cell/value",
};

/// A cell of a frame's grid.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    BorshSerialize,
    BorshDeserialize,
    Serialize,
    Deserialize,
)]
pub struct CellKey {
    /// The frame.
    pub frame: FrameId,
    /// The level (edge `10 m × 2^level`).
    pub level: u8,
    /// Cell coordinates on each axis.
    pub coords: [i64; 3],
}

impl CellKey {
    /// The edge of a cell at `level`, in millimetres.
    pub fn edge_mm(level: u8) -> Result<i64> {
        if level > MAX_CELL_LEVEL {
            return Err(MemoryError::InvalidQuery("cell level too coarse"));
        }
        Ok(CELL_EDGE_MM << level)
    }

    /// The cell holding `point`.
    pub fn containing(frame: FrameId, level: u8, point: [i64; 3]) -> Result<CellKey> {
        let edge = Self::edge_mm(level)?;
        Ok(CellKey {
            frame,
            level,
            coords: point.map(|v| v.div_euclid(edge)),
        })
    }

    /// The cells a box meets (at most [`MAX_CELLS_PER_BOX`]), in coordinate order.
    pub fn covering(frame: FrameId, level: u8, bbox: &Box3) -> Result<Vec<CellKey>> {
        if !bbox.is_valid() {
            return Err(MemoryError::InvalidQuery("box has min greater than max"));
        }
        let low = Self::containing(frame, level, bbox.min)?.coords;
        let high = Self::containing(frame, level, bbox.max)?.coords;
        let span = |axis: usize| i128::from(high[axis]) - i128::from(low[axis]) + 1;
        if span(0) * span(1) * span(2) > MAX_CELLS_PER_BOX as i128 {
            return Err(MemoryError::InvalidQuery("box covers too many cells"));
        }
        let mut cells = Vec::new();
        for x in low[0]..=high[0] {
            for y in low[1]..=high[1] {
                for z in low[2]..=high[2] {
                    cells.push(CellKey {
                        frame,
                        level,
                        coords: [x, y, z],
                    });
                }
            }
        }
        Ok(cells)
    }

    /// The box the cell covers (saturating at the coordinate limits).
    pub fn bounds(&self) -> Result<Box3> {
        let edge = Self::edge_mm(self.level)?;
        let mut min = [0i64; 3];
        let mut max = [0i64; 3];
        for axis in 0..3 {
            let low = i128::from(self.coords[axis]) * i128::from(edge);
            let high = low + i128::from(edge) - 1;
            min[axis] = low.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64;
            max[axis] = high.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64;
        }
        Ok(Box3::new(min, max))
    }

    /// The cell's id.
    pub fn id(&self) -> Hash32 {
        hash_domain(CELL_DOMAIN, &encode(self))
    }

    /// Whether `atom` is tagged to this cell.
    pub fn tags(&self, atom: &MemoryAtom) -> Result<bool> {
        let bounds = self.bounds()?;
        Ok(atom
            .space
            .is_some_and(|space| space.frame == self.frame && space.bbox.intersects(&bounds)))
    }
}

/// A curator's published digest of a cell.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct CellDigest {
    /// The cell.
    pub cell: CellKey,
    /// The Pulse the heads were read as of.
    pub pulse: Pulse,
    /// The writers' heads read, by writer.
    pub heads: Vec<(AccountId, Head)>,
    /// The sorted list of `(writer, atom id)` tagged to the cell.
    pub members: MapCommitment,
}

impl CellDigest {
    /// The digest's id: `H("sion/mem/cell/digest", borsh(self))`.
    pub fn id(&self) -> Hash32 {
        hash_domain(DIGEST_DOMAIN, &encode(self))
    }

    fn head_of(&self, writer: &AccountId) -> Option<&Head> {
        self.heads
            .binary_search_by_key(writer, |(w, _)| *w)
            .ok()
            .map(|i| &self.heads[i].1)
    }
}

/// A curated cell: the digest and the list behind it.
#[derive(Clone, Debug)]
pub struct CuratedCell {
    digest: CellDigest,
    members: Vec<(AccountId, MemoryAtomId)>,
    map: SortedMap<(AccountId, MemoryAtomId)>,
}

impl CuratedCell {
    /// What an honest curator publishes: every atom tagged to `cell` in the
    /// first `size` atoms of each log.
    pub fn build(cell: CellKey, pulse: Pulse, logs: &[(&WriterLog, u64)]) -> Result<CuratedCell> {
        let mut heads = Vec::with_capacity(logs.len());
        let mut members = BTreeSet::new();
        for (log, size) in logs {
            heads.push((log.writer(), log.head(*size)?));
            for (atom, id) in log.atoms()[..*size as usize].iter().zip(log.ids()) {
                if cell.tags(atom)? {
                    members.insert((log.writer(), *id));
                }
            }
        }
        Self::from_members(cell, pulse, heads, members)
    }

    /// A digest over any list (an honest one comes from [`CuratedCell::build`]).
    pub fn from_members(
        cell: CellKey,
        pulse: Pulse,
        mut heads: Vec<(AccountId, Head)>,
        members: BTreeSet<(AccountId, MemoryAtomId)>,
    ) -> Result<CuratedCell> {
        heads.sort_by_key(|(writer, _)| *writer);
        if heads.windows(2).any(|pair| pair[0].0 == pair[1].0) {
            return Err(MemoryError::InvalidQuery("a writer appears twice"));
        }
        let members: Vec<(AccountId, MemoryAtomId)> = members.into_iter().collect();
        let unit = CELL_MAP.value_digest(&());
        let map = SortedMap::new(CELL_MAP, members.iter().map(|m| (*m, unit)).collect());
        let digest = CellDigest {
            cell,
            pulse,
            heads,
            members: map.commitment(),
        };
        Ok(CuratedCell {
            digest,
            members,
            map,
        })
    }

    /// The published digest.
    pub fn digest(&self) -> &CellDigest {
        &self.digest
    }

    /// The listed `(writer, atom id)` pairs.
    pub fn members(&self) -> &[(AccountId, MemoryAtomId)] {
        &self.members
    }

    /// A lookup of one `(writer, atom id)` in the list.
    pub fn lookup(
        &self,
        writer: AccountId,
        id: MemoryAtomId,
    ) -> MapLookup<(AccountId, MemoryAtomId)> {
        self.map.lookup(&(writer, id))
    }
}

/// Evidence that a curator's digest omits a tagged atom.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct CellOmissionProof {
    /// The omitted atom.
    pub atom: MemoryAtom,
    /// Its inclusion in its writer's log at the head the digest lists.
    pub inclusion: Vec<Hash32>,
    /// The absence of `(writer, atom id)` from the digest's list.
    pub absence: MapLookup<(AccountId, MemoryAtomId)>,
}

/// Proves that `curated` omits the atom at `seq` of `log`, if it does.
pub fn prove_cell_omission(
    curated: &CuratedCell,
    log: &WriterLog,
    seq: u64,
) -> Result<Option<CellOmissionProof>> {
    let digest = curated.digest();
    let head = digest
        .head_of(&log.writer())
        .ok_or(MemoryError::InvalidQuery(
            "the digest did not read this writer",
        ))?;
    if seq >= head.size {
        return Err(MemoryError::OutOfRange {
            index: seq,
            size: head.size,
        });
    }
    let atom = log.atom(seq).ok_or(MemoryError::OutOfRange {
        index: seq,
        size: log.len(),
    })?;
    if !digest.cell.tags(atom)? {
        return Ok(None);
    }
    let id = atom.id();
    let absence = curated.lookup(atom.writer, id);
    if absence.found {
        return Ok(None);
    }
    Ok(Some(CellOmissionProof {
        atom: atom.clone(),
        inclusion: log.inclusion_proof_at(seq, head.size)?,
        absence,
    }))
}

/// Checks a cell omission proof: `Ok(())` means the digest provably leaves
/// out an atom tagged to its cell from a head it claims to have read.
pub fn verify_cell_omission(digest: &CellDigest, proof: &CellOmissionProof) -> Result<()> {
    let atom = &proof.atom;
    atom.validate()?;
    let head = digest
        .head_of(&atom.writer)
        .ok_or(MemoryError::InvalidProof(
            "the digest did not read this writer",
        ))?;
    if atom.seq >= head.size {
        return Err(MemoryError::InvalidProof(
            "atom beyond the head the digest read",
        ));
    }
    let id = atom.id();
    verify_inclusion(
        &head.root,
        head.size,
        atom.seq,
        &id.leaf(),
        &proof.inclusion,
    )?;
    if !digest.cell.tags(atom)? {
        return Err(MemoryError::InvalidProof("atom is not tagged to the cell"));
    }
    let present = proof
        .absence
        .verify(&CELL_MAP, &digest.members, &(atom.writer, id), None)?;
    if present {
        return Err(MemoryError::InvalidProof("the digest lists the atom"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atom::{AtomClass, MemoryKind, Statement, Value};
    use crate::geom::Interval;

    fn frame() -> FrameId {
        FrameId::named("warehouse")
    }

    fn robot_log(name: &str, points: &[[i64; 3]]) -> WriterLog {
        let writer = AccountId::derive(name.as_bytes());
        let mut log = WriterLog::new(writer);
        for (seq, point) in points.iter().enumerate() {
            log.append(
                MemoryAtom::builder(
                    writer,
                    seq as u64,
                    MemoryKind::Observation,
                    AtomClass::Observation,
                    Statement::claim(format!("pallet-{seq}"), "at", Value::Boolean(true)),
                )
                .valid(Interval::starting(0))
                .at(frame(), *point)
                .build()
                .unwrap(),
            )
            .unwrap();
        }
        log
    }

    #[test]
    fn cells_grid_with_integer_arithmetic() {
        let cell = CellKey::containing(frame(), 0, [9_999, 10_000, -1]).unwrap();
        assert_eq!(cell.coords, [0, 1, -1]);
        assert_eq!(
            cell.bounds().unwrap(),
            Box3::new([0, 10_000, -10_000], [9_999, 19_999, -1])
        );
        assert_eq!(CellKey::edge_mm(3).unwrap(), 80_000);
        assert!(CellKey::edge_mm(MAX_CELL_LEVEL + 1).is_err());
        let cells = CellKey::covering(frame(), 0, &Box3::new([0, 0, 0], [10_000, 0, 0])).unwrap();
        assert_eq!(cells.len(), 2);
        assert!(CellKey::covering(
            frame(),
            0,
            &Box3::new([0, 0, 0], [1_000_000_000, 1_000_000_000, 0])
        )
        .is_err());
        assert!(CellKey::covering(frame(), 0, &Box3::new([1, 0, 0], [0, 0, 0])).is_err());
        assert_ne!(
            cell.id(),
            CellKey::containing(frame(), 1, [0, 0, 0]).unwrap().id()
        );
        let far = CellKey {
            frame: frame(),
            level: 20,
            coords: [i64::MAX, i64::MIN, 0],
        };
        assert!(far.bounds().unwrap().is_valid());
    }

    #[test]
    fn honest_digests_list_every_tagged_atom() {
        let a = robot_log(
            "robot-a",
            &[[1_000, 1_000, 0], [50_000, 0, 0], [2_000, 3_000, 500]],
        );
        let b = robot_log("robot-b", &[[9_000, 9_000, 0]]);
        let cell = CellKey::containing(frame(), 0, [0, 0, 0]).unwrap();
        let curated = CuratedCell::build(cell, Pulse(10), &[(&a, 3), (&b, 1)]).unwrap();
        assert_eq!(curated.members().len(), 3);
        assert_eq!(curated.digest().heads.len(), 2);
        for (log, seq) in [(&a, 0), (&a, 1), (&a, 2), (&b, 0)] {
            assert!(prove_cell_omission(&curated, log, seq).unwrap().is_none());
        }
        assert_ne!(curated.digest().id(), Hash32::ZERO);
        assert!(CuratedCell::build(cell, Pulse(1), &[(&a, 3), (&a, 3)]).is_err());
    }

    #[test]
    fn omitted_atoms_are_proved() {
        let a = robot_log("robot-a", &[[1_000, 1_000, 0], [2_000, 3_000, 500]]);
        let cell = CellKey::containing(frame(), 0, [0, 0, 0]).unwrap();
        let honest = CuratedCell::build(cell, Pulse(3), &[(&a, 2)]).unwrap();
        // A curator that drops the second atom.
        let mut members: BTreeSet<_> = honest.members().iter().copied().collect();
        members.remove(&(a.writer(), a.ids()[1]));
        let dishonest =
            CuratedCell::from_members(cell, Pulse(3), honest.digest().heads.clone(), members)
                .unwrap();
        let proof = prove_cell_omission(&dishonest, &a, 1)
            .unwrap()
            .expect("omission");
        verify_cell_omission(dishonest.digest(), &proof).unwrap();
        // The same proof fails against the honest digest.
        assert!(verify_cell_omission(honest.digest(), &proof).is_err());
        // An untagged atom cannot be claimed omitted.
        let elsewhere = CellKey::containing(frame(), 0, [100_000, 0, 0]).unwrap();
        let other = CuratedCell::from_members(
            elsewhere,
            Pulse(3),
            honest.digest().heads.clone(),
            BTreeSet::new(),
        )
        .unwrap();
        assert!(prove_cell_omission(&other, &a, 1).unwrap().is_none());
        let mut forged = proof.clone();
        forged.atom.seq = 0;
        assert!(verify_cell_omission(dishonest.digest(), &forged).is_err());
        assert!(prove_cell_omission(&dishonest, &a, 5).is_err());
        let stranger = robot_log("robot-c", &[[0, 0, 0]]);
        assert!(prove_cell_omission(&dishonest, &stranger, 0).is_err());
    }
}
