//! Structured errors for memory logs, indexes, proofs and keys.

use sion_core::{AccountId, CoreError};
use sion_crypto::CryptoError;
use thiserror::Error;

use crate::atom::{AtomError, EpochKeyId, MemoryAtomId};

/// Errors produced by this crate.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum MemoryError {
    /// An atom failed validation.
    #[error("invalid atom: {0}")]
    InvalidAtom(#[from] AtomError),
    /// An atom was offered to a log that belongs to another writer.
    #[error("atom written by {actual}, but the log belongs to {expected}")]
    WrongWriter {
        /// The log's writer.
        expected: AccountId,
        /// The atom's writer.
        actual: AccountId,
    },
    /// An atom's sequence number is not the next position in its log.
    #[error("atom has sequence number {actual}, expected {expected}")]
    WrongSeq {
        /// The next position in the log.
        expected: u64,
        /// The atom's sequence number.
        actual: u64,
    },
    /// A position or size beyond the end of a log.
    #[error("index {index} out of range for size {size}")]
    OutOfRange {
        /// The requested position or size.
        index: u64,
        /// The number of atoms available.
        size: u64,
    },
    /// A query is malformed (empty time range, inverted box, too many cells).
    #[error("invalid query: {0}")]
    InvalidQuery(&'static str),
    /// A proof did not verify.
    #[error("invalid proof: {0}")]
    InvalidProof(&'static str),
    /// A sequence of ledger commits is not ordered.
    #[error("invalid commit history: {0}")]
    InvalidCommits(&'static str),
    /// A belief policy or a set of reviser logs is malformed.
    #[error("invalid belief input: {0}")]
    InvalidBelief(&'static str),
    /// An atom that a request names is not in the log.
    #[error("unknown atom {0}")]
    UnknownAtom(MemoryAtomId),
    /// No epoch key with this id is held.
    #[error("unknown epoch key {0}")]
    UnknownKey(EpochKeyId),
    /// The epoch key was destroyed (FORGET); what it sealed is unreadable.
    #[error("epoch key {0} was destroyed")]
    KeyDestroyed(EpochKeyId),
    /// The atom's payload is not sealed.
    #[error("payload is not sealed")]
    NotSealed,
    /// Authenticated decryption failed: wrong key, wrong atom or tampering.
    #[error("decryption failed")]
    Decryption,
    /// A core primitive failed.
    #[error("core: {0}")]
    Core(#[from] CoreError),
    /// A cryptographic primitive failed.
    #[error("crypto: {0}")]
    Crypto(#[from] CryptoError),
    /// The on-disk store is malformed or of an unknown version.
    #[error("store: {0}")]
    Store(String),
    /// An operating-system input or output error.
    #[error("i/o: {0}")]
    Io(String),
}

impl From<std::io::Error> for MemoryError {
    fn from(error: std::io::Error) -> Self {
        MemoryError::Io(error.to_string())
    }
}

/// Results with [`MemoryError`].
pub type Result<T, E = MemoryError> = std::result::Result<T, E>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_render_and_convert() {
        let io: MemoryError = std::io::Error::other("disk").into();
        assert_eq!(io, MemoryError::Io("disk".into()));
        let core: MemoryError = CoreError::InvalidProof("x").into();
        assert!(core.to_string().contains("invalid proof"));
        let atom: MemoryError = AtomError::NoTargets.into();
        assert!(atom.to_string().starts_with("invalid atom"));
    }
}
