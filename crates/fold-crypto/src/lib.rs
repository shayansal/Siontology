//! Post-quantum identity for Fold.
//!
//! The architecture requires that every authorization carries two
//! signatures from unrelated mathematical families, so a forger must break
//! both (trust formula: `lattice ∨ hash`), and that keys rotate without
//! changing the account identifier. This crate provides:
//!
//! - [`keys`]: ML-DSA-44 (FIPS 204, module lattices) and SLH-DSA-SHA2-128s /
//!   128f (FIPS 205, stateless hash-based) keypairs and signatures. The
//!   signing domain tag is passed as the FIPS context string, so a signature
//!   made for one purpose never verifies for another.
//! - [`dual`]: a convenience keypair holding one key of each family.
//! - [`policy`]: account authorization policies. An account's ID is the hash
//!   of its genesis policy and never changes; the policy itself can rotate.
//! - [`kem`]: ML-KEM-768 (FIPS 203) key encapsulation and single-recipient
//!   sealed envelopes (ML-KEM + ChaCha20-Poly1305), used for viewing keys and
//!   Travel Rule payloads.
//! - [`shamir`] and [`committee`]: sealing to a committee with t-of-n Shamir
//!   sharing of the payload key, used for sealed intent batches. Decryption
//!   needs `t` members, so the committee threshold is itself an atom in any
//!   trust formula that relies on the sealing.

pub mod committee;
pub mod dual;
pub mod error;
pub mod kem;
pub mod keys;
pub mod policy;
pub mod scheme;
pub mod shamir;

pub use committee::{CommitteeEnvelope, KeyShare};
pub use dual::{DualKeypair, DualSignature};
pub use error::CryptoError;
pub use kem::{KemKeypair, KemPublicKey, SealedEnvelope};
pub use keys::{verify, Keypair, PublicKey, Signature};
pub use policy::{AuthPolicy, Authorization, KeySignature};
pub use scheme::{Family, Scheme};
