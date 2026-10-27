//! Pulse headers and the committee certificates that finalize them.
//!
//! After executing a Pulse, every validator signs the Pulse header: the
//! world commitments before and after, the executed order, the receipts
//! root and the data-availability root. A header signed by validators
//! holding more than two thirds of the stake is final. A light client that
//! knows the committee checks the certificate, then checks everything else
//! (objects, claims, receipts, data shares) against the header with proofs.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use borsh::{BorshDeserialize, BorshSerialize};
use fold_consensus::{signing_digest, Committee, SignatureVerifier};
use fold_core::encode::{decode, encode};
use fold_core::hash::{hash_domain, Hash32};
use fold_core::ids::{Pulse, ValidatorId};
use fold_crypto::{verify, DualSignature, PublicKey};
use fold_exec::WorldCommitment;
use serde::{Deserialize, Serialize};

use crate::error::LightError;

/// The signing domain validators use (shared with the node).
pub const VALIDATOR_DOMAIN: &str = "fold/validator/v1";

/// What a Pulse committed to.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct PulseHeader {
    /// The chain identity ([`crate::ChainSpec::id`]): genesis, validators,
    /// keys and shard layout.
    pub chain: Hash32,
    /// The consensus epoch.
    pub epoch: u64,
    /// The Pulse.
    pub pulse: Pulse,
    /// The Pulse's beacon value.
    pub beacon: Hash32,
    /// The world commitment before.
    pub before: WorldCommitment,
    /// The world commitment after.
    pub after: WorldCommitment,
    /// Digest of the executed order.
    pub order_digest: Hash32,
    /// Merkle root over the transaction effects.
    pub receipts_root: Hash32,
    /// Data-availability root of the Pulse's published data.
    pub data_root: Hash32,
    /// Width `k` of the data square (before extension).
    pub data_k: u32,
}

impl PulseHeader {
    /// The header's digest.
    pub fn digest(&self) -> Hash32 {
        hash_domain("fold/light/pulse-header", &encode(self))
    }

    /// The digest validators sign.
    pub fn signing_digest(&self) -> Hash32 {
        signing_digest("fold/light/pulse-header-signature", &[&self.digest().0])
    }
}

/// A header with signatures from the committee.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct PulseCertificate {
    /// The header.
    pub header: PulseHeader,
    /// Validator signatures over [`PulseHeader::signing_digest`].
    pub signatures: Vec<(ValidatorId, Vec<u8>)>,
}

impl PulseCertificate {
    /// The signers.
    pub fn signers(&self) -> BTreeSet<ValidatorId> {
        self.signatures.iter().map(|(v, _)| *v).collect()
    }
}

/// Checks a certificate against the committee; returns the signing stake.
pub fn verify_certificate<V: SignatureVerifier>(
    certificate: &PulseCertificate,
    committee: &Committee,
    verifier: &V,
) -> Result<u64, LightError> {
    let message = certificate.header.signing_digest();
    let mut seen = BTreeSet::new();
    let mut stake = 0u64;
    for (validator, signature) in &certificate.signatures {
        if !seen.insert(*validator) {
            return Err(LightError::Certificate(format!(
                "{} signed twice",
                validator.short()
            )));
        }
        if !committee.contains(validator) {
            return Err(LightError::Certificate(format!(
                "{} is not in the committee",
                validator.short()
            )));
        }
        if !verifier.verify(validator, &message.0, signature) {
            return Err(LightError::Certificate(format!(
                "bad signature from {}",
                validator.short()
            )));
        }
        stake += committee.stake_of(validator);
    }
    if !committee.is_quorum(stake) {
        return Err(LightError::Certificate(format!(
            "signing stake {stake} below quorum {}",
            committee.quorum_threshold()
        )));
    }
    Ok(stake)
}

/// The validators' public keys: verifies their dual post-quantum signatures
/// (ML-DSA-44 and SLH-DSA-SHA2-128f over the same message, both required).
#[derive(
    Clone, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize,
)]
pub struct ValidatorSet {
    /// Each validator's lattice and hash-based public keys.
    pub keys: BTreeMap<ValidatorId, (PublicKey, PublicKey)>,
}

impl fmt::Debug for ValidatorSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ValidatorSet({} validators)", self.keys.len())
    }
}

/// The identifier of a validator with these public keys.
pub fn validator_id(lattice: &PublicKey, hash: &PublicKey) -> ValidatorId {
    ValidatorId::derive_parts(&[&lattice.bytes, &hash.bytes])
}

impl ValidatorSet {
    /// Adds a validator; its identifier is derived from its keys.
    pub fn insert(&mut self, lattice: PublicKey, hash: PublicKey) -> ValidatorId {
        let id = validator_id(&lattice, &hash);
        self.keys.insert(id, (lattice, hash));
        id
    }
}

impl SignatureVerifier for ValidatorSet {
    fn verify(&self, validator: &ValidatorId, message: &[u8], signature: &[u8]) -> bool {
        let Some((lattice, hash)) = self.keys.get(validator) else {
            return false;
        };
        let Ok(signature) = decode::<DualSignature>(signature) else {
            return false;
        };
        verify(lattice, VALIDATOR_DOMAIN, message, &signature.lattice).is_ok()
            && verify(hash, VALIDATOR_DOMAIN, message, &signature.hash).is_ok()
    }
}

/// Checks that a sequence of certified headers is contiguous: each starts
/// where the previous ended.
pub fn verify_header_chain(headers: &[PulseHeader]) -> Result<(), LightError> {
    for pair in headers.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        if b.pulse != a.pulse.next() || b.before != a.after || b.chain != a.chain {
            return Err(LightError::Chain(format!(
                "Pulse {} does not follow Pulse {}",
                b.pulse, a.pulse
            )));
        }
    }
    Ok(())
}
