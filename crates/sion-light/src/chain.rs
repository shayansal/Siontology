//! The chain identity a light client pins.
//!
//! A certificate is only as good as the committee it is checked against, so
//! a light client must never take the committee from the node it is
//! checking. The chain identity is a digest of everything that fixes who may
//! certify the chain's Pulses: the genesis state, the validators with their
//! stake and public keys, and the shard layout. Pulse headers carry it, and
//! a client pins it once, out of band (like an SSH host key). From then on
//! it rebuilds every committee it needs from the specification it was
//! served, after checking that the specification hashes to the pinned
//! identity and that every validator identifier derives from its keys.

use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use sion_consensus::{sample_committee, Committee};
use sion_core::encode::encode;
use sion_core::hash::{hash_domain, Hash32};
use sion_core::ids::{Epoch, ShardId, ValidatorId};

use crate::error::LightError;
use crate::header::{validator_id, ValidatorSet};

/// Everything that fixes who may certify a chain's Pulses.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct ChainSpec {
    /// Digest of the genesis state specification.
    pub genesis: Hash32,
    /// The validators and their stake.
    pub validators: Vec<(ValidatorId, u64)>,
    /// Their public keys; each identifier is derived from its keys.
    pub keys: ValidatorSet,
    /// Number of lane shards.
    pub shards: u16,
    /// Seats per shard committee (lane committees are in the state from
    /// then on: sampled from each epoch's active set, rotated when a shard
    /// misses a forced-inclusion deadline).
    pub seats: u16,
    /// INSECURE: validators sign with a simulation MAC instead of dual
    /// post-quantum signatures (load experiments only). Part of the
    /// identity, so a client always knows.
    pub insecure_signatures: bool,
}

impl ChainSpec {
    /// The chain identity: what Pulse headers carry and clients pin.
    pub fn id(&self) -> Hash32 {
        hash_domain("sion/light/chain-spec", &encode(self))
    }

    /// Checks that the specification is well formed: validators are listed
    /// once each with positive stake, and (with real signatures) each
    /// identifier is derived from exactly the keys listed for it.
    pub fn check(&self) -> Result<(), LightError> {
        if self.validators.is_empty() {
            return Err(LightError::Chain("the chain has no validators".into()));
        }
        if self.shards == 0 || self.seats == 0 {
            return Err(LightError::Chain(
                "the chain needs at least one shard and one seat".into(),
            ));
        }
        let mut seen = BTreeMap::new();
        for (id, stake) in &self.validators {
            if *stake == 0 {
                return Err(LightError::Chain(format!("{} has no stake", id.short())));
            }
            if seen.insert(*id, ()).is_some() {
                return Err(LightError::Chain(format!("{} is listed twice", id.short())));
            }
        }
        if self.insecure_signatures {
            return Ok(());
        }
        if self.keys.keys.len() != self.validators.len() {
            return Err(LightError::Chain(
                "keys do not match the validator list".into(),
            ));
        }
        for (id, _) in &self.validators {
            let Some((lattice, hash)) = self.keys.keys.get(id) else {
                return Err(LightError::Chain(format!("no keys for {}", id.short())));
            };
            if validator_id(lattice, hash) != *id {
                return Err(LightError::Chain(format!(
                    "{} is not derived from its keys",
                    id.short()
                )));
            }
        }
        Ok(())
    }

    /// The consensus committee of the first epoch (it certifies Pulse
    /// headers); later epochs' committees are proven from the state
    /// ([`crate::epochs`]).
    pub fn committee(&self, epoch: u64) -> Result<Committee, LightError> {
        Committee::new(Epoch(epoch), ShardId(0), self.validators.clone())
            .map(|committee| committee.on_chain(self.id()))
            .map_err(|e| LightError::Chain(e.to_string()))
    }

    /// The lane committee of each shard in the first epoch, before any
    /// rotation (they certify inputs; equal to what the genesis state
    /// samples).
    pub fn shard_committees(&self, epoch: u64) -> Result<BTreeMap<ShardId, Committee>, LightError> {
        let seats = usize::from(self.seats).clamp(1, self.validators.len());
        let chain = self.id();
        (0..self.shards.max(1))
            .map(|shard| {
                sample_committee(
                    &self.validators,
                    seats,
                    &self.genesis,
                    Epoch(epoch),
                    ShardId(shard),
                )
                .map(|committee| (ShardId(shard), committee.on_chain(chain)))
                .map_err(|e| LightError::Chain(e.to_string()))
            })
            .collect()
    }

    /// Checks the specification against a pinned identity, if any, and that
    /// it is well formed. Returns the identity.
    pub fn verify(&self, pinned: Option<&Hash32>) -> Result<Hash32, LightError> {
        let id = self.id();
        if let Some(pinned) = pinned {
            if *pinned != id {
                return Err(LightError::Chain(format!(
                    "the node serves chain {}, not the pinned chain {}",
                    id.short(),
                    pinned.short()
                )));
            }
        }
        self.check()?;
        Ok(id)
    }
}

#[cfg(test)]
mod tests {
    use sion_crypto::{DualKeypair, Scheme};

    use super::*;

    fn spec(n: usize) -> ChainSpec {
        let mut keys = ValidatorSet::default();
        let mut validators = Vec::new();
        for i in 0..n {
            let pair = DualKeypair::from_seed(&[i as u8; 32], Scheme::SlhDsaSha2_128f);
            let id = keys.insert(pair.lattice.public().clone(), pair.hash.public().clone());
            validators.push((id, 1));
        }
        ChainSpec {
            genesis: hash_domain("test/genesis", b"g"),
            validators,
            keys,
            shards: 2,
            seats: 3,
            insecure_signatures: false,
        }
    }

    #[test]
    fn a_well_formed_spec_verifies_and_fixes_its_committees() {
        let spec = spec(4);
        let id = spec.verify(None).unwrap();
        assert_eq!(spec.verify(Some(&id)).unwrap(), id);
        let committee = spec.committee(3).unwrap();
        assert_eq!(committee.size(), 4);
        assert_eq!(committee.epoch(), Epoch(3));
        let shards = spec.shard_committees(3).unwrap();
        assert_eq!(shards.len(), 2);
        assert!(shards.values().all(|c| c.size() == 3));
        // Deterministic: the same epoch gives the same committees.
        assert_eq!(shards, spec.shard_committees(3).unwrap());
    }

    #[test]
    fn substituted_keys_or_another_chain_are_rejected() {
        let honest = spec(4);
        let id = honest.id();

        // A node that serves its own keys under the honest identifiers.
        let mut forged = honest.clone();
        let attacker = DualKeypair::from_seed(&[99; 32], Scheme::SlhDsaSha2_128f);
        let victim = forged.validators[0].0;
        forged.keys.keys.insert(
            victim,
            (
                attacker.lattice.public().clone(),
                attacker.hash.public().clone(),
            ),
        );
        assert!(forged.check().is_err());
        assert!(forged.verify(Some(&id)).is_err());

        // A self-consistent but different chain fails against the pin.
        let other = spec(5);
        assert!(other.check().is_ok());
        assert!(other.verify(Some(&id)).is_err());

        // Duplicates and zero stake are malformed.
        let mut duplicate = honest.clone();
        duplicate.validators.push(duplicate.validators[0]);
        assert!(duplicate.check().is_err());
        let mut zero = honest;
        zero.validators[1].1 = 0;
        assert!(zero.check().is_err());
    }
}
