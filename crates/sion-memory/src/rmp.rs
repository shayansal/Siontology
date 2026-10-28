//! The Reality Memory Protocol over a writer's log.
//!
//! [`Memory`] holds one writer's log and keys, in memory or on disk, and
//! exposes Hotgraph's RMP operations as Siontology implements them:
//!
//! | Operation | Here |
//! |---|---|
//! | REMEMBER | [`Memory::remember`]: append an atom (sealing its payload if asked); visible to others at the next checkpoint |
//! | RECALL, GROUND | [`Memory::recall`]: a range, subject, link or position query answered with atoms and a proof against the current head; the head says what the answer is complete as of |
//! | VERIFY | [`Memory::verify`]: check a recall against a head (from the ledger, for others' logs) |
//! | REVISE | [`Memory::revise`]: a revision atom; nothing is mutated |
//! | FORGET | [`Memory::forget`]: an erasure atom, then the epoch key's destruction |
//! | SHARE | [`Memory::share`]: the epoch key wrapped to a member's ML-KEM-768 key |
//! | AUDIT | [`Memory::audit`]: a consistency proof between two sizes of the log |
//!
//! SIMULATE, EXPLAIN and COMPRESS are out of scope. They stay local to the
//! agent's runtime, their outputs are labelled (Simulation and Summary
//! atoms, simulation-only), and they are never written to shared memory as
//! fact: a Simulation atom cannot be labelled usable for planning, and no
//! revision makes it Accepted.
//!
//! Requests and responses are serde types (JSON-ready) for an MCP server.
//! Beliefs across several revisers' logs are in [`crate::belief`].

use std::path::Path;

use rand_core::CryptoRngCore;
use serde::{Deserialize, Serialize};
use sion_core::hash::Hash32;
use sion_core::ids::AccountId;
use sion_core::merkle::{verify_consistency, verify_inclusion};
use sion_crypto::kem::{KemKeypair, KemPublicKey};

use crate::atom::{
    AtomClass, AtomPolicy, Clock, EpochKeyId, Link, MemoryAtom, MemoryAtomId, MemoryKind, Payload,
    Provenance, SealedPayload, Space, Statement, PERMILLE,
};
use crate::belief::BeliefState;
use crate::error::{MemoryError, Result};
use crate::geom::Interval;
use crate::hexser::hex_bytes;
use crate::index::keyed::{
    check_size, prove_links, prove_subject, verify_links, verify_subject, LinkProof, SubjectProof,
};
use crate::index::range::{
    prove_range, select_atoms, verify_range, AtomEntry, RangeProof, RangeQuery,
};
use crate::index::MemoryIndex;
use crate::log::{Head, LogStore, WriterLog};
use crate::seal::{EpochKey, KeyRing, WrappedEpochKey};

fn full_confidence() -> u16 {
    PERMILLE
}

/// A payload as the caller supplies it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PayloadRequest {
    /// No payload.
    #[default]
    None,
    /// Plain bytes.
    Public(#[serde(with = "hex_bytes")] Vec<u8>),
    /// Bytes to seal under one of the memory's epoch keys.
    Sealed {
        /// The epoch key.
        key: EpochKeyId,
        /// The plaintext.
        #[serde(with = "hex_bytes")]
        plaintext: Vec<u8>,
    },
    /// A commitment to a blob kept elsewhere.
    Commitment {
        /// The blob's hash.
        blob: Hash32,
        /// Its size.
        size: u64,
    },
}

