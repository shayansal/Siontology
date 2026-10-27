//! Structured errors for the data-availability layer.

use std::fmt;

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Which axis of the extended square a statement is about.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    BorshSerialize,
    BorshDeserialize,
    Serialize,
    Deserialize,
)]
pub enum Axis {
    /// A row (fixed row index, varying column).
    Row,
    /// A column (fixed column index, varying row).
    Column,
}

impl fmt::Display for Axis {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Axis::Row => f.write_str("row"),
            Axis::Column => f.write_str("column"),
        }
    }
}

/// Errors produced by the data-availability layer.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum DaError {
    /// The square size is not a power of two in the supported range.
    #[error("invalid square size k = {0}: must be a power of two in [2, 2048]")]
    InvalidK(usize),
    /// The blob does not fit into a `k × k` data square.
    #[error("blob of {len} bytes needs {needed} symbols; k = {k} holds {capacity}")]
    BlobTooLarge {
        /// Blob length in bytes.
        len: usize,
        /// Symbols needed, including the length symbol.
        needed: usize,
        /// The square size.
        k: usize,
        /// Symbols a `k × k` square holds.
        capacity: usize,
    },
    /// A position lies outside the extended square.
    #[error("position ({row}, {column}) outside a {width}-wide square")]
    OutOfRange {
        /// Row index.
        row: usize,
        /// Column index.
        column: usize,
        /// Width of the extended square (`2k`).
        width: usize,
    },
    /// A field element was not in canonical form (`>= p`).
    #[error("non-canonical field element {0:#x}")]
    NonCanonicalField(u64),
    /// A data-quadrant symbol does not fit in seven bytes.
    #[error("data symbol {index} does not fit in seven bytes")]
    NonCanonicalSymbol {
        /// Row-major index in the data quadrant.
        index: usize,
    },
    /// The blob length symbol or the padding is not canonical.
    #[error("invalid blob layout: {0}")]
    InvalidPadding(&'static str),
    /// A header's fields do not agree with each other.
    #[error("inconsistent header: {0}")]
    HeaderInconsistent(&'static str),
    /// A share failed its Merkle proof.
    #[error("share at ({row}, {column}) failed its Merkle proof")]
    InvalidShare {
        /// Row index.
        row: usize,
        /// Column index.
        column: usize,
    },
    /// Iterative decoding stalled with shares still missing.
    #[error("{missing} shares could not be recovered")]
    Unrecoverable {
        /// Number of cells still unknown when decoding stalled.
        missing: usize,
    },
    /// Known shares on one axis do not lie on a single polynomial of degree `< k`.
    #[error("{axis} {index}: known shares are not on one polynomial of degree < k")]
    InconsistentAxis {
        /// The axis.
        axis: Axis,
        /// The axis index.
        index: usize,
    },
    /// An axis of a complete square is not a Reed-Solomon codeword.
    #[error("{axis} {index} is not a Reed-Solomon codeword")]
    BadEncoding {
        /// The axis.
        axis: Axis,
        /// The axis index.
        index: usize,
    },
    /// A reconstructed square does not reproduce the committed roots.
    #[error("reconstructed square does not match the header roots")]
    RootMismatch,
    /// A fraud proof did not establish bad encoding.
    #[error("fraud proof rejected: {0}")]
    InvalidFraudProof(&'static str),
    /// Too few shares to decode an axis.
    #[error("need at least {needed} shares on the axis, have {have}")]
    NotEnoughShares {
        /// Shares required (`k`).
        needed: usize,
        /// Shares available.
        have: usize,
    },
}
