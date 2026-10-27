//! Randomness beacon: hash-chain commit-reveal (RANDAO-like).
//!
//! Each validator registers the tip `h_N` of a private hash chain
//! `h_0 = seed, h_{i+1} = H(h_i)`. In Pulse `t` it reveals `h_{N-t}`, which
//! anyone can check by hashing it forward to the last known value. The
//! Pulse's output hashes the previous output with every valid reveal in
//! validator order, so it is unpredictable as long as one revealer is honest
//! and unknown before the reveals.
//!
//! **Bias.** The last validator to reveal sees everyone else's reveals first
//! and can choose to withhold, choosing between two outputs: one bit of
//! influence per withholding validator, bought at the cost of being recorded
//! as missing (and penalized by the node). A sequential-hash delay function
//! verified by a STARK would remove this bias without leaving post-quantum
//! assumptions; that is future work. Uses here (committee sampling, leader
//! seeds, tie-breaking in clearing) tolerate a few bits of bias.

use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use fold_core::{hash_domain, Hash32, Hasher, Pulse, ValidatorId};
use serde::{Deserialize, Serialize};

use crate::error::ConsensusError;

/// Maximum Pulses a validator may skip between reveals (bounds the hashing
/// work a single reveal can demand).
pub const MAX_SKIPPED_PULSES: u64 = 10_000;

/// One step of a validator's hash chain.
pub fn chain_step(value: &Hash32) -> Hash32 {
    hash_domain("fold/beacon/chain", &value.0)
}

/// A validator's private hash chain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HashChain {
    values: Vec<Hash32>,
}

impl HashChain {
    /// Builds a chain of `length` reveals from a secret seed.
    pub fn new(seed: Hash32, length: usize) -> Self {
        let mut values = Vec::with_capacity(length + 1);
        values.push(seed);
        for i in 0..length {
            let next = chain_step(&values[i]);
            values.push(next);
        }
        HashChain { values }
    }

    /// The public commitment registered with the beacon.
    pub fn tip(&self) -> Hash32 {
        *self.values.last().expect("chain has a seed")
    }

    /// The reveal for Pulse `pulse` (Pulses start at 1).
    pub fn reveal_for(&self, pulse: Pulse) -> Option<Hash32> {
        let last = self.values.len() - 1;
        let pulse = usize::try_from(pulse.0).ok()?;
        if pulse == 0 || pulse > last {
            return None;
        }
        Some(self.values[last - pulse])
    }

    /// The reveal for Pulse `pulse` of a chain registered with the beacon
    /// when it was collecting Pulse `base + 1` (see [`Beacon::base_of`]): the
    /// first reveal hashes one step to the tip.
    pub fn reveal_after(&self, base: Pulse, pulse: Pulse) -> Option<Hash32> {
        self.reveal_for(Pulse(pulse.0.checked_sub(base.0)?))
    }
}

/// The beacon's output for one Pulse.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct BeaconOutput {
    /// The Pulse.
    pub pulse: Pulse,
    /// The random value.
    pub value: Hash32,
    /// Validators whose reveals counted.
    pub revealed: Vec<ValidatorId>,
    /// Registered validators that did not reveal.
    pub missing: Vec<ValidatorId>,
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
struct Commitment {
    last_value: Hash32,
    last_pulse: Pulse,
    /// `last_pulse` at registration: reveals are indexed from it.
    base: Pulse,
}

/// The beacon state machine.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Beacon {
    participants: BTreeMap<ValidatorId, Commitment>,
    reveals: BTreeMap<ValidatorId, Hash32>,
    pulse: Pulse,
    previous: Hash32,
}

impl Beacon {
    /// Starts a beacon from a genesis value; the first Pulse collected is 1.
    pub fn new(genesis: Hash32) -> Self {
        Beacon {
            participants: BTreeMap::new(),
            reveals: BTreeMap::new(),
            pulse: Pulse(1),
            previous: genesis,
        }
    }

    /// Registers a validator's hash-chain tip, effective from the current
    /// Pulse. Re-registering a participant keeps its commitment.
    pub fn register(&mut self, validator: ValidatorId, tip: Hash32) {
        let base = Pulse(self.pulse.0 - 1);
        self.participants.entry(validator).or_insert(Commitment {
            last_value: tip,
            last_pulse: base,
            base,
        });
    }