/// REMEMBER: everything of an atom but its writer and position.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RememberRequest {
    /// Kind.
    pub kind: MemoryKind,
    /// Class.
    pub class: AtomClass,
    /// Statement.
    pub statement: Statement,
    /// Valid interval.
    pub valid: Interval,
    /// Clock behind it.
    #[serde(default)]
    pub clock: Clock,
    /// Capture time.
    pub observed_at: u64,
    /// Place.
    #[serde(default)]
    pub space: Option<Space>,
    /// Confidence in permille (default 1,000).
    #[serde(default = "full_confidence")]
    pub confidence: u16,
    /// Provenance.
    #[serde(default)]
    pub provenance: Provenance,
    /// Links.
    #[serde(default)]
    pub links: Vec<Link>,
    /// Policy (a sealed payload sets its erasure key).
    #[serde(default)]
    pub policy: AtomPolicy,
    /// Payload.
    #[serde(default)]
    pub payload: PayloadRequest,
}

impl RememberRequest {
    /// A request with defaults for everything but the essentials.
    pub fn new(
        kind: MemoryKind,
        class: AtomClass,
        statement: Statement,
        valid: Interval,
        observed_at: u64,
    ) -> Self {
        let mut policy = AtomPolicy::default();
        if class == AtomClass::Simulation {
            policy.ai_usage = crate::atom::AiUsage::SimulationOnly;
        }
        RememberRequest {
            kind,
            class,
            statement,
            valid,
            clock: Clock::default(),
            observed_at,
            space: None,
            confidence: PERMILLE,
            provenance: Provenance::default(),
            links: Vec::new(),
            policy,
            payload: PayloadRequest::None,
        }
    }
}

/// The result of an append. It carries the new log root, which is cheap;
/// the index root is computed when a head is needed ([`Memory::head`], at
/// checkpoint or recall time), so appending stays fast.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RememberResponse {
    /// The new atom's id.
    pub id: MemoryAtomId,
    /// Its position.
    pub seq: u64,
    /// The log's size after it.
    pub size: u64,
    /// The log root after it (uncommitted until the ledger records it).
    pub root: Hash32,
}

/// RECALL: a query.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecallRequest {
    /// Everything in a region of a frame during a time range.
    Range(RangeQuery),
    /// Every claim with this subject and predicate.
    Subject {
        /// Subject.
        subject: String,
        /// Predicate.
        predicate: String,
    },
    /// Every atom that names `target` (revisions, conflicts, links).
    Links {
        /// The atom named.
        target: MemoryAtomId,
    },
    /// The atom at a position.
    Atom {
        /// Its position.
        seq: u64,
    },
}

/// The proof part of a recall.
// A proof container, built and sent once per recall: boxing the large
// variant would only complicate the serialized type.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecallProof {
    /// A complete range answer.
    Range {
        /// Leaf entries of every atom that matches in space and time.
        results: Vec<AtomEntry>,
        /// Its proof.
        proof: RangeProof,
    },
    /// A complete subject list.
    Subject {
        /// The ids.
        ids: Vec<MemoryAtomId>,
        /// Its proof.
        proof: SubjectProof,
    },
    /// A complete link list.
    Links {
        /// The ids.
        ids: Vec<MemoryAtomId>,
        /// Its proof.
        proof: LinkProof,
    },
    /// One atom's inclusion.
    Atom {
        /// Its inclusion proof.
        inclusion: Vec<Hash32>,
    },
}

/// A recall's answer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecallResponse {
    /// The head the answer is complete as of.
    pub head: Head,
    /// The atoms, in the proof's order (for a range, every space-time match;
    /// the kind filter applies on verification).
    pub atoms: Vec<MemoryAtom>,
    /// The proof.
    pub proof: RecallProof,
}

/// REVISE: a new belief state for some atoms.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviseRequest {
    /// The revised atoms (any writer's).
    pub targets: Vec<MemoryAtomId>,
    /// Their new state.
    pub state: BeliefState,
    /// When they stopped holding, if the revision says so.
    #[serde(default)]
    pub end_valid: Option<u64>,
    /// Why.
    #[serde(default)]
    pub reason: String,
    /// When the revision was made (nanoseconds).
    pub at: u64,
}

/// FORGET: destroy an epoch key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForgetRequest {
    /// The key.
    pub key: EpochKeyId,
    /// When (nanoseconds).
    pub at: u64,
}

