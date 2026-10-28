//! Memory atoms: the unit of memory, its canonical encoding, its content
//! address and its validation rules.
//!
//! An atom is Hotgraph's Reality Atom made integer-only and extended with
//! space: a statement with valid time, capture time, an optional box in a
//! frame, confidence in permille, provenance, typed links, a policy and a
//! payload. It is encoded with Borsh (field order as declared below, enum
//! variants by declaration index) and addressed by
//! `MemoryAtomId = H("sion/mem/atom", borsh(atom))`.
//!
//! Two things are deliberately not fields:
//!
//! - **Belief.** What anyone believes about an atom is computed by folding
//!   revision and conflict atoms ([`crate::belief`]); nothing is mutated.
//! - **Transaction time (`known_at`).** The ledger assigns it: the Pulse of
//!   the first commit whose size covers the atom ([`crate::log::known_at`]).
//!
//! # Limits
//!
//! [`MemoryAtom::validate`] bounds every variable-size field so an atom
//! stays small and its encoding cannot be used to exhaust a verifier:
//!
//! | Field | Limit |
//! |---|---|
//! | subject, predicate, entity value | 1 to [`MAX_NAME_BYTES`] (256) bytes of UTF-8 |
//! | text value, bytes value | [`MAX_TEXT_BYTES`] (4,096) bytes |
//! | revision reason | [`MAX_REASON_BYTES`] (1,024) bytes |
//! | source locator | [`MAX_LOCATOR_BYTES`] (1,024) bytes |
//! | decimal scale | at most [`MAX_DECIMAL_SCALE`] (38, the digits of an `i128`) |
//! | revision targets | 1 to [`MAX_TARGETS`] (256), strictly increasing |
//! | conflict members | 2 to [`MAX_MEMBERS`] (64), strictly increasing |
//! | links | at most [`MAX_LINKS`] (64), strength at most 1,000 permille |
//! | sources, evidence spans | at most [`MAX_SOURCES`] (32) and [`MAX_EVIDENCE`] (64); spans non-empty |
//! | public payload, sealed ciphertext | [`MAX_PAYLOAD_BYTES`] (65,536) bytes, plus the 16-byte tag when sealed |
//!
//! It also rejects: confidence above 1,000; an empty valid interval
//! (`end <= start`); a box with `min > max` on any axis; space in the
//! reserved null frame; a revision or conflict statement in an atom of
//! another class, or another statement in a Revision or Conflict atom; a
//! Derived atom without a `derived_from` link; a Simulation atom not
//! labelled simulation-only (its initial belief is Simulated, and its
//! labels must not say otherwise); an atom tainted as poisoned or
//! injection-prone but labelled usable for planning; and a sealed payload
//! whose key is not the atom's erasure key.

use std::collections::BTreeSet;
use std::fmt;

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use sion_core::encode::encode;
use sion_core::hash::{hash_domain, Hash32};
use sion_core::ids::AccountId;
use sion_core::merkle;
use thiserror::Error;

use crate::belief::BeliefState;
use crate::geom::{Box3, Interval, TimeRange};
use crate::hexser::hex_bytes;
use crate::index::BUCKET_NS;

/// Maximum bytes in a subject, a predicate or an entity value.
pub const MAX_NAME_BYTES: usize = 256;
/// Maximum bytes in a text or bytes value.
pub const MAX_TEXT_BYTES: usize = 4_096;
/// Maximum bytes in a revision's reason.
pub const MAX_REASON_BYTES: usize = 1_024;
/// Maximum bytes in a source locator.
pub const MAX_LOCATOR_BYTES: usize = 1_024;
/// Maximum targets of one revision.
pub const MAX_TARGETS: usize = 256;
/// Maximum members of one conflict.
pub const MAX_MEMBERS: usize = 64;
/// Maximum links of one atom.
pub const MAX_LINKS: usize = 64;
/// Maximum sources of one atom.
pub const MAX_SOURCES: usize = 32;
/// Maximum evidence spans of one atom.
pub const MAX_EVIDENCE: usize = 64;
/// Maximum plaintext bytes of a public or sealed payload. Larger content
/// (maps, splats, transcripts) is kept elsewhere as a [`Payload::Commitment`].
pub const MAX_PAYLOAD_BYTES: usize = 65_536;
/// Bytes the AEAD adds to a sealed payload.
pub const SEAL_TAG_BYTES: usize = 16;
/// Largest decimal scale (an `i128` has 38 decimal digits).
pub const MAX_DECIMAL_SCALE: u8 = 38;
/// Confidence and link strength are permille.
pub const PERMILLE: u16 = 1_000;

macro_rules! hash_newtype {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(
            Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default,
            BorshSerialize, BorshDeserialize, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(pub Hash32);

        impl $name {
            /// The underlying digest.
            pub fn as_hash(&self) -> &Hash32 {
                &self.0
            }

            /// The first eight hex characters, for logs and human-readable output.
            pub fn short(&self) -> String {
                self.0.short()
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({})", stringify!($name), self.0.short())
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(&self.0, f)
            }
        }
    };
}

hash_newtype!(
    /// A memory atom's content address: `H("sion/mem/atom", borsh(atom))`.
    /// Distinct from [`sion_core::AtomId`], which names trust atoms in
    /// assurance formulas.
    MemoryAtomId
);

hash_newtype!(
    /// A coordinate frame (a map, a building, a robot's body). Frames are
    /// registered by Transform atoms; the id is usually [`FrameId::named`].
    FrameId
);

hash_newtype!(
    /// The id of a memory space's epoch key ([`crate::seal::EpochKey::id`]):
    /// what sealed payloads, erasure keys and erasure atoms refer to.
    EpochKeyId
);

impl MemoryAtomId {
    /// The domain tag of atom ids.
    pub const DOMAIN: &'static str = "sion/mem/atom";

    /// The atom's leaf in its writer's log: `sion_core::merkle::leaf_hash(id)`.
    pub fn leaf(&self) -> Hash32 {
        merkle::leaf_hash(&self.0 .0)
    }
}

