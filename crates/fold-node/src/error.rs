//! Node errors.

use thiserror::Error;

/// Why the node could not start or serve.
#[derive(Debug, Error)]
pub enum NodeError {
    /// Bad configuration.
    #[error("configuration: {0}")]
    Config(String),
    /// I/O.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// Storage.
    #[error("storage: {0}")]
    Storage(#[from] crate::storage::StorageError),
    /// Execution.
    #[error("execution: {0}")]
    Exec(#[from] fold_exec::ExecError),
    /// Consensus.
    #[error("consensus: {0}")]
    Consensus(#[from] fold_consensus::ConsensusError),
}