/// SHARE: wrap an epoch key to a member.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShareRequest {
    /// The key.
    pub key: EpochKeyId,
    /// The member's ML-KEM-768 public key.
    pub member: KemPublicKey,
}

/// The wrapped key.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShareResponse {
    /// The key sealed to the member.
    pub wrapped: WrappedEpochKey,
}

/// AUDIT: prove that the log at `old_size` is a prefix of the log at `new_size`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditRequest {
    /// The earlier size.
    pub old_size: u64,
    /// The later size (the current one if `None`).
    #[serde(default)]
    pub new_size: Option<u64>,
}

/// The consistency proof between two sizes of the log.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditResponse {
    /// The earlier size.
    pub old_size: u64,
    /// The root at it.
    pub old_root: Hash32,
    /// The later size.
    pub new_size: u64,
    /// The root at it.
    pub new_root: Hash32,
    /// RFC 9162 consistency proof.
    pub proof: Vec<Hash32>,
}

#[derive(Debug)]
enum Backing {
    Volatile { log: WriterLog, keys: KeyRing },
    Disk(LogStore),
}

/// One writer's memory: its log, its keys and a cached index.
#[derive(Debug)]
pub struct Memory {
    backing: Backing,
    index: Option<MemoryIndex>,
}

impl Memory {
    /// A memory kept in this process only.
    pub fn in_memory(writer: AccountId) -> Memory {
        Memory {
            backing: Backing::Volatile {
                log: WriterLog::new(writer),
                keys: KeyRing::new(),
            },
            index: None,
        }
    }

    /// A new memory on disk ([`LogStore::create`]).
    pub fn create(dir: impl AsRef<Path>, writer: AccountId) -> Result<Memory> {
        Ok(Memory {
            backing: Backing::Disk(LogStore::create(dir, writer)?),
            index: None,
        })
    }

    /// An existing memory on disk ([`LogStore::open`]).
    pub fn open(dir: impl AsRef<Path>) -> Result<Memory> {
        Ok(Memory {
            backing: Backing::Disk(LogStore::open(dir)?),
            index: None,
        })
    }

    /// Opens the memory in `dir`, or creates one for `writer`.
    pub fn open_or_create(dir: impl AsRef<Path>, writer: AccountId) -> Result<Memory> {
        Ok(Memory {
            backing: Backing::Disk(LogStore::open_or_create(dir, writer)?),
            index: None,
        })
    }

    /// The writer.
    pub fn writer(&self) -> AccountId {
        self.log().writer()
    }

    /// The log.
    pub fn log(&self) -> &WriterLog {
        match &self.backing {
            Backing::Volatile { log, .. } => log,
            Backing::Disk(store) => store.log(),
        }
    }

    /// The keys.
    pub fn keys(&self) -> &KeyRing {
        match &self.backing {
            Backing::Volatile { keys, .. } => keys,
            Backing::Disk(store) => store.keys(),
        }
    }

    fn update_keys<T>(&mut self, change: impl FnOnce(&mut KeyRing) -> Result<T>) -> Result<T> {
        match &mut self.backing {
            Backing::Volatile { keys, .. } => change(keys),
            Backing::Disk(store) => store.update_keys(change),
        }
    }

    fn append(&mut self, atom: MemoryAtom) -> Result<u64> {
        self.index = None;
        match &mut self.backing {
            Backing::Volatile { log, .. } => log.append(atom),
            Backing::Disk(store) => store.append(atom),
        }
    }

    /// The index of the whole log (rebuilt on first use after appends).
    pub fn index(&mut self) -> &MemoryIndex {
        let len = self.log().len();
        if self.index.as_ref().map(MemoryIndex::size) != Some(len) {
            let index = self.log().index(len).expect("the whole log is indexable");
            self.index = Some(index);
        }
        self.index.as_ref().expect("just built")
    }

