//! The Fold devnet node.
//!
//! A devnet runs every validator of a committee in one process, each on its
//! own thread with its own state replica, connected by an in-memory
//! network. Everything else is the real protocol:
//!
//! - **Lanes** ([`fold_consensus::LaneValidator`]): owned-path transactions
//!   are certified by their shard's committee, approvals batch-signed with
//!   dual post-quantum signatures ([`keys`]).
//! - **Consensus** ([`fold_consensus::Core`]): a certified-DAG BFT protocol
//!   orders contended transactions, lane certificates and beacon reveals.
//! - **Pulses** ([`pulses`]): the commit sequence is cut into Pulses on
//!   round boundaries; every validator executes each Pulse
//!   ([`fold_exec::execute_pulse`]) and signs the resulting header; headers
//!   signed by more than two thirds are final ([`fold_light::PulseCertificate`]).
//! - **Data availability**: each Pulse's transaction bodies are erasure-coded
//!   into a 2D Reed–Solomon square whose root the header commits to.
//! - **The Fold** ([`prover`]): validator 0 traces execution and a prover
//!   thread folds every Pulse into a World Proof.
//! - **Storage** ([`storage`]): a segmented, checksummed write-ahead log of
//!   executed Pulses and periodic snapshots; restarts replay and verify.
//! - **API** ([`api`]): JSON over HTTP for wallets, the CLI and light clients.

/// Stack size of every node thread. Decoding and checking nested structures
/// (formulas, claim references) recurses; protocol limits keep that depth
/// small, and a generous stack is a second line of defence.
pub const THREAD_STACK_BYTES: usize = 64 * 1024 * 1024;

pub mod api;
pub mod config;
pub mod devnet;
pub mod error;
pub mod forced;
pub mod gateway;
pub mod keys;
pub mod owners;
pub mod payload;
pub mod prover;
pub mod pulses;
pub mod storage;
pub mod validator;
pub mod view;

pub use config::DevnetConfig;
pub use devnet::Devnet;
pub use error::NodeError;
pub use keys::SignatureMode;
pub use view::{Query, TxStatus, TxView};
