//! Structured execution errors.

use sion_assurance::AssuranceError;
use sion_clearing::ClearingError;
use sion_core::ids::{ObjectId, Version};
use sion_core::CoreError;
use sion_crypto::CryptoError;
use thiserror::Error;

/// Why a transaction or Pulse could not be executed.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ExecError {
    /// An object is missing (or, for a witness-backed store, not in the witness).
    #[error("missing object {0}")]
    MissingObject(ObjectId),
    /// An input's version does not match the object's version.
    #[error("object {id} is at version {actual}, transaction expects {expected}")]
    VersionMismatch {
        /// Object.
        id: ObjectId,
        /// Expected by the transaction.
        expected: Version,
        /// Actual.
        actual: Version,
    },
    /// The sender does not own an input.
    #[error("sender does not own object {0}")]
    NotOwner(ObjectId),
    /// Authorization failed.
    #[error("unauthorized: {0}")]
    Unauthorized(String),
    /// Not enough funds.
    #[error("insufficient funds: need {need}, have {have}")]
    InsufficientFunds {
        /// Required.
        need: String,
        /// Available.
        have: String,
    },
    /// A malformed or invalid transaction.
    #[error("invalid transaction: {0}")]
    Invalid(String),
    /// A charter caveat would be violated.
    #[error("charter caveat violated: {0}")]
    Caveat(String),
    /// The transaction expired.
    #[error("transaction expired")]
    Expired,
    /// Assurance Algebra error.
    #[error("assurance: {0}")]
    Assurance(#[from] AssuranceError),
    /// Clearing error.
    #[error("clearing: {0}")]
    Clearing(#[from] ClearingError),
    /// Cryptography error.
    #[error("crypto: {0}")]
    Crypto(#[from] CryptoError),
    /// Core error.
    #[error("core: {0}")]
    Core(#[from] CoreError),
}