impl FrameId {
    /// The reserved frame of atoms without space.
    pub const NULL: FrameId = FrameId(Hash32::ZERO);

    /// The domain tag of named frames.
    pub const DOMAIN: &'static str = "sion/mem/frame";

    /// The frame named `name`: `H("sion/mem/frame", name)`.
    pub fn named(name: &str) -> FrameId {
        FrameId(hash_domain(Self::DOMAIN, name.as_bytes()))
    }

    /// Whether this is the reserved null frame.
    pub fn is_null(&self) -> bool {
        *self == FrameId::NULL
    }
}

/// What kind of memory an atom is: Hotgraph's ten agent memory kinds, then
/// the kinds robots need. The Borsh discriminant is the declaration index.
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
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    /// Something that happened to or around the agent.
    Episodic,
    /// A general fact.
    Semantic,
    /// How to do something.
    Procedural,
    /// What someone prefers.
    Preference,
    /// Something to achieve.
    Goal,
    /// Steps towards a goal.
    Plan,
    /// The agent's conclusion about its own experience.
    Reflection,
    /// A correction of earlier memory.
    Correction,
    /// A relation between entities.
    Relationship,
    /// The state of the world as the agent understands it.
    WorldState,
    /// A sensor observation.
    Observation,
    /// What a sensor saw, including that a volume was empty.
    Coverage,
    /// A tile of a map (usually a commitment to the tile's blob).
    MapTile,
    /// A pose of a robot or an object.
    Pose,
    /// A path through space over time.
    Trajectory,
    /// A transform between frames, which registers a frame.
    Transform,
    /// A learned skill or policy (usually a commitment to its weights).
    Skill,
}

impl MemoryKind {
    /// Every kind, in declaration order.
    pub const ALL: [MemoryKind; 17] = [
        MemoryKind::Episodic,
        MemoryKind::Semantic,
        MemoryKind::Procedural,
        MemoryKind::Preference,
        MemoryKind::Goal,
        MemoryKind::Plan,
        MemoryKind::Reflection,
        MemoryKind::Correction,
        MemoryKind::Relationship,
        MemoryKind::WorldState,
        MemoryKind::Observation,
        MemoryKind::Coverage,
        MemoryKind::MapTile,
        MemoryKind::Pose,
        MemoryKind::Trajectory,
        MemoryKind::Transform,
        MemoryKind::Skill,
    ];
}

/// How an atom came to be (Hotgraph's claim types plus revisions and
/// conflicts). The class fixes the atom's initial belief state.
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
#[serde(rename_all = "snake_case")]
pub enum AtomClass {
    /// Captured by a sensor or read from a source.
    Observation,
    /// Asserted by the writer.
    Assertion,
    /// Written by an agent about its own work or users.
    AgentMemory,
    /// An event that happened.
    Event,
    /// A summary of other atoms.
    Summary,
    /// The output of a simulation; never fact. Starts Simulated.
    Simulation,
    /// A hypothesis awaiting review. Starts Candidate.
    Hypothesis,
    /// Derived from other atoms; needs a `derived_from` link.
    Derived,
    /// A revision of other atoms' belief state.
    Revision,
    /// A record that atoms disagree.
    Conflict,
}

/// A typed value. Integer-only: decimals are a mantissa and a base-10 scale.
#[derive(
    Clone, Debug, PartialEq, Eq, Hash, BorshSerialize, BorshDeserialize, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Value {
    /// A reference to an entity by name.
    Entity(String),
    /// Free text.
    Text(String),
    /// An integer.
    Integer(i128),
    /// `mantissa × 10^(−scale)`.
    Decimal {
        /// The digits.
        mantissa: i128,
        /// The number of digits after the decimal point.
        scale: u8,
    },
    /// A boolean.
    Boolean(bool),
    /// Raw bytes.
    Bytes(#[serde(with = "hex_bytes")] Vec<u8>),
    /// A digest.
    Hash(Hash32),
}

/// How atoms in a conflict disagree.
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
#[serde(rename_all = "snake_case")]
pub enum ConflictKind {
    /// They place the same thing in different places.
    Geometric,
    /// They make incompatible claims.
    Semantic,
    /// One says something exists, another that the volume was empty.
    Existence,
    /// Their sources disagree.
    SourceDisagreement,
    /// They claim incompatible values over overlapping valid times.
    ValidTimeOverlap,
}

/// What an atom says.
#[derive(
    Clone, Debug, PartialEq, Eq, Hash, BorshSerialize, BorshDeserialize, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Statement {
    /// Subject, predicate and a typed object.
    Claim {
        /// What the claim is about.
        subject: String,
        /// The relation or attribute.
        predicate: String,
        /// The value.
        object: Value,
    },
    /// A new belief state for other atoms. Nothing is mutated: belief is a
    /// fold over revisions ([`crate::belief`]).
    Revision {
        /// The revised atoms, strictly increasing.
        targets: Vec<MemoryAtomId>,
        /// Their new state.
        state: BeliefState,
        /// When they stopped holding, if the revision closes their validity.
        end_valid: Option<u64>,
        /// Why.
        reason: String,
    },
    /// Atoms that disagree; both sides stay.
    Conflict {
        /// The disagreeing atoms, at least two, strictly increasing.
        members: Vec<MemoryAtomId>,
        /// How they disagree.
        kind: ConflictKind,
    },
    /// The record that an epoch key was destroyed (FORGET).
    Erasure {
        /// The destroyed key.
        key: EpochKeyId,
    },
    /// A reference to content kept elsewhere (a document, a transcript).
    Content(Hash32),
}

impl Statement {
    /// A claim.
    pub fn claim(subject: impl Into<String>, predicate: impl Into<String>, object: Value) -> Self {
        Statement::Claim {
            subject: subject.into(),
            predicate: predicate.into(),
            object,
        }
    }

    /// A revision of `targets` (sorted and deduplicated) to `state`.
    pub fn revision(
        targets: impl IntoIterator<Item = MemoryAtomId>,
        state: BeliefState,
        reason: impl Into<String>,
    ) -> Self {
        let targets: BTreeSet<MemoryAtomId> = targets.into_iter().collect();
        Statement::Revision {
            targets: targets.into_iter().collect(),
            state,
            end_valid: None,
            reason: reason.into(),
        }
    }

    /// A conflict among `members` (sorted and deduplicated).
    pub fn conflict(members: impl IntoIterator<Item = MemoryAtomId>, kind: ConflictKind) -> Self {
        let members: BTreeSet<MemoryAtomId> = members.into_iter().collect();
        Statement::Conflict {
            members: members.into_iter().collect(),
            kind,
        }
    }
}

/// Where a timestamp came from.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    BorshSerialize,
    BorshDeserialize,
    Serialize,
    Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ClockSource {
    /// Not stated.
    #[default]
    Unknown,
    /// A monotonic device clock, anchored to wall time by the writer.
    Monotonic,
    /// The host's system clock.
    System,
    /// NTP.
    Ntp,
    /// IEEE 1588 PTP.
    Ptp,
    /// A satellite navigation receiver.
    Gnss,
}

/// The clock behind an atom's valid time.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Hash,
    Default,
    BorshSerialize,
    BorshDeserialize,
    Serialize,
    Deserialize,
)]
pub struct Clock {
    /// The source.
    pub source: ClockSource,
    /// Its uncertainty, in nanoseconds.
    pub uncertainty_ns: u64,
}