    /// Removes a participant (a validator that left the active set).
    pub fn deregister(&mut self, validator: &ValidatorId) {
        self.participants.remove(validator);
        self.reveals.remove(validator);
    }

    /// The registered participants.
    pub fn participants(&self) -> Vec<ValidatorId> {
        self.participants.keys().copied().collect()
    }

    /// The Pulse a participant's reveals are indexed from: its reveal for
    /// Pulse `t` is the one its hash chain gives for `t − base`
    /// ([`HashChain::reveal_after`]).
    pub fn base_of(&self, validator: &ValidatorId) -> Option<Pulse> {
        self.participants.get(validator).map(|c| c.base)
    }

    /// The Pulse whose reveals are being collected.
    pub fn current_pulse(&self) -> Pulse {
        self.pulse
    }

    /// The latest output (the genesis value before the first Pulse).
    pub fn latest(&self) -> Hash32 {
        self.previous
    }

    /// Whether `preimage` is `validator`'s valid reveal for `pulse` (a
    /// current or future Pulse), without recording it.
    pub fn check_reveal(&self, validator: ValidatorId, pulse: Pulse, preimage: Hash32) -> bool {
        let Some(commitment) = self.participants.get(&validator) else {
            return false;
        };
        let steps = pulse.0.saturating_sub(commitment.last_pulse.0);
        if steps == 0 || steps > MAX_SKIPPED_PULSES {
            return false;
        }
        let mut value = preimage;
        for _ in 0..steps {
            value = chain_step(&value);
        }
        value == commitment.last_value
    }

    /// Accepts a reveal for the current Pulse.
    pub fn submit_reveal(
        &mut self,
        validator: ValidatorId,
        pulse: Pulse,
        preimage: Hash32,
    ) -> Result<(), ConsensusError> {
        if pulse != self.pulse {
            return Err(ConsensusError::Mismatch("reveal for another pulse"));
        }
        let commitment = self
            .participants
            .get(&validator)
            .ok_or(ConsensusError::UnknownValidator(validator))?;
        if self.reveals.contains_key(&validator) {
            return Err(ConsensusError::Mismatch("already revealed this pulse"));
        }
        let steps = pulse.0.saturating_sub(commitment.last_pulse.0);
        if steps == 0 || steps > MAX_SKIPPED_PULSES {
            return Err(ConsensusError::InvalidReveal(validator));
        }
        let mut value = preimage;
        for _ in 0..steps {
            value = chain_step(&value);
        }
        if value != commitment.last_value {
            return Err(ConsensusError::InvalidReveal(validator));
        }
        self.reveals.insert(validator, preimage);
        Ok(())
    }

