//! Structured errors for core primitives.

use thiserror::Error;

/// Errors produced by core primitives.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum CoreError {
    /// A byte string could not be decoded into the expected type.
    #[error("decode error: {0}")]
    Decode(String),
    /// A hex string was malformed or had the wrong length.
    #[error("invalid hex: {0}")]
    InvalidHex(String),
    /// Arithmetic overflowed or underflowed.
    #[error("arithmetic overflow: {0}")]
    Overflow(&'static str),
    /// A Merkle proof did not verify.
    #[error("invalid proof: {0}")]
    InvalidProof(&'static str),
    /// An index was outside the valid range.
    #[error("index {index} out of range for size {size}")]
    OutOfRange {
        /// The requested index.
        index: u64,
        /// The size of the structure.
        size: u64,
    },
}
