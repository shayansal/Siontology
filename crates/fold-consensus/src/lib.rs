//! Ordering for the Fold neo-blockchain.
//!
//! The architecture orders only what conflicts:
//!
//! - [`lanes`]: owned objects finalize with one lane certificate (approvals
//!   from more than two thirds of the stake, signed per batch root), with no
//!   consensus at all.
//! - [`core`]: contended objects go through a certified-DAG BFT protocol in
//!   the Bullshark family, where every validator proposes every round and
//!   anchors commit with more than one third of next-round support.
//! - [`committee`]: stake thresholds, committee sampling from a beacon, and
//!   the capture probability of a sampled committee.
//! - [`beacon`]: hash-chain commit-reveal randomness.
//! - [`forced`]: forced-inclusion deadlines that freeze a censoring shard.
//! - [`sim`]: a deterministic simulator with Byzantine behaviors, used to
//!   test safety and liveness.
//!
//! Signatures are abstracted by [`auth::Authenticator`] and
//! [`auth::SignatureVerifier`]; the simulator's MAC is insecure by design and
//! labeled as such. Nodes plug in `fold-crypto`'s dual post-quantum
//! signatures.

pub mod auth;
pub mod beacon;
pub mod committee;
pub mod core;
pub mod dag;
pub mod error;
pub mod forced;
pub mod lanes;
pub mod sim;

pub use auth::{
    insecure_sim_keyring, signing_digest, Authenticator, InsecureSimAuthenticator,
    InsecureSimVerifier, SignatureVerifier,
};
pub use beacon::{Beacon, BeaconOutput, HashChain};
pub use committee::{capture_probability, sample_committee, Committee};
pub use core::{
    item_hash, Action, CommittedSubDag, Core, CoreConfig, Event, LeaderSchedule, TimerKind,
};
pub use dag::{Certificate, DagEvidence, Header, Message, SignedHeader, Vote};
pub use error::ConsensusError;
pub use forced::{ForcedInclusionTracker, OverdueIntent, PostedIntent, Resolution};
pub use lanes::{
    approval_leaf, find_conflicting_approvals, verify_lane_certificate, ApprovalProof,
    ApprovalStatus, BatchHeader, CertificateBuilder, LaneCertificate, LaneRejection, LaneRequest,
    LaneValidator, ObjectRef, OwnerAuthorization, OwnerEquivocation, SignedBatch,
    ValidatorEquivocation,
};
pub use sim::{Behavior, CommitRecord, SimConfig, SimReport, Simulation};
