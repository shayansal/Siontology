//! Verifiable memory for AI agents and robots.
//!
//! An agent's or a robot's memory is a **single-writer, append-only log of
//! atoms** ([`MemoryAtom`]). An atom is a statement (a claim, a revision, a
//! conflict, an erasure or a reference to content) with when it held
//! (valid time, in nanoseconds), when it was captured, where it was (an
//! optional box in a frame, in millimetres), integer confidence in permille,
//! provenance, typed links, a policy and a payload that may be public,
//! sealed or a commitment to a blob kept elsewhere. Atoms are encoded with
//! Borsh and addressed by `H("sion/mem/atom", borsh(atom))`. Everything that
//! is hashed, compared or proved is an integer.
//!
//! The ledger keeps each log append-only. A writer's [`Head`] binds the log
//! root (RFC 9162 over `leaf_hash(atom_id)` for each atom, exactly
//! `sion_core::merkle`'s) and the **index root** of the same prefix; a
//! `CommitMemory` transaction checks a consistency proof from the previous
//! head, and the Pulse in which it executes becomes the transaction time
//! (`known_at`) of the atoms it adds ([`log::known_at`]). The writer cannot
//! choose it.
//!
//! The [`index`] is canonical: a deterministic function of the log prefix,
//! specified in its module documentation so that independent
//! implementations agree. It files atoms by frame, then in a time-bucketed
//! forest of Merkle R-trees (short-lived atoms) and a long-lived R-tree, and
//! keeps a subject map and a link map. Its proofs answer "everything writer
//! W had recorded in box B during [t₁, t₂), as of head h" completely,
//! including that nothing was recorded; list every claim about a subject or
//! every revision of an atom; and convict a writer whose index leaves an
//! atom out. Completeness is relative to the log: a proof shows what was
//! recorded, never what happened.
//!
//! **Belief is computed, never stored** ([`belief`]): a fold over the
//! revision and conflict atoms that the revisers a consumer trusts wrote,
//! as of a Pulse, with a bundle of link-map proofs that lets a client check
//! the fold (including that no authorized retraction exists) without the
//! logs. Payloads can be sealed under per-atom keys derived from a memory
//! space's epoch keys, which are wrapped to members with ML-KEM-768 and
//! destroyed to forget ([`seal`]). [`cell`] has a first building block for
//! curated shared memory of the world, and [`rmp`] exposes it all through
//! the Reality Memory Protocol's operations.
//!
//! The design is section 4 of the research note *Billions of agents*
//! (`docs/book/src/research/agent-scale.md`); its semantics follow
//! Hotgraph's Reality Atoms without their floating-point confidence or
//! in-place belief updates.
//!
//! # Example
//!
//! ```
//! use sion_core::AccountId;
//! use sion_memory::{
//!     prove_range, verify_range, AtomClass, Box3, FrameId, Interval, MemoryAtom, MemoryKind,
//!     RangeQuery, Statement, TimeRange, Value, WriterLog,
//! };
//!
//! let robot = AccountId::derive(b"robot-7");
//! let warehouse = FrameId::named("warehouse-3");
//! let mut log = WriterLog::new(robot);
//! let atom = MemoryAtom::builder(
//!     robot,
//!     0,
//!     MemoryKind::Observation,
//!     AtomClass::Observation,
//!     Statement::claim("pallet-42", "at", Value::Entity("bay-12".into())),
//! )
//! .valid(Interval::closed(1_000, 2_000))
//! .at(warehouse, [12_000, 3_500, 0])
//! .confidence(950)
//! .build()
//! .unwrap();
//! log.append(atom).unwrap();
//!
//! let head = log.head(log.len()).unwrap();
//! let index = log.index(log.len()).unwrap();
//! let query = RangeQuery::new(
//!     warehouse,
//!     Box3::new([10_000, 0, 0], [15_000, 5_000, 3_000]),
//!     TimeRange::new(0, 10_000),
//! );
//! let (results, proof) = prove_range(&index, &query).unwrap();
//! verify_range(&head.index_root, &query, &results, &proof).unwrap();
//! assert_eq!(results.len(), 1);
//! ```

pub mod atom;
pub mod belief;
pub mod cell;
pub mod error;
pub mod geom;
mod hexser;
pub mod index;
pub mod log;
mod merkle;
pub mod rmp;
pub mod seal;

pub use atom::{
    AiUsage, AtomBuilder, AtomClass, AtomError, AtomPolicy, Clock, ClockSource, ConflictKind,
    EpochKeyId, EvidenceSpan, ExtractionTrace, FrameId, Link, LinkKind, MemoryAtom, MemoryAtomId,
    MemoryKind, Payload, Permission, Provenance, RetentionClass, SealedPayload, SourceRef, Space,
    Span, Statement, Taint, Value,
};
pub use belief::{
    belief_at, prove_belief, verify_belief, Belief, BeliefPolicy, BeliefProof, BeliefState, Prefer,
    ReviserEvidence, ReviserHead, ReviserLog,
};
pub use cell::{
    prove_cell_omission, verify_cell_omission, CellDigest, CellKey, CellOmissionProof, CuratedCell,
};
pub use error::{MemoryError, Result};
pub use geom::{Box3, Interval, TimeRange};
pub use index::keyed::{
    prove_links, prove_subject, verify_links, verify_subject, LinkProof, SubjectProof,
};
pub use index::map::{MapCommitment, MapLookup};
pub use index::omission::{prove_omission, verify_omission, OmissionEvidence, OmissionProof};
pub use index::range::{
    prove_range, select_atoms, verify_range, AtomEntry, ProofEntry, ProofNode, RangeProof,
    RangeQuery, TreeProof,
};
pub use index::{
    index_root, Entry, FrameValue, IndexBuilder, IndexHeader, MemoryIndex, BUCKET_NS, FANOUT,
};
pub use log::{known_at, Commit, CommitHistory, Head, LogStore, WriterLog};
pub use rmp::{
    AuditRequest, AuditResponse, ForgetRequest, Memory, PayloadRequest, RecallProof, RecallRequest,
    RecallResponse, RememberRequest, RememberResponse, ReviseRequest, ShareRequest, ShareResponse,
};
pub use seal::{EpochKey, KeyRing, WrappedEpochKey};

#[cfg(test)]
pub(crate) mod testing {
    //! Deterministic property-test configuration.

    use proptest::test_runner::{Config, RngSeed};

    /// `cases` cases from a fixed seed, so every run explores the same inputs.
    pub(crate) fn config(cases: u32, seed: u64) -> Config {
        Config {
            cases,
            rng_seed: RngSeed::Fixed(seed),
            ..Config::default()
        }
    }
}
