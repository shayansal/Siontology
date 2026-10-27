//! Prover errors.

use thiserror::Error;

/// Why proving or verification failed.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ProveError {
    /// The trace or statement is malformed.
    #[error("malformed: {0}")]
    Malformed(String),
    /// A leaf proof does not verify.
    #[error("leaf {index} ({stack}): {reason}")]
    Leaf {
        /// Leaf index.
        index: u32,
        /// Proof stack.
        stack: String,
        /// Why.
        reason: String,
    },
    /// Leaves do not chain into a valid Pulse.
    #[error("chain: {0}")]
    Chain(String),
    /// A required stack is missing from the proof.
    #[error("missing proofs from stack {0}")]
    MissingStack(String),
}