    /// The current head, with the index root of the whole log (the index is
    /// rebuilt if atoms were appended since it was last built).
    pub fn head(&mut self) -> Head {
        let index_root = self.index().root();
        let log = self.log();
        Head {
            size: log.len(),
            root: log.root(),
            index_root,
        }
    }

    /// Adds a fresh epoch key for `epoch` of memory space `space`.
    pub fn new_epoch_key(
        &mut self,
        space: Hash32,
        epoch: u64,
        rng: &mut impl CryptoRngCore,
    ) -> Result<EpochKeyId> {
        let key = EpochKey::generate(space, epoch, rng);
        self.update_keys(|keys| keys.insert(key))
    }

    /// Takes in an epoch key another writer shared with this one.
    pub fn accept_share(
        &mut self,
        wrapped: &WrappedEpochKey,
        keypair: &KemKeypair,
    ) -> Result<EpochKeyId> {
        let key = EpochKey::unwrap(wrapped, keypair)?;
        self.update_keys(|keys| keys.insert(key))
    }

    /// Opens a sealed payload with the keys held.
    pub fn open_payload(&self, atom: &MemoryAtom) -> Result<Vec<u8>> {
        self.keys().open(atom)
    }

    /// REMEMBER: appends an atom built from the request, sealing its
    /// payload under the named epoch key if asked.
    pub fn remember(
        &mut self,
        request: RememberRequest,
        rng: &mut impl CryptoRngCore,
    ) -> Result<RememberResponse> {
        let sealed = match &request.payload {
            PayloadRequest::Sealed { key, plaintext } => {
                let (writer, seq) = (self.writer(), self.log().len());
                Some(self.keys().get(key)?.seal(&writer, seq, plaintext, rng)?)
            }
            _ => None,
        };
        self.append_request(request, sealed)
    }

    fn append_request(
        &mut self,
        request: RememberRequest,
        sealed: Option<SealedPayload>,
    ) -> Result<RememberResponse> {
        let writer = self.writer();
        let seq = self.log().len();
        let mut policy = request.policy;
        let payload = match request.payload {
            PayloadRequest::None => Payload::default(),
            PayloadRequest::Public(bytes) => Payload::Public(bytes),
            PayloadRequest::Commitment { blob, size } => Payload::Commitment { blob, size },
            PayloadRequest::Sealed { key, .. } => {
                let sealed = sealed.ok_or(MemoryError::NotSealed)?;
                policy.erasure_key = Some(key);
                Payload::Sealed(sealed)
            }
        };
        let atom = MemoryAtom {
            writer,
            seq,
            kind: request.kind,
            class: request.class,
            statement: request.statement,
            valid: request.valid,
            clock: request.clock,
            observed_at: request.observed_at,
            space: request.space,
            confidence: request.confidence,
            provenance: request.provenance,
            links: request.links,
            policy,
            payload,
        };
        let id = atom.id();
        self.append(atom)?;
        let log = self.log();
        Ok(RememberResponse {
            id,
            seq,
            size: log.len(),
            root: log.root(),
        })
    }

