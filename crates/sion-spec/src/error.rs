//! Why a chunk is rejected, and how an operation stops.
//!
//! Two kinds of error exist and they must never be confused:
//!
//! - A [`SpecError`] means the *chunk* is invalid (one of the checks of
//!   spec §8 failed, or execution read an object outside the witness). It
//!   always propagates to [`crate::verify_chunk`] and rejects the chunk.
//! - An [`OpError`] is an error of one *operation* (spec §3.4). Where it
//!   occurs decides a transaction's outcome: during prepare it rejects the
//!   transaction, during apply it fails it. Its message is not committed.
//!
//! Execution code returns [`Abort`], which carries either of the two, so `?`
//! propagates both without turning a witness gap into a failed transaction.

use sion_assurance::AssuranceError;
use sion_clearing::ClearingError;
use sion_core::hash::Hash32;
use sion_core::ids::ObjectId;
use sion_core::CoreError;
use sion_crypto::CryptoError;
use thiserror::Error;

/// Why a chunk was rejected: which check of spec §8 failed.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SpecError {
    /// §8.1: the witness configuration's digest differs from the
    /// commitments' configuration digests.
    #[error("configuration digest mismatch (witness {witness}, before {before}, after {after})")]
    ConfigMismatch {
        /// Digest of the witness configuration.
        witness: Hash32,
        /// `S.before.config`.
        before: Hash32,
        /// `S.after.config`.
        after: Hash32,
    },
    /// §8.2: the Pulse numbers of the statement are inconsistent.
    #[error("pulse numbers inconsistent: statement {pulse}, before {before}, after {after}")]
    PulseMismatch {
        /// `S.pulse`.
        pulse: u64,
        /// `S.before.pulse`.
        before: u64,
        /// `S.after.pulse`.
        after: u64,
    },
    /// §8.3: a witness entry holds an object with another identifier.
    #[error("witness entry {id} holds object {found}")]
    WitnessIdMismatch {
        /// The entry's identifier.
        id: ObjectId,
        /// The identifier of the object it holds.
        found: ObjectId,
    },
    /// §8.3: a witness proof does not verify against `S.before.state_root`.
    #[error("witness proof for {id} does not verify: {reason}")]
    WitnessProof {
        /// The entry's identifier.
        id: ObjectId,
        /// What the proof check reported.
        reason: CoreError,
    },
    /// §8.3: an identifier appears twice in the witness.
    #[error("object {0} appears twice in the witness")]
    DuplicateWitness(ObjectId),
    /// §8.7: the witness proofs do not combine into one partial tree.
    #[error("witness proofs are inconsistent: {0}")]
    WitnessTree(CoreError),
    /// §8.4: the knowledge frontier is malformed or does not match
    /// `S.before`.
    #[error("knowledge frontier does not match the statement before the chunk")]
    FrontierMismatch,
    /// §8.5: a transaction chunk carries deferred items.
    #[error("a transaction chunk carries {0} deferred items")]
    UnexpectedDeferredItems(usize),
    /// §8.5: `steps(T) ≠ S.steps` (for the deferred phase, `T` are the
    /// posted forced intents).
    #[error("steps digest mismatch")]
    StepsMismatch,
    /// §8.5: the deferred-effects accumulator differs.
    #[error("deferred accumulator mismatch")]
    DeferredMismatch,
    /// §8.5: the receipts accumulator differs.
    #[error("effects accumulator mismatch")]
    EffectsMismatch,
    /// §2.1 and §8.5: execution read an object that is not in the witness.
    #[error("read of object {0} outside the witness")]
    Unwitnessed(ObjectId),
    /// §6.3: the deferred phase could not run (a protocol object is missing
    /// or malformed, its arithmetic overflowed, or a committee could not be
    /// sampled).
    #[error("deferred phase: {0}")]
    DeferredPhase(String),
    /// §8.6: the knowledge log after the chunk differs from `S.after`.
    #[error(
        "knowledge log mismatch: computed size {computed_size}, statement size {statement_size}"
    )]
    KnowledgeMismatch {
        /// Size of the log the chunk produced.
        computed_size: u64,
        /// `S.after.knowledge_size`.
        statement_size: u64,
    },
    /// §8.7: a write is not covered by the witness proofs.
    #[error("write to {id} is not covered by the witness: {reason}")]
    UncoveredWrite {
        /// The written object.
        id: ObjectId,
        /// What the partial tree reported.
        reason: CoreError,
    },
    /// §8.7: the state root after the chunk differs from `S.after`.
    #[error("state root mismatch: computed {computed}, statement {statement}")]
    StateRootMismatch {
        /// Root the chunk's writes produce.
        computed: Hash32,
        /// `S.after.state_root`.
        statement: Hash32,
    },
}

/// An error of one operation (spec §3.4). The message is for humans only;
/// only the outcome class (success, failed, rejected) is committed.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[error("{0}")]
pub struct OpError(pub String);

/// Why an execution step stopped: an operation error (the transaction's
/// outcome decides what follows) or a chunk-level rejection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Abort {
    /// An error of the operation (§3.4).
    Op(OpError),
    /// The chunk itself is invalid.
    Chunk(SpecError),
}

impl Abort {
    /// An operation error with `message`.
    pub(crate) fn op(message: impl Into<String>) -> Abort {
        Abort::Op(OpError(message.into()))
    }
}

/// Shorthand for an operation error, for `Err(fail("…"))`.
pub(crate) fn fail(message: impl Into<String>) -> Abort {
    Abort::op(message)
}

/// The result of an execution step.
pub(crate) type Step<T = ()> = Result<T, Abort>;

impl From<SpecError> for Abort {
    fn from(error: SpecError) -> Self {
        Abort::Chunk(error)
    }
}

impl From<OpError> for Abort {
    fn from(error: OpError) -> Self {
        Abort::Op(error)
    }
}

impl From<AssuranceError> for Abort {
    fn from(error: AssuranceError) -> Self {
        Abort::op(format!("assurance: {error}"))
    }
}

impl From<ClearingError> for Abort {
    fn from(error: ClearingError) -> Self {
        Abort::op(format!("clearing: {error}"))
    }
}

impl From<CryptoError> for Abort {
    fn from(error: CryptoError) -> Self {
        Abort::op(format!("crypto: {error}"))
    }
}

/// Arithmetic overflow inside an operation is an error of that operation
/// (spec §1).
impl From<CoreError> for Abort {
    fn from(error: CoreError) -> Self {
        Abort::op(format!("arithmetic: {error}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_primitive_errors_are_operation_errors() {
        let abort: Abort = CoreError::Overflow("x").into();
        assert!(matches!(abort, Abort::Op(_)));
        let abort: Abort = AssuranceError::Overflow.into();
        assert!(matches!(abort, Abort::Op(_)));
    }

    #[test]
    fn chunk_errors_stay_chunk_errors() {
        let id = ObjectId::derive(b"x");
        let abort: Abort = SpecError::Unwitnessed(id).into();
        assert_eq!(abort, Abort::Chunk(SpecError::Unwitnessed(id)));
        assert_eq!(fail("boom"), Abort::Op(OpError("boom".into())));
    }
}
