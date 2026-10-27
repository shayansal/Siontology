//! CLI errors.

use thiserror::Error;

/// Why a command failed.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum CliError {
    /// The node could not be reached.
    #[error("cannot reach the node: {0}")]
    Connection(String),
    /// The node answered with an error.
    #[error("{0}")]
    Node(String),
    /// The node's answer was not understood.
    #[error("unexpected answer: {0}")]
    Protocol(String),
    /// A transaction could not be built.
    #[error("{0}")]
    Build(String),
    /// A transaction did not execute in time, or was rejected.
    #[error("{0}")]
    Transaction(String),
    /// Bad input.
    #[error("{0}")]
    Usage(String),
    /// A verification failed.
    #[error("verification failed: {0}")]
    Verification(String),
}

impl From<fold_exec::ExecError> for CliError {
    fn from(error: fold_exec::ExecError) -> Self {
        CliError::Build(error.to_string())
    }
}
