//! Structured cryptographic errors.

use thiserror::Error;

use crate::scheme::Scheme;

/// Errors from signing, verification, policies and encryption.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum CryptoError {
    /// A key or signature had the wrong length for its scheme.
    #[error("{scheme:?}: expected {expected} bytes, got {actual}")]
    BadLength {
        /// The scheme.
        scheme: Scheme,
        /// Expected length.
        expected: usize,
        /// Actual length.
        actual: usize,
    },
    /// The signature's scheme does not match the key's scheme.
    #[error("scheme mismatch: key is {key:?}, signature is {signature:?}")]
    SchemeMismatch {
        /// The key's scheme.
        key: Scheme,
        /// The signature's scheme.
        signature: Scheme,
    },
    /// A signature did not verify.
    #[error("invalid signature")]
    InvalidSignature,
    /// The underlying library reported an error.
    #[error("library error: {0}")]
    Library(&'static str),
    /// A domain tag longer than the 255-byte FIPS context limit.
    #[error("domain tag too long")]
    DomainTooLong,
    /// An authorization policy is malformed.
    #[error("invalid policy: {0}")]
    InvalidPolicy(&'static str),
    /// An authorization does not satisfy its policy.
    #[error("unauthorized: {0}")]
    Unauthorized(String),
    /// Decryption failed: wrong key, tampered ciphertext or wrong context.
    #[error("decryption failed")]
    Decryption,
    /// Not enough key shares to reconstruct a secret.
    #[error("need {needed} shares, have {have}")]
    NotEnoughShares {
        /// Threshold.
        needed: usize,
        /// Shares supplied.
        have: usize,
    },
    /// Shares were malformed (duplicate or zero indices, mismatched lengths).
    #[error("invalid shares: {0}")]
    InvalidShares(&'static str),
}
