//! Structured errors for the ordering layer.

use fold_core::{BlockDigest, TxDigest, ValidatorId};
use thiserror::Error;

use crate::lanes::ObjectRef;

/// Errors produced by committees, lanes, the DAG protocol and the beacon.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ConsensusError {
    /// A committee was built with no members.
    #[error("committee has no members")]
    EmptyCommittee,
    /// A committee listed the same validator twice.
    #[error("validator {0:?} listed twice")]
    DuplicateMember(ValidatorId),
    /// A committee member had zero stake.
    #[error("validator {0:?} has zero stake")]
    ZeroStake(ValidatorId),
    /// Total stake overflowed.
    #[error("total stake overflows u64")]
    StakeOverflow,
    /// A message came from a validator outside the committee.
    #[error("validator {0:?} is not a committee member")]
    UnknownValidator(ValidatorId),
    /// A signature did not verify.
    #[error("invalid signature from {0:?}")]
    InvalidSignature(ValidatorId),
    /// A set of signers did not reach the required stake.
    #[error("insufficient stake: have {have}, need {need}")]
    InsufficientStake {
        /// Stake collected.
        have: u64,
        /// Stake required.
        need: u64,
    },
    /// The same validator appeared twice in one certificate.
    #[error("validator {0:?} counted twice")]
    DuplicateSigner(ValidatorId),
    /// An owned object version is already locked to another transaction.
    #[error("object {input:?} is locked to transaction {existing:?}")]
    LockConflict {
        /// The contested object version.
        input: ObjectRef,
        /// The transaction it is locked to.
        existing: TxDigest,
    },
    /// A transaction listed no inputs.
    #[error("transaction has no owned inputs")]
    NoInputs,
    /// A Merkle inclusion proof did not verify.
    #[error("invalid inclusion proof: {0}")]
    InvalidProof(String),
    /// Two values that must agree did not.
    #[error("mismatch: {0}")]
    Mismatch(&'static str),
    /// A certificate's parents were not all known.
    #[error("missing parent certificate {0:?}")]
    MissingParent(BlockDigest),
    /// A header or certificate was malformed.
    #[error("malformed: {0}")]
    Malformed(&'static str),
    /// A beacon reveal did not hash to the registered commitment.
    #[error("invalid beacon reveal from {0:?}")]
    InvalidReveal(ValidatorId),
}
