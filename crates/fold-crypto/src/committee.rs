//! Sealing a payload to a committee with a t-of-n threshold.
//!
//! Sealed intent batches need a ciphertext nobody can read until the batch
//! seals, and then anyone can. Threshold encryption that is both
//! post-quantum and efficient for large committees is an open problem, so
//! this implementation uses a simple, sound construction for small
//! committees:
//!
//! 1. Encrypt the payload under a fresh random key `K` (ChaCha20-Poly1305).
//! 2. Split `K` into `n` Shamir shares with threshold `t`.
//! 3. Seal share `i` to member `i`'s ML-KEM key.
//!
//! After the batch seals, members publish their decrypted shares; any `t`
//! reconstruct `K`. Confidentiality before sealing holds if fewer than `t`
//! members collude: the trust formula atom is "fewer than t of this
//! committee collude". Ciphertext size grows linearly with `n`
//! (about 1.1 KB per member).

use borsh::{BorshDeserialize, BorshSerialize};
use fold_core::encode::encode;
use fold_core::hash::{hash_parts, Hash32};
use rand_core::CryptoRngCore;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::error::CryptoError;
use crate::kem::{aead_open, aead_seal, seal, KemKeypair, KemPublicKey, SealedEnvelope};
use crate::shamir::{combine, split, Share};

/// A payload sealed to a committee.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct CommitteeEnvelope {
    /// Shares needed to open.
    pub threshold: u8,
    /// Fingerprints of the member keys, in share order.
    pub members: Vec<Hash32>,
    /// One sealed key share per member.
    pub shares: Vec<SealedEnvelope>,
    /// Payload nonce.
    pub nonce: [u8; 12],
    /// Payload ciphertext under the shared key.
    #[serde(with = "crate::keys::hex_bytes")]
    pub payload: Vec<u8>,
}

/// A decrypted key share published by a member after the batch seals.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct KeyShare {
    /// The member's index in the envelope.
    pub member_index: u8,
    /// The share.
    pub share: Share,
}

impl CommitteeEnvelope {
    /// Commitment to everything except the payload, bound into the payload's
    /// associated data so shares cannot be swapped between envelopes.
    fn header_digest(&self) -> Hash32 {
        hash_parts(
            "fold/crypto/committee-header",
            &[&[self.threshold], &encode(&self.members), &encode(&self.shares), &self.nonce],
        )
    }

    /// Digest of the whole envelope, for ordering and commitments.
    pub fn digest(&self) -> Hash32 {
        hash_parts("fold/crypto/committee-envelope", &[&encode(self)])
    }

    /// Seals `plaintext` to `members` so that any `threshold` can open it.
    pub fn seal(
        members: &[KemPublicKey],
        threshold: u8,
        domain: &str,
        aad: &[u8],
        plaintext: &[u8],
        rng: &mut impl CryptoRngCore,
    ) -> Result<CommitteeEnvelope, CryptoError> {
        let n = u8::try_from(members.len()).map_err(|_| CryptoError::InvalidShares("at most 255 members"))?;
        let mut key = Zeroizing::new([0u8; 32]);
        rng.fill_bytes(key.as_mut());
        let shares = split(key.as_ref(), threshold, n, rng)?;
        let mut sealed = Vec::with_capacity(members.len());
        for (index, (member, share)) in members.iter().zip(&shares).enumerate() {
            let share_aad = share_associated(aad, index as u8);
            sealed.push(seal(member, domain, &share_aad, &encode(share), rng)?);
        }
        let mut nonce = [0u8; 12];
        rng.fill_bytes(&mut nonce);
        let mut envelope = CommitteeEnvelope {
            threshold,
            members: members.iter().map(KemPublicKey::fingerprint).collect(),
            shares: sealed,
            nonce,
            payload: Vec::new(),
        };
        let payload_aad = payload_associated(domain, aad, &envelope.header_digest());
        envelope.payload = aead_seal(&key, &nonce, &payload_aad, plaintext);
        Ok(envelope)
    }

