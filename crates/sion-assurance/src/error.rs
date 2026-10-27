//! Structured errors for the Assurance Algebra.

use sion_core::ids::{AtomId, ClaimId};
use sion_core::Amount;
use thiserror::Error;

/// Errors from the Assurance Algebra.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum AssuranceError {
    /// The atom is not registered.
    #[error("unknown atom {0}")]
    UnknownAtom(AtomId),
    /// An atom was re-registered with different properties.
    #[error("atom {0} already registered with different properties")]
    ConflictingAtom(AtomId),
    /// The claim is not registered.
    #[error("unknown claim {0}")]
    UnknownClaim(ClaimId),
    /// The claim already exists.
    #[error("claim {0} already exists")]
    DuplicateClaim(ClaimId),
    /// A claim references a claim that is not active.
    #[error("claim {0} is not active")]
    InactiveClaim(ClaimId),
    /// A malformed claim or parameter.
    #[error("invalid: {0}")]
    Invalid(String),
    /// The atom has no bond.
    #[error("atom {0} is not bonded")]
    NotBonded(AtomId),
    /// Not enough unreserved backing for a reservation.
    #[error("insufficient backing: requested {requested}, reservable {available}")]
    InsufficientBacking {
        /// Amount requested (κ·V).
        requested: Amount,
        /// Maximum reservable.
        available: String,
    },
    /// The formula is too large to process.
    #[error("formula too large: {0} nodes after unfolding")]
    TooLarge(usize),
    /// A challenge-related error.
    #[error("challenge: {0}")]
    Challenge(String),
    /// Arithmetic overflow.
    #[error("overflow")]
    Overflow,
}