/// Where an atom is: a box in a frame, in millimetres.
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
pub struct Space {
    /// The frame (never [`FrameId::NULL`]).
    pub frame: FrameId,
    /// The box; a point is a box with `min == max`. The index uses this box
    /// as given: a writer that wants its uncertainty searched widens the box.
    pub bbox: Box3,
    /// Positional uncertainty, in millimetres.
    pub uncertainty_mm: u32,
}

/// A source the atom rests on.
#[derive(
    Clone, Debug, PartialEq, Eq, Hash, BorshSerialize, BorshDeserialize, Serialize, Deserialize,
)]
pub struct SourceRef {
    /// The source's content hash.
    pub content: Hash32,
    /// Where to find it (a URI, a path, a sensor topic).
    pub locator: String,
}

/// A range within a source.
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
#[serde(rename_all = "snake_case")]
pub enum Span {
    /// Bytes `[start, end)` of a document or stream.
    Bytes {
        /// First byte.
        start: u64,
        /// First byte after the span.
        end: u64,
    },
    /// Nanoseconds `[start, end)` of a sensor stream.
    Time {
        /// First instant.
        start: u64,
        /// First instant after the span.
        end: u64,
    },
}

/// Evidence for the atom: a span of a source.
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
pub struct EvidenceSpan {
    /// The source's content hash.
    pub source: Hash32,
    /// The span within it.
    pub span: Span,
}

/// What produced the atom.
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
pub struct ExtractionTrace {
    /// Hash of the model's weights or identifier.
    pub model: Hash32,
    /// Hash of the code that ran it.
    pub code: Hash32,
}

/// Where an atom came from.
#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    Hash,
    Default,
    BorshSerialize,
    BorshDeserialize,
    Serialize,
    Deserialize,
)]
pub struct Provenance {
    /// Sources by content hash.
    #[serde(default)]
    pub sources: Vec<SourceRef>,
    /// Byte or time ranges in those sources.
    #[serde(default)]
    pub evidence: Vec<EvidenceSpan>,
    /// The model and code that extracted it.
    #[serde(default)]
    pub extraction: Option<ExtractionTrace>,
    /// A digest of a device attestation (a secure element's quote).
    #[serde(default)]
    pub attestation: Option<Hash32>,
}

/// How a link relates an atom to its target.
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
#[serde(rename_all = "snake_case")]
pub enum LinkKind {
    /// This atom replaces the target (record only; belief changes need a revision).
    Supersedes,
    /// This atom contradicts the target (record only; see conflict atoms).
    Contradicts,
    /// This atom was derived from the target.
    DerivedFrom,
    /// This atom supports the target.
    Supports,
    /// This atom assumes the target.
    Assumes,
    /// The target's event caused this one.
    Causes,
    /// The target made this possible.
    Enables,
    /// This atom invalidates the target.
    Invalidates,
}

/// A typed edge to another atom.
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
pub struct Link {
    /// The relation.
    pub kind: LinkKind,
    /// The other atom.
    pub target: MemoryAtomId,
    /// Strength in permille.
    pub strength: u16,
}

/// Who may read an atom.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    BorshSerialize,
    BorshDeserialize,
    Serialize,
    Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    /// Anyone.
    Public,
    /// The memory space's members.
    #[default]
    Internal,
    /// Named members only.
    Restricted,
}

/// How far an atom's content can be trusted as input to a model.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    BorshSerialize,
    BorshDeserialize,
    Serialize,
    Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Taint {
    /// From a trusted source.
    Trusted,
    /// From an unvetted source.
    #[default]
    Untrusted,
    /// May carry instructions aimed at a model.
    PromptInjectionRisk,
    /// Known to be adversarial.
    Poisoned,
}

/// Whether a planner may rely on an atom.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    BorshSerialize,
    BorshDeserialize,
    Serialize,
    Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum AiUsage {
    /// Safe to plan with.
    SafeForPlanning,
    /// Usable with caution.
    #[default]
    UseWithCaution,
    /// Not to be planned with.
    UnsafeForPlanning,
    /// Only for simulations; never fact.
    SimulationOnly,
}

/// How long an atom should be kept readable.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    BorshSerialize,
    BorshDeserialize,
    Serialize,
    Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum RetentionClass {
    /// Dropped after use.
    Ephemeral,
    /// For one session.
    Session,
    /// The memory space's standard term.
    #[default]
    Standard,
    /// Kept long-term.
    LongTerm,
    /// Kept as long as a regulation requires (logs of high-risk systems).
    Regulatory,
}