    /// Closes the current Pulse and returns its output.
    pub fn finalize(&mut self) -> BeaconOutput {
        let mut hasher = Hasher::new("fold/beacon/output");
        hasher
            .update(&self.previous.0)
            .update(&self.pulse.0.to_be_bytes());
        let mut revealed = Vec::new();
        let mut missing = Vec::new();
        for (validator, commitment) in self.participants.iter_mut() {
            hasher.update(&validator.0 .0);
            match self.reveals.get(validator) {
                Some(reveal) => {
                    hasher.update(&[1]).update(&reveal.0);
                    commitment.last_value = *reveal;
                    commitment.last_pulse = self.pulse;
                    revealed.push(*validator);
                }
                None => {
                    hasher.update(&[0]);
                    missing.push(*validator);
                }
            }
        }
        let output = BeaconOutput {
            pulse: self.pulse,
            value: hasher.finalize(),
            revealed,
            missing,
        };
        self.previous = output.value;
        self.reveals.clear();
        self.pulse = self.pulse.next();
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn validator(i: u8) -> ValidatorId {
        ValidatorId::derive(&[i])
    }

    #[test]
    fn honest_reveals_produce_outputs_and_skips_are_allowed() {
        let chains: Vec<HashChain> = (0..3)
            .map(|i| HashChain::new(hash_domain("seed", &[i]), 20))
            .collect();
        let mut beacon = Beacon::new(hash_domain("genesis", b""));
        for (i, chain) in chains.iter().enumerate() {
            beacon.register(validator(i as u8), chain.tip());
        }
        let mut outputs = Vec::new();
        for pulse in 1..=5u64 {
            for (i, chain) in chains.iter().enumerate() {
                // Validator 2 skips Pulse 3.
                if i == 2 && pulse == 3 {
                    continue;
                }
                beacon
                    .submit_reveal(
                        validator(i as u8),
                        Pulse(pulse),
                        chain.reveal_for(Pulse(pulse)).unwrap(),
                    )
                    .unwrap();
            }
            outputs.push(beacon.finalize());
        }
        assert_eq!(outputs[2].missing, vec![validator(2)]);
        assert!(
            outputs[3].missing.is_empty(),
            "validator 2 resumed after skipping"
        );
        let values: std::collections::BTreeSet<Hash32> = outputs.iter().map(|o| o.value).collect();
        assert_eq!(values.len(), 5);
    }

    #[test]
    fn late_participants_reveal_from_their_registration() {
        let early = HashChain::new(hash_domain("seed", b"early"), 50);
        let late = HashChain::new(hash_domain("seed", b"late"), 50);
        let mut beacon = Beacon::new(Hash32::ZERO);
        beacon.register(validator(1), early.tip());
        for pulse in 1..=5u64 {
            beacon
                .submit_reveal(
                    validator(1),
                    Pulse(pulse),
                    early.reveal_for(Pulse(pulse)).unwrap(),
                )
                .unwrap();
            beacon.finalize();
        }
        // Joining while Pulse 6 is collected: its first reveal is one step
        // from its tip.
        beacon.register(validator(2), late.tip());
        let base = beacon.base_of(&validator(2)).unwrap();
        assert_eq!(base, Pulse(5));
        for pulse in 6..=8u64 {
            for (v, chain, from) in [(1u8, &early, Pulse(0)), (2, &late, base)] {
                beacon
                    .submit_reveal(
                        validator(v),
                        Pulse(pulse),
                        chain.reveal_after(from, Pulse(pulse)).unwrap(),
                    )
                    .unwrap();
            }
            let output = beacon.finalize();
            assert_eq!(output.revealed.len(), 2);
        }
        beacon.deregister(&validator(1));
        assert_eq!(beacon.participants(), vec![validator(2)]);
        assert!(beacon.finalize().missing.contains(&validator(2)));
    }

    #[test]
    fn rejects_bad_reveals() {
        let chain = HashChain::new(hash_domain("seed", b"x"), 10);
        let mut beacon = Beacon::new(Hash32::ZERO);
        beacon.register(validator(1), chain.tip());
        assert_eq!(
            beacon.submit_reveal(validator(1), Pulse(1), hash_domain("wrong", b"")),
            Err(ConsensusError::InvalidReveal(validator(1)))
        );
        assert!(beacon
            .submit_reveal(validator(1), Pulse(2), chain.reveal_for(Pulse(2)).unwrap())
            .is_err());
        assert!(beacon
            .submit_reveal(validator(9), Pulse(1), chain.reveal_for(Pulse(1)).unwrap())
            .is_err());
        beacon
            .submit_reveal(validator(1), Pulse(1), chain.reveal_for(Pulse(1)).unwrap())
            .unwrap();
        assert!(beacon
            .submit_reveal(validator(1), Pulse(1), chain.reveal_for(Pulse(1)).unwrap())
            .is_err());
    }

    #[test]
    fn output_depends_on_every_reveal() {
        let chains: Vec<HashChain> = (0..2)
            .map(|i| HashChain::new(hash_domain("seed", &[i]), 5))
            .collect();
        let run = |withhold: bool| {
            let mut beacon = Beacon::new(Hash32::ZERO);
            for (i, chain) in chains.iter().enumerate() {
                beacon.register(validator(i as u8), chain.tip());
            }
            beacon
                .submit_reveal(
                    validator(0),
                    Pulse(1),
                    chains[0].reveal_for(Pulse(1)).unwrap(),
                )
                .unwrap();
            if !withhold {
                beacon
                    .submit_reveal(
                        validator(1),
                        Pulse(1),
                        chains[1].reveal_for(Pulse(1)).unwrap(),
                    )
                    .unwrap();
            }
            beacon.finalize().value
        };
        // The last revealer can flip between two outputs: the documented bias.
        assert_ne!(run(false), run(true));
    }
}
