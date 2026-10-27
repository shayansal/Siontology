//! Cutting the consensus commit sequence into Pulses.
//!
//! Every honest validator commits the same anchors in the same order with
//! strictly increasing rounds. Pulse `p` of a consensus epoch is everything
//! committed by anchors in rounds `((p − base − 1)·K, (p − base)·K]`, where
//! `K` is the number of rounds per Pulse and `base` the last Pulse executed
//! before the epoch began. A Pulse closes as soon as an anchor beyond its
//! last round commits, so every validator closes the same Pulses with the
//! same contents without any extra agreement. The beacon value of a Pulse
//! comes from the validators' hash-chain reveals committed by its close.

use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use fold_consensus::{Beacon, BeaconOutput, CommittedSubDag};
use fold_core::hash::Hash32;
use fold_core::ids::{Pulse, Round, ValidatorId};

use crate::payload::PayloadItem;

/// How many Pulses ahead a beacon reveal is kept.
pub const REVEAL_WINDOW: u64 = 8;

/// A closed Pulse, ready to execute.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClosedPulse {
    /// The Pulse.
    pub pulse: Pulse,
    /// Its committed payload items, in commit order (reveals removed).
    pub items: Vec<PayloadItem>,
    /// The beacon output.
    pub beacon: BeaconOutput,
    /// Anchors committed in it.
    pub anchors: u64,
}

/// Turns commits into Pulses.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct PulseBuilder {
    rounds_per_pulse: u64,
    base: Pulse,
    next: Pulse,
    buffered: Vec<Vec<u8>>,
    anchors: u64,
    beacon: Beacon,
    reveals: BTreeMap<Pulse, Vec<(ValidatorId, Hash32)>>,
}

impl PulseBuilder {
    /// A builder for a consensus epoch that starts after Pulse `base`.
    pub fn new(rounds_per_pulse: u64, base: Pulse, beacon: Beacon) -> Self {
        PulseBuilder {
            rounds_per_pulse: rounds_per_pulse.max(1),
            base,
            next: base.next(),
            buffered: Vec::new(),
            anchors: 0,
            beacon,
            reveals: BTreeMap::new(),
        }
    }

    /// The next Pulse to close.
    pub fn next_pulse(&self) -> Pulse {
        self.next
    }

    /// The beacon state (persisted with snapshots).
    pub fn beacon(&self) -> &Beacon {
        &self.beacon
    }

    fn last_round(&self, pulse: Pulse) -> Round {
        Round((pulse.0 - self.base.0) * self.rounds_per_pulse)
    }

    /// Absorbs one commit; returns the Pulses it closed, in order.
    pub fn on_commit(&mut self, commit: &CommittedSubDag) -> Vec<ClosedPulse> {
        let mut closed = Vec::new();
        while commit.round > self.last_round(self.next) {
            closed.push(self.close());
        }
        self.buffered.extend(commit.items.iter().cloned());
        self.anchors += 1;
        closed
    }

    fn close(&mut self) -> ClosedPulse {
        let pulse = self.next;
        let mut items = Vec::new();
        for bytes in std::mem::take(&mut self.buffered) {
            match PayloadItem::from_bytes(&bytes) {
                Some(PayloadItem::Reveal {
                    validator,
                    pulse: target,
                    value,
                }) => {
                    // Only valid reveals are kept, for the next few Pulses,
                    // one per validator per Pulse (everyone keeps the same
                    // ones: validity depends only on the beacon's state).
                    if target >= pulse
                        && target.0 - pulse.0 <= REVEAL_WINDOW
                        && self.beacon.check_reveal(validator, target, value)
                    {
                        let waiting = self.reveals.entry(target).or_default();
                        if !waiting.iter().any(|(v, _)| *v == validator) {
                            waiting.push((validator, value));
                        }
                    }
                }
                Some(item) => items.push(item),
                None => {}
            }
        }
        // The beacon collects Pulses in order; skip ahead if needed (a fresh
        // epoch starting after `base`).
        while self.beacon.current_pulse() < pulse {
            self.beacon.finalize();
        }
        for (validator, value) in self.reveals.remove(&pulse).unwrap_or_default() {
            // Invalid or duplicate reveals are ignored: every validator
            // ignores the same ones.
            let _ = self.beacon.submit_reveal(validator, pulse, value);
        }
        let beacon = self.beacon.finalize();
        let anchors = std::mem::take(&mut self.anchors);
        self.next = pulse.next();
        ClosedPulse {
            pulse,
            items,
            beacon,
            anchors,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fold_consensus::HashChain;
    use fold_core::ids::BlockDigest;

    fn commit(round: u64, items: Vec<Vec<u8>>) -> CommittedSubDag {
        CommittedSubDag {
            sequence: round,
            anchor: BlockDigest::derive(&round.to_be_bytes()),
            anchor_author: ValidatorId::derive(b"a"),
            round: Round(round),
            certificates: Vec::new(),
            items,
        }
    }

    #[test]
    fn pulses_close_on_round_boundaries_with_beacon_reveals() {
        let validator = ValidatorId::derive(b"v");
        let chain = HashChain::new(Hash32::ZERO, 100);
        let mut beacon = Beacon::new(Hash32::ZERO);
        beacon.register(validator, chain.tip());
        let mut builder = PulseBuilder::new(2, Pulse(0), beacon);
        let reveal = |p: u64| {
            PayloadItem::Reveal {
                validator,
                pulse: Pulse(p),
                value: chain.reveal_for(Pulse(p)).unwrap(),
            }
            .to_bytes()
        };
        assert!(builder
            .on_commit(&commit(2, vec![reveal(1), b"junk".to_vec()]))
            .is_empty());
        // Round 6 skips Pulse 2's rounds entirely: Pulses 1 and 2 close.
        let closed = builder.on_commit(&commit(6, vec![reveal(3)]));
        assert_eq!(closed.len(), 2);
        assert_eq!(closed[0].pulse, Pulse(1));
        assert_eq!(closed[0].beacon.revealed, vec![validator]);
        assert!(closed[0].items.is_empty(), "undecodable items are dropped");
        assert_eq!(closed[1].pulse, Pulse(2));
        assert_eq!(closed[1].beacon.missing, vec![validator]);
        let closed = builder.on_commit(&commit(8, vec![]));
        assert_eq!(
            closed.iter().map(|c| c.pulse).collect::<Vec<_>>(),
            vec![Pulse(3)]
        );
        assert_eq!(
            closed[0].beacon.revealed,
            vec![validator],
            "a missed Pulse does not break the chain"
        );
        let closed = builder.on_commit(&commit(9, vec![]));
        assert_eq!(
            closed.iter().map(|c| c.pulse).collect::<Vec<_>>(),
            vec![Pulse(4)]
        );
        assert_eq!(
            closed[0].anchors, 1,
            "the round-8 anchor belongs to Pulse 4"
        );
    }
}
