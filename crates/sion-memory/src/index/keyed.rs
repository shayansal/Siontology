//! Subject and link queries with proofs of completeness.
//!
//! The subject map lists, for each `(subject, predicate)`, every claim of
//! the prefix with that subject and predicate; the link map lists, for each
//! atom id, every atom of the prefix that names it (revisions, conflicts and
//! links). A [`KeyedProof`] proves the complete list for one key, or that
//! the key has none.

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use sion_core::hash::Hash32;

use crate::atom::MemoryAtomId;
use crate::error::{MemoryError, Result};
use crate::index::map::{MapDomains, MapLookup};
use crate::index::{IndexHeader, MemoryIndex, LINK_MAP, SUBJECT_MAP};

/// A proof of the complete list for one key of a map of the index.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct KeyedProof<K> {
    /// What the index root commits to.
    pub header: IndexHeader,
    /// The key's presence or absence.
    pub lookup: MapLookup<K>,
}

/// The complete list of claims for a `(subject, predicate)`.
pub type SubjectProof = KeyedProof<(String, String)>;

/// The complete list of atoms that name a target.
pub type LinkProof = KeyedProof<MemoryAtomId>;

fn verify_list<K: Ord + BorshSerialize>(
    domains: &MapDomains,
    index_root: &Hash32,
    header_map: impl Fn(&IndexHeader) -> crate::index::map::MapCommitment,
    key: &K,
    results: &[MemoryAtomId],
    proof: &KeyedProof<K>,
) -> Result<()> {
    proof.header.check(index_root)?;
    let digest = (!results.is_empty()).then(|| domains.value_digest(results));
    proof
        .lookup
        .verify(domains, &header_map(&proof.header), key, digest.as_ref())?;
    Ok(())
}

/// The claims with this subject and predicate, in log order, and the proof
/// that the list is complete.
pub fn prove_subject(
    index: &MemoryIndex,
    subject: &str,
    predicate: &str,
) -> (Vec<MemoryAtomId>, SubjectProof) {
    let key = (subject.to_owned(), predicate.to_owned());
    let map = index.subject_map();
    let results = map
        .position(&key)
        .map_or_else(|_| Vec::new(), |i| index.subject_list(i).to_vec());
    let proof = KeyedProof {
        header: *index.header(),
        lookup: map.lookup(&key),
    };
    (results, proof)
}

/// Verifies that `results` are exactly the claims of the indexed prefix
/// with this subject and predicate, in log order.
pub fn verify_subject(
    index_root: &Hash32,
    subject: &str,
    predicate: &str,
    results: &[MemoryAtomId],
    proof: &SubjectProof,
) -> Result<()> {
    let key = (subject.to_owned(), predicate.to_owned());
    verify_list(
        &SUBJECT_MAP,
        index_root,
        |h| h.subjects,
        &key,
        results,
        proof,
    )
}

/// The atoms that name `target` (as a revision target, a conflict member or
/// a link target), in log order, and the proof that the list is complete.
pub fn prove_links(index: &MemoryIndex, target: &MemoryAtomId) -> (Vec<MemoryAtomId>, LinkProof) {
    let map = index.link_map();
    let results = map
        .position(target)
        .map_or_else(|_| Vec::new(), |i| index.link_list(i).to_vec());
    let proof = KeyedProof {
        header: *index.header(),
        lookup: map.lookup(target),
    };
    (results, proof)
}

/// Verifies that `results` are exactly the atoms of the indexed prefix that
/// name `target`, in log order.
pub fn verify_links(
    index_root: &Hash32,
    target: &MemoryAtomId,
    results: &[MemoryAtomId],
    proof: &LinkProof,
) -> Result<()> {
    verify_list(&LINK_MAP, index_root, |h| h.links, target, results, proof)
}

