//! Fraud proofs that an index leaves out an atom of its log.
//!
//! A writer's head binds a log root and an index root over the same size.
//! Because the index is canonical, an index that omits or misfiles an atom
//! is wrong, and anyone holding the atom can prove it compactly: the atom
//! with its inclusion proof in the log, plus one of
//!
//! - **Subject**: the complete subject list for the atom's
//!   `(subject, predicate)`, proved by its map lookup (the adjacent entries
//!   bracket the key, or the key's own entry lists other atoms), without the
//!   atom;
//! - **Link**: the complete link list for one of the atoms it names, without it;
//! - **Space**: a complete range answer for the atom's own frame, box and
//!   interval ([`RangeQuery::around`]) that does not return it;
//! - **Size**: an index header whose size differs from the head's.
//!
//! What is built and what is not: the subject and link maps get bracketing
//! proofs; the R-trees get the complete-range-query proof, which the brief
//! allowed in place of a bracketing proof inside an R-tree. Proofs that an
//! index node's summary is inconsistent, that a map is unsorted, or that a
//! complete index is packed in a non-canonical order are not built here:
//! the verifiers reject such indexes' proofs, and a full rebuild from the
//! log exposes them.

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use sion_core::hash::Hash32;
use sion_core::merkle::verify_inclusion;

use crate::atom::{MemoryAtom, MemoryAtomId};
use crate::error::{MemoryError, Result};
use crate::index::keyed::{
    prove_links, prove_subject, verify_links, verify_subject, LinkProof, SubjectProof,
};
use crate::index::range::{prove_range, verify_range, AtomEntry, RangeProof, RangeQuery};
use crate::index::{IndexHeader, MemoryIndex};
use crate::log::{Head, WriterLog};

/// Where the index fails to file the atom.
// A proof container, built and sent rarely: boxing the large variant would
// only complicate the serialized type.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OmissionEvidence {
    /// The index covers a different number of atoms than the head.
    Size {
        /// The index's header.
        header: IndexHeader,
    },
    /// The complete list for the atom's subject and predicate leaves it out.
    Subject {
        /// The list.
        ids: Vec<MemoryAtomId>,
        /// Its proof.
        proof: SubjectProof,
    },
    /// The complete list of atoms naming `target` leaves it out.
    Link {
        /// An atom the omitted atom names.
        target: MemoryAtomId,
        /// The list.
        ids: Vec<MemoryAtomId>,
        /// Its proof.
        proof: LinkProof,
    },
    /// A complete answer for the atom's own box and interval leaves it out.
    Space {
        /// The answer.
        results: Vec<AtomEntry>,
        /// Its proof.
        proof: RangeProof,
    },
}

/// Evidence that an index root omits an atom of its log.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct OmissionProof {
    /// The omitted atom.
    pub atom: MemoryAtom,
    /// Its inclusion proof in the log at the head's size.
    pub inclusion: Vec<Hash32>,
    /// Where the index fails.
    pub evidence: OmissionEvidence,
}

/// Looks for a place where `index` (claimed for the first `size` atoms of
/// `log`) fails to file the atom at `seq`; returns a proof if it finds one.
pub fn prove_omission(
    log: &WriterLog,
    size: u64,
    index: &MemoryIndex,
    seq: u64,
) -> Result<Option<OmissionProof>> {
    if seq >= size {
        return Err(MemoryError::OutOfRange { index: seq, size });
    }
    let atom = log
        .atom(seq)
        .ok_or(MemoryError::OutOfRange {
            index: seq,
            size: log.len(),
        })?
        .clone();
    let inclusion = log.inclusion_proof_at(seq, size)?;
    let id = atom.id();
    let evidence = if index.size() != size {
        Some(OmissionEvidence::Size {
            header: *index.header(),
        })
    } else {
        find_gap(index, &atom, &id)?
    };
    Ok(evidence.map(|evidence| OmissionProof {
        atom,
        inclusion,
        evidence,
    }))
}

fn find_gap(
    index: &MemoryIndex,
    atom: &MemoryAtom,
    id: &MemoryAtomId,
) -> Result<Option<OmissionEvidence>> {
    if let Some((subject, predicate)) = atom.claim_key() {
        let (ids, proof) = prove_subject(index, subject, predicate);
        if !ids.contains(id) {
            return Ok(Some(OmissionEvidence::Subject { ids, proof }));
        }
    }
    for target in atom.link_targets() {
        let (ids, proof) = prove_links(index, &target);
        if !ids.contains(id) {
            return Ok(Some(OmissionEvidence::Link { target, ids, proof }));
        }
    }
    let (results, proof) = prove_range(index, &RangeQuery::around(atom))?;
    if !results.iter().any(|r| r.id == *id) {
        return Ok(Some(OmissionEvidence::Space { results, proof }));
    }
    Ok(None)
}

