//! The data-availability substrate of the Fold architecture.
//!
//! A validity proof shows that a transition was correct; it cannot show that
//! the transition's data was published. This crate makes withholding
//! detectable by everyone:
//!
//! 1. Each Pulse's data ("blob") is laid out as a `k × k` square of
//!    Goldilocks field symbols ([`layout`]).
//! 2. The square is extended with a two-dimensional Reed–Solomon code to
//!    `2k × 2k` ([`rs`], [`square`]), so any half of a row or column rebuilds
//!    it and making the data unrecoverable requires withholding at least
//!    `(k + 1)²` shares — over a quarter of the square.
//! 3. Every row and column is committed with a SHA3 Merkle root, and a data
//!    root commits to all of them ([`DaHeader`]). The commitments are
//!    hash-based: no trusted setup, no pairings, nothing a quantum computer
//!    breaks.
//! 4. Light clients sample random shares and verify them against the header
//!    ([`sampling`]); one client is fooled with probability below `(3/4)^s`.
//! 5. Anyone holding enough shares rebuilds the square ([`reconstruct`]).
//! 6. Hash commitments do not prove correct encoding: in the Fold the World
//!    Proof does, and compact fraud proofs are the fallback ([`fraud`]).
//!
//! # Example
//!
//! ```
//! use fold_da::{ExtendedSquare, LightSampler, Availability};
//!
//! let square = ExtendedSquare::encode(b"one Pulse of data").unwrap();
//! let header = square.header();
//! let report = LightSampler::new(16, 42)
//!     .run(&header, |row, column| square.share(row, column).ok())
//!     .unwrap();
//! assert_eq!(report.outcome, Availability::Available);
//! ```

pub mod error;
pub mod field;
pub mod fraud;
pub mod layout;
pub mod reconstruct;
pub mod rs;
pub mod sampling;
pub mod square;

pub use error::{Axis, DaError};
pub use field::Fp;
pub use fraud::{verify_bad_encoding_proof, verify_extended_square, AxisShare, BadEncodingProof};
pub use layout::{choose_k, MAX_K, MIN_K, SYMBOL_BYTES};
pub use reconstruct::{complete, reconstruct, reconstruct_blob};
pub use sampling::{
    crowd_undetected_probability, detection_probability, min_withholding, sample_positions,
    undetected_bound, undetected_probability, Availability, LightSampler, SampleReport,
};
pub use square::{share_leaf, verify_share, DaHeader, ExtendedSquare, Share};