/// An atom's labels.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Hash,
    Default,
    BorshSerialize,
    BorshDeserialize,
    Serialize,
    Deserialize,
)]
pub struct AtomPolicy {
    /// Who may read it.
    #[serde(default)]
    pub permission: Permission,
    /// How trustworthy its content is as model input.
    #[serde(default)]
    pub taint: Taint,
    /// Whether planners may rely on it.
    #[serde(default)]
    pub ai_usage: AiUsage,
    /// How long it is kept.
    #[serde(default)]
    pub retention: RetentionClass,
    /// The epoch key whose destruction makes its payload unreadable.
    #[serde(default)]
    pub erasure_key: Option<EpochKeyId>,
}

/// A payload encrypted under a per-atom key derived from an epoch key
/// ([`crate::seal`]).
#[derive(
    Clone, Debug, PartialEq, Eq, Hash, BorshSerialize, BorshDeserialize, Serialize, Deserialize,
)]
pub struct SealedPayload {
    /// The epoch key it is sealed under.
    pub key: EpochKeyId,
    /// The ChaCha20-Poly1305 nonce.
    pub nonce: [u8; 12],
    /// Ciphertext with its 16-byte tag.
    #[serde(with = "hex_bytes")]
    pub ciphertext: Vec<u8>,
}

/// An atom's content.
#[derive(
    Clone, Debug, PartialEq, Eq, Hash, BorshSerialize, BorshDeserialize, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Payload {
    /// Plain bytes (empty for none).
    Public(#[serde(with = "hex_bytes")] Vec<u8>),
    /// Encrypted bytes.
    Sealed(SealedPayload),
    /// A commitment to a blob kept elsewhere (a map tile, a splat, a transcript).
    Commitment {
        /// The blob's hash.
        blob: Hash32,
        /// Its size in bytes.
        size: u64,
    },
}

impl Default for Payload {
    fn default() -> Self {
        Payload::Public(Vec::new())
    }
}

/// One memory: the unit a writer appends to its log.
#[derive(
    Clone, Debug, PartialEq, Eq, Hash, BorshSerialize, BorshDeserialize, Serialize, Deserialize,
)]
pub struct MemoryAtom {
    /// The writer, whose log holds the atom.
    pub writer: AccountId,
    /// The atom's position in its writer's log (its leaf index).
    pub seq: u64,
    /// What kind of memory it is.
    pub kind: MemoryKind,
    /// How it came to be.
    pub class: AtomClass,
    /// What it says.
    pub statement: Statement,
    /// When it held.
    pub valid: Interval,
    /// The clock behind `valid`.
    #[serde(default)]
    pub clock: Clock,
    /// When it was captured, in nanoseconds since the Unix epoch.
    pub observed_at: u64,
    /// Where it is, if anywhere.
    #[serde(default)]
    pub space: Option<Space>,
    /// Confidence in permille (0 to 1,000).
    pub confidence: u16,
    /// Where it came from.
    #[serde(default)]
    pub provenance: Provenance,
    /// Typed edges to other atoms.
    #[serde(default)]
    pub links: Vec<Link>,
    /// Labels.
    #[serde(default)]
    pub policy: AtomPolicy,
    /// Content.
    #[serde(default)]
    pub payload: Payload,
}

