//! The Fold: validity proofs for every Pulse.
//!
//! Execution of a Pulse is cut into chunks (see `fold_exec::pulse`); each
//! chunk carries a statement — "these steps take the world from commitment
//! `before` to commitment `after`, extending these accumulators" — and a
//! witness. The architecture proves each statement twice, with two proof
//! stacks built independently, and folds the leaf proofs into one World
//! Proof per Pulse. Anyone checking a World Proof checks that the leaves
//! chain from the Pulse's start to its end and that **every** stack's proof
//! of every leaf verifies, so a false statement would need a soundness bug
//! in both stacks at once: the trust formula is
//! `spec ∧ sha3 ∧ ((stack_A ∧ circuit_A) ∨ (stack_B ∧ circuit_B))`.
//!
//! # What this implementation proves
//!
//! **Simulation of succinct proofs.** The stacks here are *replay* stacks:
//! a leaf proof is the witness itself, and verifying it means re-executing
//! the chunk statelessly from that witness (`fold_exec::replay_chunk`) and
//! comparing every root and accumulator. This is sound — a replay proof of
//! a false statement does not verify — but not succinct: it is as large as
//! the witness and as slow to check as execution. Replacing the replay
//! stacks with STARK provers changes the size and cost of a leaf proof, not
//! the statements, the chaining or the folding. The leaf-sampling verifier
//! ([`verify_world_proof_sampled`]) gives light clients a sublinear check in
//! the meantime: re-execute a random subset of leaves, detecting an invalid
//! leaf with probability `samples / leaves`.
//!
//! - [`stack`]: the [`ProofStack`] interface and leaf proofs.
//! - [`replay`]: stack A, stateless re-execution with the reference executor.
//! - [`world`]: aggregation into World Proofs and their verification.

pub mod error;
pub mod replay;
pub mod stack;
pub mod world;

pub use error::ProveError;
pub use replay::ReplayStack;
pub use stack::{LeafProof, ProofStack};
pub use world::{
    prove_pulse, verify_world_proof, verify_world_proof_sampled, WorldProof, WorldReport,
};
