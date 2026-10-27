//! Proof stack B: an independent implementation of the Siontology state
//! transition, written from `docs/spec/state-transition.md` (v1.3) alone.
//!
//! The Fold checks every chunk of every Pulse twice, with two independent
//! implementations of the same specification (N-version programming): the
//! reference executor in `sion-exec` (stack A) and this crate (stack B). They
//! must accept every honest chunk and reject every invalid one; where they
//! disagree, one of them is wrong. This crate shares only the separately
//! trusted primitives with stack A — hashing and Merkle structures
//! (`sion-core`), signatures, envelopes and validator identities
//! (`sion-crypto`), the Assurance Algebra (`sion-assurance`), clearing
//! certificates (`sion-clearing`), committee sampling and the consensus
//! message formats (`sion-consensus`) — and the data layouts and type
//! helpers spec §0 allows. It never calls the reference executor's logic.
//!
//! Modules follow the specification:
//!
//! | module | spec | what |
//! |---|---|---|
//! | [`context`] | §1 | the Pulse context |
//! | `view` | §2.1 | the witnessed pre-state plus kept writes |
//! | `journal` | §3.1 | load, try_load, store, consume |
//! | `tx` | §3 | create, mint, draw, charge_fee_from_gas, finalize_inputs |
//! | `prepare` | §4.1 | preparation (failures reject) |
//! | `outcome` | §4.2 | success, failure with fee, rejection |
//! | `ops` | §5 | one function per transaction kind |
//! | `deferred` | §6.3 | the deferred phase: items, forced intents, due events, epochs |
//! | [`commit`] | §7 | effects leaves, accumulators, steps |
//! | [`chunk`] | §8 | verifying a chunk |
//!
//! The entry point is [`verify_chunk`].

#![warn(missing_docs)]

pub mod chunk;
pub mod commit;
pub mod context;
mod deferred;
pub mod error;
mod journal;
mod ops;
mod outcome;
mod prepare;
mod tx;
mod view;

pub use chunk::{execute_chunk, verify_chunk, ChunkExecution, ChunkReport};
pub use context::SpecContext;
pub use error::{OpError, SpecError};