    /// RECALL: answers a query with atoms and a proof against the current head.
    pub fn recall(&mut self, request: &RecallRequest) -> Result<RecallResponse> {
        let head = self.head();
        let fetch = |log: &WriterLog, ids: &[MemoryAtomId]| -> Result<Vec<MemoryAtom>> {
            ids.iter()
                .map(|id| {
                    log.atom_by_id(id)
                        .cloned()
                        .ok_or(MemoryError::UnknownAtom(*id))
                })
                .collect()
        };
        let (atoms, proof) = match request {
            RecallRequest::Range(query) => {
                let (results, proof) = prove_range(self.index(), query)?;
                let ids: Vec<MemoryAtomId> = results.iter().map(|r| r.id).collect();
                (
                    fetch(self.log(), &ids)?,
                    RecallProof::Range { results, proof },
                )
            }
            RecallRequest::Subject { subject, predicate } => {
                let (ids, proof) = prove_subject(self.index(), subject, predicate);
                (
                    fetch(self.log(), &ids)?,
                    RecallProof::Subject { ids, proof },
                )
            }
            RecallRequest::Links { target } => {
                let (ids, proof) = prove_links(self.index(), target);
                (fetch(self.log(), &ids)?, RecallProof::Links { ids, proof })
            }
            RecallRequest::Atom { seq } => {
                let atom = self
                    .log()
                    .atom(*seq)
                    .cloned()
                    .ok_or(MemoryError::OutOfRange {
                        index: *seq,
                        size: head.size,
                    })?;
                let inclusion = self.log().inclusion_proof(*seq)?;
                (vec![atom], RecallProof::Atom { inclusion })
            }
        };
        Ok(RecallResponse { head, atoms, proof })
    }

    /// VERIFY: checks a recall against a trusted head and returns its atoms
    /// (for a range, those the query's kind filter keeps).
    pub fn verify(
        head: &Head,
        request: &RecallRequest,
        response: &RecallResponse,
    ) -> Result<Vec<MemoryAtom>> {
        if response.head != *head {
            return Err(MemoryError::InvalidProof("answer is against another head"));
        }
        let ids_match = |ids: &[MemoryAtomId]| -> Result<()> {
            if response.atoms.len() != ids.len()
                || response
                    .atoms
                    .iter()
                    .zip(ids)
                    .any(|(atom, id)| atom.id() != *id)
            {
                return Err(MemoryError::InvalidProof("atoms do not match the proof"));
            }
            Ok(())
        };
        match (request, &response.proof) {
            (RecallRequest::Range(query), RecallProof::Range { results, proof }) => {
                verify_range(&head.index_root, query, results, proof)?;
                if proof.header.size != head.size {
                    return Err(MemoryError::InvalidProof(
                        "index size differs from the head",
                    ));
                }
                Ok(select_atoms(query, results, &response.atoms)?
                    .into_iter()
                    .cloned()
                    .collect())
            }
            (
                RecallRequest::Subject { subject, predicate },
                RecallProof::Subject { ids, proof },
            ) => {
                verify_subject(&head.index_root, subject, predicate, ids, proof)?;
                check_size(proof, head.size)?;
                ids_match(ids)?;
                Ok(response.atoms.clone())
            }
            (RecallRequest::Links { target }, RecallProof::Links { ids, proof }) => {
                verify_links(&head.index_root, target, ids, proof)?;
                check_size(proof, head.size)?;
                ids_match(ids)?;
                Ok(response.atoms.clone())
            }
            (RecallRequest::Atom { seq }, RecallProof::Atom { inclusion }) => {
                let [atom] = response.atoms.as_slice() else {
                    return Err(MemoryError::InvalidProof("expected one atom"));
                };
                if atom.seq != *seq {
                    return Err(MemoryError::InvalidProof("atom at another position"));
                }
                verify_inclusion(&head.root, head.size, *seq, &atom.id().leaf(), inclusion)?;
                Ok(response.atoms.clone())
            }
            _ => Err(MemoryError::InvalidProof(
                "proof does not answer the request",
            )),
        }
    }

    /// REVISE: appends a revision atom. With one target in this log, the
    /// revision inherits its space and valid interval, so the range queries
    /// that find the target find the revision too.
    pub fn revise(&mut self, request: ReviseRequest) -> Result<RememberResponse> {
        let mut statement = Statement::revision(
            request.targets.iter().copied(),
            request.state,
            request.reason,
        );
        if let Statement::Revision { end_valid, .. } = &mut statement {
            *end_valid = request.end_valid;
        }
        let local = match request.targets.as_slice() {
            [only] => self.log().atom_by_id(only).cloned(),
            _ => None,
        };
        let mut remember = RememberRequest::new(
            MemoryKind::Correction,
            AtomClass::Revision,
            statement,
            Interval::starting(request.at),
            request.at,
        );
        if let Some(target) = local {
            remember.space = target.space;
            remember.valid = target.valid;
        }
        self.append_request(remember, None)
    }

