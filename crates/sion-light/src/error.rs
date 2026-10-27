//! Light-client errors.

use thiserror::Error;

/// Why a light-client check failed.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum LightError {
    /// A Pulse certificate does not verify.
    #[error("certificate: {0}")]
    Certificate(String),
    /// Headers do not chain.
    #[error("header chain: {0}")]
    Chain(String),
    /// A state, knowledge or receipt proof does not verify.
    #[error("proof: {0}")]
    Proof(String),
    /// Data availability sampling failed.
    #[error("availability: {0}")]
    Availability(String),
    /// A World Proof does not verify.
    #[error("world proof: {0}")]
    World(String),
}