    /// A member decrypts its own share.
    pub fn open_share(
        &self,
        member_index: u8,
        keypair: &KemKeypair,
        domain: &str,
        aad: &[u8],
    ) -> Result<KeyShare, CryptoError> {
        let index = usize::from(member_index);
        let envelope = self.shares.get(index).ok_or(CryptoError::InvalidShares("no such member"))?;
        if self.members.get(index) != Some(&keypair.public().fingerprint()) {
            return Err(CryptoError::InvalidShares("key does not match member"));
        }
        let bytes = keypair.open(domain, &share_associated(aad, member_index), envelope)?;
        let share: Share = fold_core::encode::decode(&bytes).map_err(|_| CryptoError::Decryption)?;
        Ok(KeyShare { member_index, share })
    }

    /// Opens the payload from at least `threshold` published shares.
    pub fn open(&self, shares: &[KeyShare], domain: &str, aad: &[u8]) -> Result<Vec<u8>, CryptoError> {
        let raw: Vec<Share> = shares.iter().map(|s| s.share.clone()).collect();
        let key_bytes = Zeroizing::new(combine(&raw, self.threshold)?);
        let key: [u8; 32] = key_bytes
            .as_slice()
            .try_into()
            .map_err(|_| CryptoError::InvalidShares("wrong key length"))?;
        let key = Zeroizing::new(key);
        let payload_aad = payload_associated(domain, aad, &self.header_digest());
        aead_open(&key, &self.nonce, &payload_aad, &self.payload)
    }

    /// Ciphertext size in bytes.
    pub fn size(&self) -> usize {
        encode(self).len()
    }
}

fn share_associated(aad: &[u8], index: u8) -> Vec<u8> {
    let mut out = aad.to_vec();
    out.extend_from_slice(b"/share/");
    out.push(index);
    out
}

fn payload_associated(domain: &str, aad: &[u8], header: &Hash32) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(domain.as_bytes());
    out.push(0);
    out.extend_from_slice(aad);
    out.extend_from_slice(&header.0);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOMAIN: &str = "fold/test/committee";

    fn committee(n: u8) -> Vec<KemKeypair> {
        (0..n).map(|i| KemKeypair::from_seed(&[i + 10; 32])).collect()
    }

    #[test]
    fn threshold_members_open_the_payload() {
        let members = committee(4);
        let publics: Vec<KemPublicKey> = members.iter().map(|m| m.public().clone()).collect();
        let mut rng = fold_core::rng::seeded(7);
        let intent = b"buy 8 GPU-hours, limit 2.10 USD";
        let envelope = CommitteeEnvelope::seal(&publics, 3, DOMAIN, b"batch-17", intent, &mut rng).unwrap();
        let shares: Vec<KeyShare> = [0u8, 2, 3]
            .iter()
            .map(|&i| envelope.open_share(i, &members[usize::from(i)], DOMAIN, b"batch-17").unwrap())
            .collect();
        assert_eq!(envelope.open(&shares, DOMAIN, b"batch-17").unwrap(), intent);
        // Two shares are not enough.
        assert!(envelope.open(&shares[..2], DOMAIN, b"batch-17").is_err());
    }

    #[test]
    fn members_cannot_open_each_others_shares() {
        let members = committee(4);
        let publics: Vec<KemPublicKey> = members.iter().map(|m| m.public().clone()).collect();
        let mut rng = fold_core::rng::seeded(8);
        let envelope = CommitteeEnvelope::seal(&publics, 2, DOMAIN, b"b", b"x", &mut rng).unwrap();
        assert!(envelope.open_share(0, &members[1], DOMAIN, b"b").is_err());
        assert!(envelope.open_share(0, &members[0], DOMAIN, b"other-batch").is_err());
    }

    #[test]
    fn shares_from_another_envelope_do_not_open() {
        let members = committee(3);
        let publics: Vec<KemPublicKey> = members.iter().map(|m| m.public().clone()).collect();
        let mut rng = fold_core::rng::seeded(9);
        let first = CommitteeEnvelope::seal(&publics, 2, DOMAIN, b"b", b"one", &mut rng).unwrap();
        let second = CommitteeEnvelope::seal(&publics, 2, DOMAIN, b"b", b"two", &mut rng).unwrap();
        let shares: Vec<KeyShare> = (0..2u8)
            .map(|i| first.open_share(i, &members[usize::from(i)], DOMAIN, b"b").unwrap())
            .collect();
        assert!(second.open(&shares, DOMAIN, b"b").is_err());
        assert_eq!(first.open(&shares, DOMAIN, b"b").unwrap(), b"one");
    }
}