    /// FORGET: records an erasure atom, then destroys the epoch key, so
    /// every payload sealed under it becomes unreadable.
    pub fn forget(&mut self, request: ForgetRequest) -> Result<RememberResponse> {
        self.keys().get(&request.key)?;
        let remember = RememberRequest::new(
            MemoryKind::Correction,
            AtomClass::Event,
            Statement::Erasure { key: request.key },
            Interval::starting(request.at),
            request.at,
        );
        let response = self.append_request(remember, None)?;
        self.update_keys(|keys| keys.destroy(&request.key))?;
        Ok(response)
    }

    /// SHARE: wraps an epoch key to a member.
    pub fn share(
        &self,
        request: &ShareRequest,
        rng: &mut impl CryptoRngCore,
    ) -> Result<ShareResponse> {
        let wrapped = self.keys().get(&request.key)?.wrap(&request.member, rng)?;
        Ok(ShareResponse { wrapped })
    }

    /// AUDIT: the consistency proof between two sizes of the log.
    pub fn audit(&self, request: &AuditRequest) -> Result<AuditResponse> {
        let log = self.log();
        let new_size = request.new_size.unwrap_or(log.len());
        Ok(AuditResponse {
            old_size: request.old_size,
            old_root: log.root_at(request.old_size.min(new_size))?,
            new_size,
            new_root: log.root_at(new_size)?,
            proof: log.consistency_proof_between(request.old_size, new_size)?,
        })
    }