/// Why an atom is invalid.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum AtomError {
    /// A permille value above 1,000.
    #[error("{field} is {value}, above 1,000 permille")]
    Permille {
        /// Which field.
        field: &'static str,
        /// Its value.
        value: u16,
    },
    /// A closed valid interval whose end is not after its start.
    #[error("valid interval [{start}, {end}) is empty")]
    EmptyInterval {
        /// Start.
        start: u64,
        /// End.
        end: u64,
    },
    /// A box with `min > max` on some axis.
    #[error("box has min greater than max")]
    InvalidBox,
    /// A `space` in the null frame, which is reserved for atoms without space.
    #[error("the null frame is reserved for atoms without space")]
    ReservedFrame,
    /// A string or byte field over its limit.
    #[error("{field} is {len} bytes, limit {max}")]
    TooLong {
        /// Which field.
        field: &'static str,
        /// Its length.
        len: usize,
        /// The limit.
        max: usize,
    },
    /// A required string that is empty.
    #[error("{0} is empty")]
    Empty(&'static str),
    /// A list over its limit.
    #[error("{field} has {len} items, limit {max}")]
    TooMany {
        /// Which list.
        field: &'static str,
        /// Its length.
        len: usize,
        /// The limit.
        max: usize,
    },
    /// A revision without targets.
    #[error("a revision needs at least one target")]
    NoTargets,
    /// A conflict with fewer than two members.
    #[error("a conflict needs at least two members")]
    TooFewMembers,
    /// A list that must be strictly increasing (sorted, no duplicates) is not.
    #[error("{0} must be strictly increasing")]
    NotCanonical(&'static str),
    /// The statement does not fit the class (revisions and conflicts must agree).
    #[error("statement and class disagree")]
    ClassMismatch,
    /// A derived atom without a `derived_from` link.
    #[error("a derived atom needs a derived_from link")]
    MissingDerivation,
    /// A simulation atom not labelled simulation-only, which would let it pass as fact.
    #[error("a simulation atom must be labelled simulation-only")]
    SimulationLabeledAsFact,
    /// An atom tainted as poisoned or injection-prone but labelled usable for planning.
    #[error("a tainted atom cannot be labelled usable for planning")]
    TaintedForPlanning,
    /// A decimal with more digits after the point than an `i128` has.
    #[error("decimal scale {0} exceeds 38")]
    DecimalScale(u8),
    /// An evidence span whose end is not after its start.
    #[error("evidence span is empty")]
    EmptySpan,
    /// A sealed payload whose key is not the atom's erasure key.
    #[error("a sealed payload's key must be the atom's erasure key")]
    ErasureKeyMismatch,
}

fn check_len(field: &'static str, len: usize, max: usize) -> Result<(), AtomError> {
    if len > max {
        Err(AtomError::TooLong { field, len, max })
    } else {
        Ok(())
    }
}

fn check_count(field: &'static str, len: usize, max: usize) -> Result<(), AtomError> {
    if len > max {
        Err(AtomError::TooMany { field, len, max })
    } else {
        Ok(())
    }
}

fn check_name(field: &'static str, name: &str) -> Result<(), AtomError> {
    if name.is_empty() {
        return Err(AtomError::Empty(field));
    }
    check_len(field, name.len(), MAX_NAME_BYTES)
}

fn strictly_increasing(ids: &[MemoryAtomId]) -> bool {
    ids.windows(2).all(|pair| pair[0] < pair[1])
}

impl MemoryAtom {
    /// Starts building an atom (see [`AtomBuilder`] for the defaults).
    pub fn builder(
        writer: AccountId,
        seq: u64,
        kind: MemoryKind,
        class: AtomClass,
        statement: Statement,
    ) -> AtomBuilder {
        AtomBuilder::new(writer, seq, kind, class, statement)
    }

    /// The content address: `H("sion/mem/atom", borsh(self))`.
    pub fn id(&self) -> MemoryAtomId {
        MemoryAtomId(hash_domain(MemoryAtomId::DOMAIN, &encode(self)))
    }

    /// The atom's leaf in its writer's log.
    pub fn leaf(&self) -> Hash32 {
        self.id().leaf()
    }

    /// The frame the index files the atom under ([`FrameId::NULL`] without space).
    pub fn frame(&self) -> FrameId {
        self.space.map_or(FrameId::NULL, |space| space.frame)
    }

    /// The box the index files the atom under ([`Box3::NULL`] without space).
    pub fn bbox(&self) -> Box3 {
        self.space.map_or(Box3::NULL, |space| space.bbox)
    }

    /// The valid interval as an index time range.
    pub fn time_range(&self) -> TimeRange {
        self.valid.range()
    }

    /// Whether the index files the atom in the time-bucketed forest: its
    /// interval is closed, and it starts and ends (`end − 1`) in the same
    /// bucket of [`BUCKET_NS`].
    pub fn is_short_lived(&self) -> bool {
        match self.valid.end {
            Some(end) if end > self.valid.start => {
                self.valid.start / BUCKET_NS == (end - 1) / BUCKET_NS
            }
            _ => false,
        }
    }

    /// The `(subject, predicate)` of a claim.
    pub fn claim_key(&self) -> Option<(&str, &str)> {
        match &self.statement {
            Statement::Claim {
                subject, predicate, ..
            } => Some((subject.as_str(), predicate.as_str())),
            _ => None,
        }
    }

    /// Every atom this one names: revision targets, conflict members and
    /// link targets. The link map files the atom under each.
    pub fn link_targets(&self) -> BTreeSet<MemoryAtomId> {
        let mut targets: BTreeSet<MemoryAtomId> = self.links.iter().map(|l| l.target).collect();
        match &self.statement {
            Statement::Revision { targets: t, .. } => targets.extend(t.iter().copied()),
            Statement::Conflict { members, .. } => targets.extend(members.iter().copied()),
            _ => {}
        }
        targets
    }

    /// The belief state the atom starts in, before any revision.
    pub fn initial_belief(&self) -> BeliefState {
        BeliefState::initial(self.class)
    }

    /// Checks every rule in the module documentation.
    pub fn validate(&self) -> Result<(), AtomError> {
        if self.confidence > PERMILLE {
            return Err(AtomError::Permille {
                field: "confidence",
                value: self.confidence,
            });
        }
        if let Some(end) = self.valid.end {
            if end <= self.valid.start {
                return Err(AtomError::EmptyInterval {
                    start: self.valid.start,
                    end,
                });
            }
        }
        if let Some(space) = &self.space {
            if space.frame.is_null() {
                return Err(AtomError::ReservedFrame);
            }
            if !space.bbox.is_valid() {
                return Err(AtomError::InvalidBox);
            }
        }
        self.validate_statement()?;
        self.validate_provenance()?;
        check_count("links", self.links.len(), MAX_LINKS)?;
        for link in &self.links {
            if link.strength > PERMILLE {
                return Err(AtomError::Permille {
                    field: "link strength",
                    value: link.strength,
                });
            }
        }
        if self.class == AtomClass::Derived
            && !self.links.iter().any(|l| l.kind == LinkKind::DerivedFrom)
        {
            return Err(AtomError::MissingDerivation);
        }
        if self.class == AtomClass::Simulation && self.policy.ai_usage != AiUsage::SimulationOnly {
            return Err(AtomError::SimulationLabeledAsFact);
        }
        if matches!(
            self.policy.taint,
            Taint::Poisoned | Taint::PromptInjectionRisk
        ) && matches!(
            self.policy.ai_usage,
            AiUsage::SafeForPlanning | AiUsage::UseWithCaution
        ) {
            return Err(AtomError::TaintedForPlanning);
        }
        match &self.payload {
            Payload::Public(bytes) => check_len("payload", bytes.len(), MAX_PAYLOAD_BYTES)?,
            Payload::Sealed(sealed) => {
                check_len(
                    "sealed payload",
                    sealed.ciphertext.len(),
                    MAX_PAYLOAD_BYTES + SEAL_TAG_BYTES,
                )?;
                if self.policy.erasure_key != Some(sealed.key) {
                    return Err(AtomError::ErasureKeyMismatch);
                }
            }
            Payload::Commitment { .. } => {}
        }
        Ok(())
    }

    fn validate_statement(&self) -> Result<(), AtomError> {
        let is_revision = matches!(self.statement, Statement::Revision { .. });
        let is_conflict = matches!(self.statement, Statement::Conflict { .. });
        if is_revision != (self.class == AtomClass::Revision)
            || is_conflict != (self.class == AtomClass::Conflict)
        {
            return Err(AtomError::ClassMismatch);
        }
        match &self.statement {
            Statement::Claim {
                subject,
                predicate,
                object,
            } => {
                check_name("subject", subject)?;
                check_name("predicate", predicate)?;
                match object {
                    Value::Entity(name) => check_name("entity", name)?,
                    Value::Text(text) => check_len("text", text.len(), MAX_TEXT_BYTES)?,
                    Value::Bytes(bytes) => check_len("bytes", bytes.len(), MAX_TEXT_BYTES)?,
                    Value::Decimal { scale, .. } if *scale > MAX_DECIMAL_SCALE => {
                        return Err(AtomError::DecimalScale(*scale));
                    }
                    _ => {}
                }
            }
            Statement::Revision {
                targets, reason, ..
            } => {
                if targets.is_empty() {
                    return Err(AtomError::NoTargets);
                }
                check_count("revision targets", targets.len(), MAX_TARGETS)?;
                if !strictly_increasing(targets) {
                    return Err(AtomError::NotCanonical("revision targets"));
                }
                check_len("reason", reason.len(), MAX_REASON_BYTES)?;
            }
            Statement::Conflict { members, .. } => {
                if members.len() < 2 {
                    return Err(AtomError::TooFewMembers);
                }
                check_count("conflict members", members.len(), MAX_MEMBERS)?;
                if !strictly_increasing(members) {
                    return Err(AtomError::NotCanonical("conflict members"));
                }
            }
            Statement::Erasure { .. } | Statement::Content(_) => {}
        }
        Ok(())
    }

    fn validate_provenance(&self) -> Result<(), AtomError> {
        let p = &self.provenance;
        check_count("sources", p.sources.len(), MAX_SOURCES)?;
        for source in &p.sources {
            check_len("locator", source.locator.len(), MAX_LOCATOR_BYTES)?;
        }
        check_count("evidence", p.evidence.len(), MAX_EVIDENCE)?;
        for evidence in &p.evidence {
            let (start, end) = match evidence.span {
                Span::Bytes { start, end } | Span::Time { start, end } => (start, end),
            };
            if end <= start {
                return Err(AtomError::EmptySpan);
            }
        }
        Ok(())
    }
}

/// Builds atoms with sensible defaults: valid from 0, open-ended; captured at
/// 0; no space; confidence 1,000; no provenance, links or payload; the
/// default policy (internal, untrusted, use with caution, standard
/// retention), except that Simulation atoms default to simulation-only.
#[derive(Clone, Debug)]
pub struct AtomBuilder {
    atom: MemoryAtom,
}

impl AtomBuilder {
    /// Starts an atom.
    pub fn new(
        writer: AccountId,
        seq: u64,
        kind: MemoryKind,
        class: AtomClass,
        statement: Statement,
    ) -> Self {
        let mut policy = AtomPolicy::default();
        if class == AtomClass::Simulation {
            policy.ai_usage = AiUsage::SimulationOnly;
        }
        AtomBuilder {
            atom: MemoryAtom {
                writer,
                seq,
                kind,
                class,
                statement,
                valid: Interval::starting(0),
                clock: Clock::default(),
                observed_at: 0,
                space: None,
                confidence: PERMILLE,
                provenance: Provenance::default(),
                links: Vec::new(),
                policy,
                payload: Payload::default(),
            },
        }
    }

    /// Sets the valid interval.
    pub fn valid(mut self, valid: Interval) -> Self {
        self.atom.valid = valid;
        self
    }

    /// Sets the clock.
    pub fn clock(mut self, source: ClockSource, uncertainty_ns: u64) -> Self {
        self.atom.clock = Clock {
            source,
            uncertainty_ns,
        };
        self
    }

    /// Sets the capture time.
    pub fn observed_at(mut self, observed_at: u64) -> Self {
        self.atom.observed_at = observed_at;
        self
    }

    /// Places the atom in a box of a frame.
    pub fn space(mut self, frame: FrameId, bbox: Box3, uncertainty_mm: u32) -> Self {
        self.atom.space = Some(Space {
            frame,
            bbox,
            uncertainty_mm,
        });
        self
    }

    /// Places the atom at a point of a frame.
    pub fn at(self, frame: FrameId, point: [i64; 3]) -> Self {
        self.space(frame, Box3::point(point), 0)
    }

    /// Copies another atom's space and valid interval, so the range queries
    /// that find it also find this atom (as revisions of it should be).
    pub fn inherit_place(mut self, other: &MemoryAtom) -> Self {
        self.atom.space = other.space;
        self.atom.valid = other.valid;
        self
    }

    /// Sets the confidence in permille.
    pub fn confidence(mut self, permille: u16) -> Self {
        self.atom.confidence = permille;
        self
    }

    /// Adds a source.
    pub fn source(mut self, content: Hash32, locator: impl Into<String>) -> Self {
        self.atom.provenance.sources.push(SourceRef {
            content,
            locator: locator.into(),
        });
        self
    }

    /// Adds an evidence span.
    pub fn evidence(mut self, source: Hash32, span: Span) -> Self {
        self.atom
            .provenance
            .evidence
            .push(EvidenceSpan { source, span });
        self
    }

    /// Sets the extraction trace.
    pub fn extraction(mut self, model: Hash32, code: Hash32) -> Self {
        self.atom.provenance.extraction = Some(ExtractionTrace { model, code });
        self
    }

    /// Sets the device-attestation digest.
    pub fn attestation(mut self, digest: Hash32) -> Self {
        self.atom.provenance.attestation = Some(digest);
        self
    }

    /// Adds a link.
    pub fn link(mut self, kind: LinkKind, target: MemoryAtomId, strength: u16) -> Self {
        self.atom.links.push(Link {
            kind,
            target,
            strength,
        });
        self
    }

    /// Replaces the policy.
    pub fn policy(mut self, policy: AtomPolicy) -> Self {
        self.atom.policy = policy;
        self
    }

    /// Sets the permission.
    pub fn permission(mut self, permission: Permission) -> Self {
        self.atom.policy.permission = permission;
        self
    }

    /// Sets the taint.
    pub fn taint(mut self, taint: Taint) -> Self {
        self.atom.policy.taint = taint;
        self
    }

    /// Sets whether planners may rely on the atom.
    pub fn ai_usage(mut self, ai_usage: AiUsage) -> Self {
        self.atom.policy.ai_usage = ai_usage;
        self
    }

    /// Sets the retention class.
    pub fn retention(mut self, retention: RetentionClass) -> Self {
        self.atom.policy.retention = retention;
        self
    }

    /// Sets the payload; a sealed payload also sets the erasure key to its key.
    pub fn payload(mut self, payload: Payload) -> Self {
        if let Payload::Sealed(sealed) = &payload {
            self.atom.policy.erasure_key = Some(sealed.key);
        }
        self.atom.payload = payload;
        self
    }

    /// The atom as built so far, unvalidated (for tests of validation).
    pub fn build_unchecked(self) -> MemoryAtom {
        self.atom
    }

    /// Validates and returns the atom.
    pub fn build(self) -> Result<MemoryAtom, AtomError> {
        self.atom.validate()?;
        Ok(self.atom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn writer() -> AccountId {
        AccountId::derive(b"robot-1")
    }

    fn claim(seq: u64) -> AtomBuilder {
        MemoryAtom::builder(
            writer(),
            seq,
            MemoryKind::Observation,
            AtomClass::Observation,
            Statement::claim("pallet-7", "location", Value::Entity("aisle-3".into())),
        )
        .valid(Interval::closed(1_000, 2_000))
        .observed_at(1_500)
    }

    fn id(n: u8) -> MemoryAtomId {
        MemoryAtomId(Hash32([n; 32]))
    }

    #[test]
    fn ids_are_content_addresses() {
        let a = claim(0).build().unwrap();
        let b = claim(0).build().unwrap();
        assert_eq!(a.id(), b.id());
        let c = claim(1).build().unwrap();
        assert_ne!(a.id(), c.id());
        assert_eq!(
            a.id().0,
            hash_domain("sion/mem/atom", &sion_core::encode(&a))
        );
        assert_eq!(a.leaf(), merkle::leaf_hash(&a.id().0 .0));
        let round: MemoryAtom = sion_core::decode(&sion_core::encode(&a)).unwrap();
        assert_eq!(round, a);
        assert!(format!("{:?}", a.id()).starts_with("MemoryAtomId("));
        assert_eq!(a.id().to_string().len(), 64);
        assert_eq!(a.id().short().len(), 8);
        assert_eq!(a.id().as_hash(), &a.id().0);
    }

    #[test]
    fn frames_are_named_and_null_is_reserved() {
        assert_ne!(FrameId::named("warehouse"), FrameId::named("dock"));
        assert!(FrameId::NULL.is_null());
        assert!(!FrameId::named("warehouse").is_null());
        let bad = claim(0)
            .space(FrameId::NULL, Box3::point([0, 0, 0]), 0)
            .build();
        assert_eq!(bad, Err(AtomError::ReservedFrame));
    }

    #[test]
    fn validation_rejects_each_rule() {
        assert!(matches!(
            claim(0).confidence(1_001).build(),
            Err(AtomError::Permille { .. })
        ));
        assert!(matches!(
            claim(0).valid(Interval::closed(5, 5)).build(),
            Err(AtomError::EmptyInterval { .. })
        ));
        assert_eq!(
            claim(0)
                .space(FrameId::named("w"), Box3::new([1, 0, 0], [0, 0, 0]), 0)
                .build(),
            Err(AtomError::InvalidBox)
        );
        let long = "x".repeat(MAX_NAME_BYTES + 1);
        let too_long = MemoryAtom::builder(
            writer(),
            0,
            MemoryKind::Semantic,
            AtomClass::Assertion,
            Statement::claim(long, "p", Value::Boolean(true)),
        )
        .build();
        assert!(matches!(too_long, Err(AtomError::TooLong { .. })));
        let empty = MemoryAtom::builder(
            writer(),
            0,
            MemoryKind::Semantic,
            AtomClass::Assertion,
            Statement::claim("", "p", Value::Boolean(true)),
        )
        .build();
        assert_eq!(empty, Err(AtomError::Empty("subject")));
        let no_targets = MemoryAtom::builder(
            writer(),
            0,
            MemoryKind::Correction,
            AtomClass::Revision,
            Statement::revision([], BeliefState::Retracted, "wrong"),
        )
        .build();
        assert_eq!(no_targets, Err(AtomError::NoTargets));
        let unsorted = MemoryAtom::builder(
            writer(),
            0,
            MemoryKind::Correction,
            AtomClass::Revision,
            Statement::Revision {
                targets: vec![id(2), id(1)],
                state: BeliefState::Retracted,
                end_valid: None,
                reason: String::new(),
            },
        )
        .build();
        assert_eq!(unsorted, Err(AtomError::NotCanonical("revision targets")));
        let lonely = MemoryAtom::builder(
            writer(),
            0,
            MemoryKind::WorldState,
            AtomClass::Conflict,
            Statement::conflict([id(1)], ConflictKind::Existence),
        )
        .build();
        assert_eq!(lonely, Err(AtomError::TooFewMembers));
        let derived = MemoryAtom::builder(
            writer(),
            0,
            MemoryKind::Semantic,
            AtomClass::Derived,
            Statement::claim("a", "b", Value::Integer(1)),
        );
        assert_eq!(derived.clone().build(), Err(AtomError::MissingDerivation));
        assert!(derived
            .link(LinkKind::DerivedFrom, id(9), 500)
            .build()
            .is_ok());
        let simulated = MemoryAtom::builder(
            writer(),
            0,
            MemoryKind::WorldState,
            AtomClass::Simulation,
            Statement::claim("door", "open", Value::Boolean(true)),
        );
        assert!(
            simulated.clone().build().is_ok(),
            "defaults to simulation-only"
        );
        assert_eq!(
            simulated.ai_usage(AiUsage::SafeForPlanning).build(),
            Err(AtomError::SimulationLabeledAsFact)
        );
        assert_eq!(
            claim(0).taint(Taint::Poisoned).build(),
            Err(AtomError::TaintedForPlanning)
        );
        assert!(claim(0)
            .taint(Taint::Poisoned)
            .ai_usage(AiUsage::UnsafeForPlanning)
            .build()
            .is_ok());
        let mismatch = MemoryAtom::builder(
            writer(),
            0,
            MemoryKind::Correction,
            AtomClass::Assertion,
            Statement::revision([id(1)], BeliefState::Retracted, "x"),
        )
        .build();
        assert_eq!(mismatch, Err(AtomError::ClassMismatch));
        assert!(matches!(
            claim(0).link(LinkKind::Supports, id(1), 1_001).build(),
            Err(AtomError::Permille { .. })
        ));
        assert_eq!(
            claim(0)
                .evidence(Hash32::ZERO, Span::Bytes { start: 4, end: 4 })
                .build(),
            Err(AtomError::EmptySpan)
        );
        let scale = MemoryAtom::builder(
            writer(),
            0,
            MemoryKind::Semantic,
            AtomClass::Assertion,
            Statement::claim(
                "a",
                "b",
                Value::Decimal {
                    mantissa: 1,
                    scale: 39,
                },
            ),
        )
        .build();
        assert_eq!(scale, Err(AtomError::DecimalScale(39)));
        let mut many = claim(0);
        for n in 0..=MAX_LINKS {
            many = many.link(LinkKind::Supports, id(n as u8), 1);
        }
        assert!(matches!(many.build(), Err(AtomError::TooMany { .. })));
        assert!(matches!(
            claim(0)
                .payload(Payload::Public(vec![0; MAX_PAYLOAD_BYTES + 1]))
                .build(),
            Err(AtomError::TooLong { .. })
        ));
    }

    #[test]
    fn sealed_payloads_carry_their_erasure_key() {
        let sealed = SealedPayload {
            key: EpochKeyId(Hash32([3; 32])),
            nonce: [0; 12],
            ciphertext: vec![1; 20],
        };
        let atom = claim(0)
            .payload(Payload::Sealed(sealed.clone()))
            .build()
            .unwrap();
        assert_eq!(atom.policy.erasure_key, Some(sealed.key));
        let mut wrong = atom.clone();
        wrong.policy.erasure_key = None;
        assert_eq!(wrong.validate(), Err(AtomError::ErasureKeyMismatch));
    }

    #[test]
    fn short_lived_means_one_bucket() {
        let start = 5 * BUCKET_NS;
        let atom = |valid| claim(0).valid(valid).build().unwrap();
        assert!(atom(Interval::closed(start, start + BUCKET_NS)).is_short_lived());
        assert!(!atom(Interval::closed(start, start + BUCKET_NS + 1)).is_short_lived());
        assert!(!atom(Interval::starting(start)).is_short_lived());
        assert!(atom(Interval::closed(start + 10, start + 11)).is_short_lived());
    }

    #[test]
    fn accessors_report_index_keys() {
        let a = claim(0).build().unwrap();
        assert_eq!(a.frame(), FrameId::NULL);
        assert_eq!(a.bbox(), Box3::NULL);
        assert_eq!(a.time_range(), TimeRange::new(1_000, 2_000));
        assert_eq!(a.claim_key(), Some(("pallet-7", "location")));
        assert_eq!(a.initial_belief(), BeliefState::Accepted);
        let frame = FrameId::named("w");
        let placed = claim(1).at(frame, [1, 2, 3]).build().unwrap();
        assert_eq!(placed.frame(), frame);
        assert_eq!(placed.bbox(), Box3::point([1, 2, 3]));
        let revision = MemoryAtom::builder(
            writer(),
            2,
            MemoryKind::Correction,
            AtomClass::Revision,
            Statement::revision([id(5), id(4), id(5)], BeliefState::Superseded, "moved"),
        )
        .inherit_place(&placed)
        .link(LinkKind::Supports, id(6), 10)
        .build()
        .unwrap();
        assert_eq!(revision.space, placed.space);
        assert_eq!(
            revision.link_targets().into_iter().collect::<Vec<_>>(),
            vec![id(4), id(5), id(6)]
        );
        assert_eq!(revision.claim_key(), None);
        assert_eq!(MemoryKind::ALL.len(), 17);
    }

    #[test]
    fn builder_sets_every_field() {
        let atom = claim(0)
            .clock(ClockSource::Ptp, 50)
            .confidence(900)
            .source(Hash32([1; 32]), "ros://camera/front")
            .evidence(Hash32([1; 32]), Span::Time { start: 1, end: 2 })
            .extraction(Hash32([2; 32]), Hash32([3; 32]))
            .attestation(Hash32([4; 32]))
            .permission(Permission::Restricted)
            .retention(RetentionClass::Regulatory)
            .policy(AtomPolicy {
                taint: Taint::Trusted,
                ..AtomPolicy::default()
            })
            .payload(Payload::Commitment {
                blob: Hash32([5; 32]),
                size: 10,
            })
            .build()
            .unwrap();
        assert_eq!(atom.clock.source, ClockSource::Ptp);
        assert_eq!(atom.provenance.sources.len(), 1);
        assert!(atom.provenance.extraction.is_some());
        assert_eq!(atom.policy.taint, Taint::Trusted);
        let json = serde_json::to_string(&atom).unwrap();
        let back: MemoryAtom = serde_json::from_str(&json).unwrap();
        assert_eq!(back, atom);
        let unchecked = claim(0).confidence(5_000).build_unchecked();
        assert!(unchecked.validate().is_err());
    }
}
