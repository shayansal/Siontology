//! Core primitives for the Fold neo-blockchain.
//!
//! Everything here is deterministic and dependency-light, because every other
//! crate (and every light client) builds on it:
//!
//! - [`hash`]: domain-separated SHA3-256 hashing and the [`Hash32`] digest type.
//! - [`encode`]: canonical binary encoding (Borsh) used for every hashed value.
//! - [`ids`]: newtype identifiers so that an account can never be confused with a claim.
//! - [`amount`]: fixed-point money and the [`Cost`] type with an explicit infinity.
//! - [`merkle`]: RFC 9162-style Merkle tree hash with inclusion and consistency proofs,
//!   which back the append-only knowledge log ("the knowledge root only grew").
//! - [`smt`]: a sparse Merkle tree for the state root, with inclusion and
//!   non-inclusion proofs a phone can check.
//! - [`rng`]: seeded deterministic randomness for simulations and tests.

pub mod amount;
pub mod encode;
pub mod error;
pub mod hash;
pub mod ids;
pub mod merkle;
pub mod rng;
pub mod smt;

pub use amount::{Amount, Cost};
pub use encode::{decode, encode, Canonical};
pub use error::CoreError;
pub use hash::{hash_domain, hash_parts, Hash32, Hasher};
pub use ids::{
    AccountId, AssetId, AtomId, BlockDigest, ClaimId, DomainId, Epoch, ObjectId, Pulse, Round,
    ShardId, TimestampMs, TxDigest, ValidatorId, Version,
};