/// Checks that a proof's header covers a given log size (as a head commits).
pub fn check_size<K>(proof: &KeyedProof<K>, size: u64) -> Result<()> {
    if proof.header.size != size {
        return Err(MemoryError::InvalidProof(
            "index size differs from the head",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atom::{MemoryAtom, Statement};
    use crate::index::testing::random_log;
    use proptest::prelude::*;
    use sion_core::encode::encode;

    fn brute_subject(atoms: &[MemoryAtom], s: &str, p: &str) -> Vec<MemoryAtomId> {
        atoms
            .iter()
            .filter(|a| a.claim_key() == Some((s, p)))
            .map(|a| a.id())
            .collect()
    }

    fn brute_links(atoms: &[MemoryAtom], target: &MemoryAtomId) -> Vec<MemoryAtomId> {
        atoms
            .iter()
            .filter(|a| a.link_targets().contains(target))
            .map(|a| a.id())
            .collect()
    }

    #[test]
    fn subject_lists_are_complete() {
        let atoms = random_log(21, 150);
        let index = MemoryIndex::build(&atoms).unwrap();
        let root = index.root();
        for s in ["s0", "s1", "s2", "s3", "s9"] {
            for p in ["p0", "p1", "p7"] {
                let (ids, proof) = prove_subject(&index, s, p);
                assert_eq!(ids, brute_subject(&atoms, s, p));
                verify_subject(&root, s, p, &ids, &proof).unwrap();
                check_size(&proof, 150).unwrap();
                assert!(check_size(&proof, 149).is_err());
                if !ids.is_empty() {
                    assert!(verify_subject(&root, s, p, &ids[1..], &proof).is_err());
                    let mut reordered = ids.clone();
                    reordered.reverse();
                    if ids.len() > 1 {
                        assert!(verify_subject(&root, s, p, &reordered, &proof).is_err());
                    }
                    assert!(verify_subject(&root, s, p, &[], &proof).is_err());
                    // A presence proof for one key does not prove another.
                    assert!(verify_subject(&root, s, "zz", &ids, &proof).is_err());
                } else {
                    assert!(verify_subject(&root, s, p, &[atoms[0].id()], &proof).is_err());
                    // An absence proof covers its whole gap between neighbours,
                    // so it also proves the absence of the gap's other keys, and
                    // never the absence of a key that is present.
                    for present in &atoms {
                        if let Some((ps, pp)) = present.claim_key() {
                            assert!(verify_subject(&root, ps, pp, &[], &proof).is_err());
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn link_lists_are_complete() {
        let atoms = random_log(22, 200);
        let index = MemoryIndex::build(&atoms).unwrap();
        let root = index.root();
        let mut linked = 0;
        for atom in &atoms {
            let target = atom.id();
            let (ids, proof) = prove_links(&index, &target);
            assert_eq!(ids, brute_links(&atoms, &target));
            verify_links(&root, &target, &ids, &proof).unwrap();
            linked += usize::from(!ids.is_empty());
            if let Statement::Revision { targets, .. } = &atom.statement {
                for t in targets {
                    assert!(index.linkers(t).contains(&atom.id()));
                }
            }
        }
        assert!(linked > 5);
        let nobody = MemoryAtomId(Hash32([0xee; 32]));
        let (ids, proof) = prove_links(&index, &nobody);
        assert!(ids.is_empty());
        verify_links(&root, &nobody, &ids, &proof).unwrap();
    }

    proptest! {
        #![proptest_config(crate::testing::config(96, 0x5eed_4e7d))]

        #[test]
        fn keyed_answers_are_complete_and_tamper_evident(
            seed in any::<u64>(),
            n in 0usize..120,
            pick in any::<usize>(),
            flip in any::<usize>(),
        ) {
            let atoms = random_log(seed, n);
            let index = MemoryIndex::build(&atoms).unwrap();
            let root = index.root();
            let s = format!("s{}", pick % 5);
            let p = format!("p{}", (pick / 5) % 3);
            let (ids, proof) = prove_subject(&index, &s, &p);
            prop_assert_eq!(&ids, &brute_subject(&atoms, &s, &p));
            prop_assert!(verify_subject(&root, &s, &p, &ids, &proof).is_ok());
            let bytes = encode(&proof);
            let mut tampered = bytes.clone();
            let bit = flip % (bytes.len() * 8);
            tampered[bit / 8] ^= 1 << (bit % 8);
            if let Ok(forged) = borsh::from_slice::<SubjectProof>(&tampered) {
                prop_assert!(verify_subject(&root, &s, &p, &ids, &forged).is_err());
            }
            if let Some(atom) = atoms.get(pick % atoms.len().max(1)) {
                let target = atom.id();
                let (ids, proof) = prove_links(&index, &target);
                prop_assert_eq!(&ids, &brute_links(&atoms, &target));
                prop_assert!(verify_links(&root, &target, &ids, &proof).is_ok());
                let mut more = ids.clone();
                more.push(target);
                prop_assert!(verify_links(&root, &target, &more, &proof).is_err());
            }
        }
    }
}