/// Checks an omission proof against a writer's head: `Ok(())` means the
/// head's index root provably fails to file an atom of the head's log, so
/// the head is fraudulent.
pub fn verify_omission(head: &Head, proof: &OmissionProof) -> Result<()> {
    let atom = &proof.atom;
    atom.validate()?;
    if atom.seq >= head.size {
        return Err(MemoryError::InvalidProof("atom beyond the head"));
    }
    let id = atom.id();
    verify_inclusion(
        &head.root,
        head.size,
        atom.seq,
        &id.leaf(),
        &proof.inclusion,
    )?;
    let covers = |header: &IndexHeader| -> Result<()> {
        if header.size != head.size {
            return Err(MemoryError::InvalidProof(
                "index size differs: use the size evidence",
            ));
        }
        Ok(())
    };
    match &proof.evidence {
        OmissionEvidence::Size { header } => {
            header.check(&head.index_root)?;
            if header.size == head.size {
                return Err(MemoryError::InvalidProof("index size matches the head"));
            }
        }
        OmissionEvidence::Subject {
            ids,
            proof: subject,
        } => {
            let (s, p) = atom
                .claim_key()
                .ok_or(MemoryError::InvalidProof("atom is not a claim"))?;
            verify_subject(&head.index_root, s, p, ids, subject)?;
            covers(&subject.header)?;
            if ids.contains(&id) {
                return Err(MemoryError::InvalidProof("the subject list holds the atom"));
            }
        }
        OmissionEvidence::Link {
            target,
            ids,
            proof: links,
        } => {
            if !atom.link_targets().contains(target) {
                return Err(MemoryError::InvalidProof("atom does not name the target"));
            }
            verify_links(&head.index_root, target, ids, links)?;
            covers(&links.header)?;
            if ids.contains(&id) {
                return Err(MemoryError::InvalidProof("the link list holds the atom"));
            }
        }
        OmissionEvidence::Space {
            results,
            proof: range,
        } => {
            verify_range(&head.index_root, &RangeQuery::around(atom), results, range)?;
            covers(&range.header)?;
            if results.iter().any(|r| r.id == id) {
                return Err(MemoryError::InvalidProof("the range answer holds the atom"));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::testing::random_log;
    use proptest::prelude::*;
    use std::collections::BTreeSet;

    fn log_of(atoms: &[MemoryAtom]) -> WriterLog {
        let mut log = WriterLog::new(atoms[0].writer);
        for atom in atoms {
            log.append(atom.clone()).unwrap();
        }
        log
    }

    fn forged_head(log: &WriterLog, index: &MemoryIndex) -> Head {
        Head {
            size: log.len(),
            root: log.root(),
            index_root: index.root(),
        }
    }

    #[test]
    fn honest_indexes_admit_no_omission() {
        let atoms = random_log(41, 80);
        let log = log_of(&atoms);
        let index = log.index(80).unwrap();
        let head = log.head(80).unwrap();
        for seq in 0..80 {
            assert!(prove_omission(&log, 80, &index, seq).unwrap().is_none());
        }
        // A forged index's proof does not convict the honest head.
        let forged = MemoryIndex::forge_omitting(&atoms, &BTreeSet::from([10])).unwrap();
        let proof = prove_omission(&log, 80, &forged, 10).unwrap().unwrap();
        assert!(verify_omission(&head, &proof).is_err());
        assert!(prove_omission(&log, 80, &index, 80).is_err());
    }

    #[test]
    fn each_kind_of_omission_is_caught() {
        let atoms = random_log(42, 120);
        let log = log_of(&atoms);
        let mut kinds = BTreeSet::new();
        for seq in 0..120 {
            let forged = MemoryIndex::forge_omitting(&atoms, &BTreeSet::from([seq])).unwrap();
            let head = forged_head(&log, &forged);
            let proof = prove_omission(&log, 120, &forged, seq)
                .unwrap()
                .expect("omission found");
            verify_omission(&head, &proof).unwrap();
            kinds.insert(match proof.evidence {
                OmissionEvidence::Subject { .. } => "subject",
                OmissionEvidence::Link { .. } => "link",
                OmissionEvidence::Space { .. } => "space",
                OmissionEvidence::Size { .. } => "size",
            });
            // The proof names this atom: it does not convict for another.
            let mut other = proof.clone();
            other.atom = atoms[(seq as usize + 1) % 120].clone();
            assert!(verify_omission(&head, &other).is_err());
        }
        assert!(kinds.contains("subject"));
        // Spaceless or claimless atoms show the other kinds too.
        let revision = atoms.iter().find(|a| a.claim_key().is_none()).unwrap();
        let forged = MemoryIndex::forge_omitting(&atoms, &BTreeSet::from([revision.seq])).unwrap();
        let proof = prove_omission(&log, 120, &forged, revision.seq)
            .unwrap()
            .unwrap();
        assert!(matches!(proof.evidence, OmissionEvidence::Link { .. }));
    }

    #[test]
    fn space_evidence_catches_misfiled_atoms() {
        // An index built over a log where one atom's box was changed files
        // that atom's id in the wrong place: the subject map still lists it,
        // but the range query around its real box does not return it.
        let atoms = random_log(43, 60);
        let log = log_of(&atoms);
        let victim = atoms.iter().find(|a| a.space.is_some()).unwrap().seq;
        let honest = log.index(60).unwrap();
        let mut moved = atoms.clone();
        let mut space = moved[victim as usize].space.unwrap();
        space.bbox.min[0] += 1_000;
        space.bbox.max[0] += 1_000;
        moved[victim as usize].space = Some(space);
        // Build the misfiled index with the victim's real id.
        let mut builder = crate::index::IndexBuilder::new();
        for (i, atom) in moved.iter().enumerate() {
            builder.push_with_id(atom, atoms[i].id()).unwrap();
        }
        let misfiled = builder.finish();
        assert_ne!(misfiled.root(), honest.root());
        let head = forged_head(&log, &misfiled);
        let proof = prove_omission(&log, 60, &misfiled, victim)
            .unwrap()
            .unwrap();
        assert!(matches!(proof.evidence, OmissionEvidence::Space { .. }));
        verify_omission(&head, &proof).unwrap();
    }

    #[test]
    fn size_evidence_catches_short_indexes() {
        let atoms = random_log(44, 30);
        let log = log_of(&atoms);
        let short = MemoryIndex::build(&atoms[..29]).unwrap();
        let head = forged_head(&log, &short);
        let proof = prove_omission(&log, 30, &short, 29).unwrap().unwrap();
        assert!(matches!(proof.evidence, OmissionEvidence::Size { .. }));
        verify_omission(&head, &proof).unwrap();
        let honest = log.head(30).unwrap();
        assert!(verify_omission(&honest, &proof).is_err());
    }

    #[test]
    fn forged_inclusion_fails() {
        let atoms = random_log(45, 40);
        let log = log_of(&atoms);
        let forged = MemoryIndex::forge_omitting(&atoms, &BTreeSet::from([7])).unwrap();
        let head = forged_head(&log, &forged);
        let mut proof = prove_omission(&log, 40, &forged, 7).unwrap().unwrap();
        proof.inclusion[0] = Hash32::ZERO;
        assert!(verify_omission(&head, &proof).is_err());
        // An atom that is not in the log cannot be claimed omitted.
        let mut stranger = prove_omission(&log, 40, &forged, 7).unwrap().unwrap();
        stranger.atom.confidence = stranger.atom.confidence.saturating_sub(1);
        assert!(verify_omission(&head, &stranger).is_err());
    }

    proptest! {
        #![proptest_config(crate::testing::config(48, 0x5eed_0315))]

        #[test]
        fn omissions_are_always_provable(seed in any::<u64>(), n in 1usize..90, pick in any::<u64>()) {
            let atoms = random_log(seed, n);
            let log = log_of(&atoms);
            let seq = pick % n as u64;
            let forged = MemoryIndex::forge_omitting(&atoms, &BTreeSet::from([seq])).unwrap();
            let head = forged_head(&log, &forged);
            let proof = prove_omission(&log, n as u64, &forged, seq).unwrap();
            prop_assert!(proof.is_some(), "an omitted atom is always caught");
            let proof = proof.unwrap();
            prop_assert!(verify_omission(&head, &proof).is_ok());
            let honest = log.head(n as u64).unwrap();
            prop_assert!(verify_omission(&honest, &proof).is_err());
            let index = log.index(n as u64).unwrap();
            prop_assert!(prove_omission(&log, n as u64, &index, seq).unwrap().is_none());
        }
    }
}
