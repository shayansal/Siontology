//! Committees, derived from the state.
//!
//! Who certifies what is part of the state transition: the validator set
//! (`sion_exec::validators`) holds the active set of the current epoch and
//! each shard's lane members (sampled every epoch, replaced when the shard
//! misses a forced-inclusion deadline). Every validator derives the same
//! committees from its state after each Pulse; nothing here is configured.
//!
//! Consensus runs one DAG per **session**: a new session starts at every
//! epoch boundary and at every restart of the devnet, so round numbers
//! never repeat within a session. A session number combines the devnet's
//! restart generation and the state epoch.

use std::collections::BTreeMap;

use sion_consensus::Committee;
use sion_core::hash::Hash32;
use sion_core::ids::{Epoch, Pulse, ShardId};
use sion_exec::State;

/// The committees a validator works with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Committees {
    /// The state epoch they belong to.
    pub epoch: u64,
    /// The consensus session.
    pub session: u64,
    /// The consensus committee: the active set, weighted, with the session
    /// as its committee epoch.
    pub consensus: Committee,
    /// Who signs Pulse headers: the same members, with the state epoch.
    pub signers: Committee,
    /// Lane committees by shard.
    pub lanes: BTreeMap<ShardId, Committee>,
}

/// The session for a restart generation and a state epoch.
pub fn session(generation: u64, epoch: u64) -> u64 {
    (generation.min(u64::from(u32::MAX)) << 32) | (epoch & u64::from(u32::MAX))
}

impl Committees {
    /// The committees of `state` on chain `chain`, for restart generation
    /// `generation`, with `shards` lane shards.
    pub fn derive(
        state: &State,
        chain: Hash32,
        generation: u64,
        shards: u16,
    ) -> Result<Committees, String> {
        let set = state.validators();
        let session = session(generation, set.epoch);
        let consensus = Committee::new(Epoch(session), ShardId(0), set.active.clone())
            .map_err(|e| e.to_string())?
            .on_chain(chain);
        let signers = set.consensus_committee(chain).map_err(|e| e.to_string())?;
        let lanes = set
            .lane_committees(chain, shards.max(1))
            .map_err(|e| e.to_string())?;
        Ok(Committees {
            epoch: set.epoch,
            session,
            consensus,
            signers,
            lanes,
        })
    }
}

/// What the gateway and the API read: the latest committees any validator
/// derived, and the keys of every registered validator.
#[derive(Clone, Debug, Default)]
pub struct CommitteeView {
    /// The Pulse whose post-state they were derived from.
    pub pulse: Pulse,
    /// The committees.
    pub committees: Option<Committees>,
    /// Every registered validator's public keys.
    pub keys: sion_light::ValidatorSet,
}

impl CommitteeView {
    /// Replaces the view if `pulse` is newer.
    pub fn update(&mut self, pulse: Pulse, committees: &Committees, state: &State) {
        if self.committees.is_some() && pulse <= self.pulse {
            return;
        }
        self.pulse = pulse;
        self.committees = Some(committees.clone());
        let set = state.validators();
        if self.keys.keys.len() != set.validators.len() {
            self.keys = registered_keys(set);
        }
    }
}

/// The public keys of every validator `set` registers (active or not: a
/// leaving validator's signatures stay checkable as evidence).
pub fn registered_keys(set: &sion_exec::validators::ValidatorSet) -> sion_light::ValidatorSet {
    let mut keys = sion_light::ValidatorSet::default();
    for record in set.validators.values() {
        keys.keys
            .insert(record.id, (record.lattice.clone(), record.hash.clone()));
    }
    keys
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sessions_change_with_the_epoch_and_the_restart_generation() {
        assert_ne!(session(0, 1), session(1, 0));
        assert_ne!(session(0, 1), session(0, 2));
        assert!(
            session(1, 0) > session(0, 9),
            "a restart outranks every epoch before it"
        );
    }
}