    /// Checks an audit with `sion_core::merkle::verify_consistency`.
    pub fn verify_audit(response: &AuditResponse) -> Result<()> {
        verify_consistency(
            &response.old_root,
            response.old_size,
            &response.new_root,
            response.new_size,
            &response.proof,
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atom::FrameId;
    use crate::atom::Value;
    use crate::geom::{Box3, TimeRange};

    fn robot() -> AccountId {
        AccountId::derive(b"rmp-robot")
    }

    fn observe(memory: &mut Memory, n: u64, point: [i64; 3]) -> RememberResponse {
        let mut request = RememberRequest::new(
            MemoryKind::Observation,
            AtomClass::Observation,
            Statement::claim(format!("pallet-{n}"), "at", Value::Entity("aisle-2".into())),
            Interval::closed(n * 1_000, n * 1_000 + 500),
            n * 1_000,
        );
        request.space = Some(Space {
            frame: FrameId::named("warehouse"),
            bbox: Box3::point(point),
            uncertainty_mm: 20,
        });
        memory
            .remember(request, &mut sion_core::rng::seeded(n))
            .unwrap()
    }

    #[test]
    fn remember_recall_verify() {
        let mut memory = Memory::in_memory(robot());
        for n in 0..10 {
            let r = observe(&mut memory, n, [n as i64 * 1_000, 0, 0]);
            assert_eq!(r.seq, n);
        }
        let head = memory.head();
        assert_eq!(head.size, 10);
        let query = RangeQuery::new(
            FrameId::named("warehouse"),
            Box3::new([0, -10, -10], [3_500, 10, 10]),
            TimeRange::ALL,
        );
        let request = RecallRequest::Range(query.clone());
        let response = memory.recall(&request).unwrap();
        let atoms = Memory::verify(&head, &request, &response).unwrap();
        assert_eq!(atoms.len(), 4);
        // Filter by kind on verification.
        let filtered = RecallRequest::Range(query.with_kinds([MemoryKind::Pose]));
        let response = memory.recall(&filtered).unwrap();
        assert!(Memory::verify(&head, &filtered, &response)
            .unwrap()
            .is_empty());
        // Subject, links, position.
        let request = RecallRequest::Subject {
            subject: "pallet-3".into(),
            predicate: "at".into(),
        };
        let response = memory.recall(&request).unwrap();
        assert_eq!(Memory::verify(&head, &request, &response).unwrap().len(), 1);
        let request = RecallRequest::Atom { seq: 7 };
        let response = memory.recall(&request).unwrap();
        assert_eq!(
            Memory::verify(&head, &request, &response).unwrap()[0].seq,
            7
        );
        // A response for another request, or another head, fails.
        let other = RecallRequest::Atom { seq: 6 };
        assert!(Memory::verify(&head, &other, &response).is_err());
        let mut stale = head;
        stale.size = 9;
        assert!(Memory::verify(&stale, &request, &response).is_err());
        let mut forged = response.clone();
        forged.atoms[0].confidence = 1;
        assert!(Memory::verify(&head, &request, &forged).is_err());
        assert!(memory.recall(&RecallRequest::Atom { seq: 99 }).is_err());
    }

    #[test]
    fn revise_inherits_place_and_links_find_it() {
        let mut memory = Memory::in_memory(robot());
        let seen = observe(&mut memory, 1, [5, 5, 5]);
        let revised = memory
            .revise(ReviseRequest {
                targets: vec![seen.id],
                state: BeliefState::Superseded,
                end_valid: Some(1_200),
                reason: "moved".into(),
                at: 2_000,
            })
            .unwrap();
        let revision = memory.log().atom(revised.seq).unwrap().clone();
        assert_eq!(revision.space, memory.log().atom(0).unwrap().space);
        let head = memory.head();
        let request = RecallRequest::Links { target: seen.id };
        let response = memory.recall(&request).unwrap();
        let linked = Memory::verify(&head, &request, &response).unwrap();
        assert_eq!(linked, vec![revision.clone()]);
        if let Statement::Revision { end_valid, .. } = revision.statement {
            assert_eq!(end_valid, Some(1_200));
        }
        // The range query around the target finds the revision too.
        let target = memory.log().atom(0).unwrap().clone();
        let request = RecallRequest::Range(RangeQuery::around(&target));
        let response = memory.recall(&request).unwrap();
        assert_eq!(Memory::verify(&head, &request, &response).unwrap().len(), 2);
        // A revision of a remote atom has no place.
        let remote = memory
            .revise(ReviseRequest {
                targets: vec![MemoryAtomId(Hash32([9; 32]))],
                state: BeliefState::Disputed,
                end_valid: None,
                reason: String::new(),
                at: 3_000,
            })
            .unwrap();
        assert!(memory.log().atom(remote.seq).unwrap().space.is_none());
    }

    #[test]
    fn seal_share_and_forget() {
        let mut rng = sion_core::rng::seeded(77);
        let mut memory = Memory::in_memory(robot());
        let key = memory.new_epoch_key(Hash32([1; 32]), 0, &mut rng).unwrap();
        let mut request = RememberRequest::new(
            MemoryKind::Episodic,
            AtomClass::AgentMemory,
            Statement::Content(Hash32([2; 32])),
            Interval::starting(0),
            0,
        );
        request.payload = PayloadRequest::Sealed {
            key,
            plaintext: b"camera frame".to_vec(),
        };
        let sealed = memory.remember(request, &mut rng).unwrap();
        let atom = memory.log().atom(sealed.seq).unwrap().clone();
        assert_eq!(atom.policy.erasure_key, Some(key));
        assert_eq!(memory.open_payload(&atom).unwrap(), b"camera frame");
        // Share with a teammate, who can open it.
        let teammate = KemKeypair::from_seed(&[3; 32]);
        let shared = memory
            .share(
                &ShareRequest {
                    key,
                    member: teammate.public().clone(),
                },
                &mut rng,
            )
            .unwrap();
        let mut other = Memory::in_memory(AccountId::derive(b"teammate"));
        assert_eq!(other.accept_share(&shared.wrapped, &teammate).unwrap(), key);
        assert_eq!(other.open_payload(&atom).unwrap(), b"camera frame");
        // Forget: an erasure atom, and the payload is unreadable.
        let erased = memory.forget(ForgetRequest { key, at: 10 }).unwrap();
        let erasure = memory.log().atom(erased.seq).unwrap();
        assert_eq!(erasure.statement, Statement::Erasure { key });
        assert_eq!(
            memory.open_payload(&atom),
            Err(MemoryError::KeyDestroyed(key))
        );
        assert!(memory.forget(ForgetRequest { key, at: 11 }).is_err());
        assert_eq!(memory.log().len(), 2, "a failed forget writes nothing");
    }

    #[test]
    fn audits_verify_with_sion_core() {
        let mut memory = Memory::in_memory(robot());
        let mut heads = vec![memory.head()];
        for n in 0..6 {
            observe(&mut memory, n, [0, 0, 0]);
            heads.push(memory.head());
        }
        for old in &heads {
            let response = memory
                .audit(&AuditRequest {
                    old_size: old.size,
                    new_size: None,
                })
                .unwrap();
            assert_eq!(response.old_root, old.root);
            Memory::verify_audit(&response).unwrap();
        }
        let mut bad = memory
            .audit(&AuditRequest {
                old_size: 2,
                new_size: Some(5),
            })
            .unwrap();
        bad.old_root = Hash32::ZERO;
        assert!(Memory::verify_audit(&bad).is_err());
        assert!(memory
            .audit(&AuditRequest {
                old_size: 2,
                new_size: Some(50)
            })
            .is_err());
    }

    #[test]
    fn memories_persist_on_disk() {
        let dir = std::env::temp_dir().join(format!("sion-memory-rmp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut rng = sion_core::rng::seeded(5);
        let (key, head) = {
            let mut memory = Memory::create(&dir, robot()).unwrap();
            observe(&mut memory, 0, [1, 2, 3]);
            let key = memory.new_epoch_key(Hash32([4; 32]), 0, &mut rng).unwrap();
            (key, memory.head())
        };
        let mut memory = Memory::open(&dir).unwrap();
        assert_eq!(memory.head(), head);
        assert!(memory.keys().get(&key).is_ok());
        memory.forget(ForgetRequest { key, at: 5 }).unwrap();
        drop(memory);
        let memory = Memory::open_or_create(&dir, robot()).unwrap();
        assert!(memory.keys().is_destroyed(&key));
        assert_eq!(memory.writer(), robot());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn requests_and_responses_are_json() {
        let mut memory = Memory::in_memory(robot());
        observe(&mut memory, 0, [0, 0, 0]);
        let request = RecallRequest::Range(RangeQuery::new(
            FrameId::named("warehouse"),
            Box3::ALL,
            TimeRange::ALL,
        ));
        let response = memory.recall(&request).unwrap();
        let json = serde_json::to_string(&response).unwrap();
        let back: RecallResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(back, response);
        let text = r#"{"kind":"goal","class":"agent_memory","statement":{"claim":{"subject":"user","predicate":"wants","object":{"text":"tea"}}},"valid":{"start":0,"end":null},"observed_at":0}"#;
        let remember: RememberRequest = serde_json::from_str(text).unwrap();
        assert_eq!(remember.confidence, 1_000);
        assert_eq!(remember.payload, PayloadRequest::None);
        let r = memory
            .remember(remember, &mut sion_core::rng::seeded(0))
            .unwrap();
        assert_eq!(
            serde_json::from_str::<RememberResponse>(&serde_json::to_string(&r).unwrap()).unwrap(),
            r
        );
        let audit = AuditRequest {
            old_size: 0,
            new_size: None,
        };
        assert_eq!(
            serde_json::from_str::<AuditRequest>(&serde_json::to_string(&audit).unwrap()).unwrap(),
            audit
        );
    }
}
