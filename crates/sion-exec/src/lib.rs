//! The Siontology state machine.
//!
//! State is two-sorted. **Linear resources** (coins, allotments, positions)
//! are objects with owners and versions: each version is consumed at most
//! once. **Knowledge** (claims) is appended to a grow-only log and never
//! conflicts. Every operation's coordination class is derived from what it
//! touches, never declared:
//!
//! | class | what it touches | coordination |
//! |---|---|---|
//! | knowledge | only the knowledge log | none (commutative, applied at Pulse end) |
//! | owned linear | objects owned by the sender | one lane certificate |
//! | bounded | a single-writer allotment of a shared total | one lane certificate |
//! | contended | a shared object | consensus in that object's domain |
//!
//! A Pulse executes owned transactions, then contended ones in consensus
//! order, then the commutative deferred effects, then whatever the schedule
//! says is due ([`pulse`]). Execution doubles as witness generation: every
//! chunk of a Pulse can be re-executed statelessly from its witness
//! ([`stateless`]), which is what both proof stacks of the Fold check.
//!
//! Modules:
//!
//! - [`object`], [`account`], [`claim`], [`charter`], [`escrow`], [`market`],
//!   [`stream`], [`assurance`]: the sorts of state.
//! - [`validators`]: stake, epochs, committees, equivocation evidence and
//!   forced-inclusion obligations.
//! - [`tx`]: transactions and their derived coordination classes.
//! - [`store`]: object stores and the recording, journaled overlay.
//! - [`execute`]: one transaction (prepare, apply, fail-with-fee).
//! - [`pulse`]: one Pulse, canonical order, deferred phase, chunk traces.
//! - [`stateless`]: re-executing a chunk from its witness.
//! - [`state`]: the full state, configuration and world commitment.
//! - [`genesis`]: the opening state.
//! - [`wallet`]: building and signing transactions.
//! - [`sim`]: a single-process chain for tests and demos (simulation only).

pub mod account;
pub mod assurance;
pub mod charter;
pub mod claim;
pub mod error;
pub mod escrow;
pub mod execute;
pub mod genesis;
pub mod market;
pub mod object;
pub mod pulse;
pub mod sim;
pub mod state;
pub mod stateless;
pub mod store;
pub mod stream;
pub mod tx;
pub mod validators;
pub mod wallet;

pub use error::ExecError;
pub use execute::{execute_transaction, Deferred, Event, ExecStatus, PulseContext, TxEffects};
pub use genesis::{genesis, GenesisAccount, GenesisSpec};
pub use object::{Object, ObjectData, ObjectRef, Owner};
pub use pulse::{
    execute_pulse, execute_pulse_traced, ChunkStatement, ChunkTrace, PulseInput, PulseOutput,
    Witness,
};
pub use state::{ChainConfig, State, WorldCommitment};
pub use stateless::{replay_chunk, ReplayReport};
pub use tx::{SignedTransaction, Transaction, TxKind};
pub use wallet::{StateView, Wallet};
