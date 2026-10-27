//! The Fold light client.
//!
//! A phone-sized client follows the chain without executing it:
//!
//! - [`chain`]: the chain identity a client pins, and the committees it
//!   fixes for every epoch.
//! - [`header`]: Pulse headers and committee certificates (more than two
//!   thirds of the stake, dual post-quantum signatures), and header chains.
//! - [`proofs`]: objects, absences, claims and receipts proven against a
//!   certified header.
//! - [`availability`]: sampling the Pulse's data square so withheld data is
//!   detected with overwhelming probability.

pub mod availability;
pub mod chain;
pub mod error;
pub mod header;
pub mod proofs;

pub use chain::ChainSpec;
pub use error::LightError;
pub use header::{
    verify_certificate, verify_header_chain, PulseCertificate, PulseHeader, ValidatorSet,
};
pub use proofs::{
    verify_claim, verify_object, verify_receipt, ClaimProof, ObjectProof, ReceiptProof,
};
