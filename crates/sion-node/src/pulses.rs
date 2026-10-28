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
//!
//! Reveals may be committed up to [`REVEAL_WINDOW`] Pulses ahead of their
//! Pulse, so what a builder carries past a close (the beacon and the reveals
//! waiting for later Pulses, a [`Carry`]) is part of what decides later
//! Pulses. Every validator that closed a Pulse holds the same carry; it is
//! logged with the Pulse and restored after a restart, so a validator that
//! restarts computes the same beacon as one that never stopped.

use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use sion_consensus::{Beacon, BeaconOutput, CommittedSubDag};
use sion_core::hash::Hash32;
use sion_core::ids::{Pulse, Round, ValidatorId};

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
    /// What the builder carried past this close.
    pub carry: Carry,
}

/// Valid beacon reveals committed ahead of their Pulse: for each Pulse, at
/// most one per validator, in commit order.
pub type PendingReveals = BTreeMap<Pulse, Vec<(ValidatorId, Hash32)>>;

/// What a Pulse builder carries past the close of a Pulse.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Carry {
    /// The beacon, collecting the next Pulse.
    pub beacon: Beacon,
    /// Reveals committed for later Pulses.
    pub reveals: PendingReveals,
}

impl Carry {
    /// A beacon with no reveals waiting (genesis).
    pub fn fresh(beacon: Beacon) -> Carry {
        Carry {
            beacon,
            reveals: PendingReveals::new(),
        }
    }
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
    reveals: PendingReveals,
    last: Option<Pulse>,
}

impl PulseBuilder {
    /// A builder for a consensus session that starts after Pulse `base`,
    /// with no reveals waiting.
    pub fn new(rounds_per_pulse: u64, base: Pulse, beacon: Beacon) -> Self {
        PulseBuilder::resume(rounds_per_pulse, base, Carry::fresh(beacon))
    }

    /// A builder for a consensus session that starts after Pulse `base`,
    /// carrying on from `carry`: the carry of `base`'s close, restored from
    /// the log after a restart, or handed over at a session change. Reveals
    /// for `base` or earlier are dropped.
    pub fn resume(rounds_per_pulse: u64, base: Pulse, carry: Carry) -> Self {
        let mut reveals = carry.reveals;
        reveals.retain(|pulse, _| *pulse > base);
        PulseBuilder {
            rounds_per_pulse: rounds_per_pulse.max(1),
            base,
            next: base.next(),
            buffered: Vec::new(),
            anchors: 0,
            beacon: carry.beacon,
            reveals,
            last: None,
        }
    }

    /// The builder of the session that follows this one after Pulse
    /// `base`: a fresh round count, `beacon` (re-synced with the validator
    /// set by the caller) and the beacon reveals already committed for the
    /// Pulses after `base`, which every validator collected alike.
    pub fn next_session(&self, base: Pulse, beacon: Beacon) -> PulseBuilder {
        PulseBuilder::resume(
            self.rounds_per_pulse,
            base,
            Carry {
                beacon,
                reveals: self.reveals.clone(),
            },
        )
    }

    /// What this builder carries now: as of its latest close, since only a
    /// close changes the beacon and the waiting reveals.
    pub fn carry(&self) -> Carry {
        Carry {
            beacon: self.beacon.clone(),
            reveals: self.reveals.clone(),
        }
    }

    /// Closes no Pulse after `pulse`: the last Pulse of the epoch, after
    /// which the next consensus session takes over. Whatever this session
    /// commits later is never executed by it; [`take_leftover`] returns it
    /// to be proposed again in the next session.
    ///
    /// [`take_leftover`]: PulseBuilder::take_leftover
    pub fn end_after(&mut self, pulse: Pulse) {
        self.last = Some(pulse);
    }

    /// Whether the builder still closes Pulses.
    pub fn is_open(&self) -> bool {
        self.last.map_or(true, |last| self.next <= last)
    }

    /// Payload items committed but not in any closed Pulse.
    pub fn take_leftover(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.buffered)
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
        while self.is_open() && commit.round > self.last_round(self.next) {
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
            carry: self.carry(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sion_consensus::HashChain;
    use sion_core::ids::BlockDigest;

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

    /// Found by the networking review: reveals committed ahead of their
    /// Pulse lived only in the builder, so a validator that restarted alone
    /// computed a different beacon for the Pulses they were for.
    #[test]
    fn a_builder_resumed_from_a_carry_closes_pulses_as_if_it_never_stopped() {
        let (v1, v2) = (ValidatorId::derive(b"v1"), ValidatorId::derive(b"v2"));
        let (c1, c2) = (
            HashChain::new(Hash32([1; 32]), 100),
            HashChain::new(Hash32([2; 32]), 100),
        );
        let mut beacon = Beacon::new(Hash32::ZERO);
        beacon.register(v1, c1.tip());
        beacon.register(v2, c2.tip());
        let reveal = |validator: ValidatorId, chain: &HashChain, p: u64| {
            PayloadItem::Reveal {
                validator,
                pulse: Pulse(p),
                value: chain.reveal_for(Pulse(p)).unwrap(),
            }
            .to_bytes()
        };
        let mut running = PulseBuilder::new(2, Pulse(0), beacon.clone());
        // v1 reveals for Pulses 1 and 2, v2 early for Pulse 3.
        running.on_commit(&commit(
            2,
            vec![reveal(v1, &c1, 1), reveal(v1, &c1, 2), reveal(v2, &c2, 3)],
        ));
        // One commit closes Pulses 1 and 2: each keeps its own carry.
        let closed = running.on_commit(&commit(6, vec![]));
        assert_eq!(closed.len(), 2);
        let after_one = closed[0].carry.clone();
        assert_eq!(after_one.beacon.current_pulse(), Pulse(2));
        assert_eq!(
            after_one.reveals.keys().copied().collect::<Vec<_>>(),
            vec![Pulse(2), Pulse(3)]
        );
        assert_ne!(
            running.carry(),
            after_one,
            "the builder has moved on past Pulse 1"
        );
        let later = running.on_commit(&commit(8, vec![]));

        // A validator that logged Pulse 1 and restarted resumes from its
        // carry in a new session (rounds start again) and closes Pulses 2
        // and 3 with the same beacon.
        let mut resumed = PulseBuilder::resume(2, Pulse(1), after_one.clone());
        let again = resumed.on_commit(&commit(5, vec![]));
        assert_eq!(
            again.iter().map(|c| c.pulse).collect::<Vec<_>>(),
            vec![Pulse(2), Pulse(3)]
        );
        assert_eq!(again[0].beacon, closed[1].beacon);
        assert_eq!(again[1].beacon, later[0].beacon);
        assert_eq!(
            again[1].beacon.revealed,
            vec![v2],
            "the early reveal counted"
        );

        // With the beacon alone, as before the fix, it diverges.
        let mut forgetful = PulseBuilder::new(2, Pulse(1), after_one.beacon);
        let diverged = forgetful.on_commit(&commit(5, vec![]));
        assert_ne!(diverged[0].beacon, closed[1].beacon);
        assert_ne!(diverged[1].beacon, later[0].beacon);
    }

    /// Found by the staking demo: a commit far beyond the epoch's last
    /// Pulse closed the Pulses after it too, so the old session executed a
    /// Pulse the next session then closed again.
    #[test]
    fn a_session_closes_nothing_after_its_last_pulse() {
        let validator = ValidatorId::derive(b"v");
        let chain = HashChain::new(Hash32::ZERO, 100);
        let mut beacon = Beacon::new(Hash32::ZERO);
        beacon.register(validator, chain.tip());
        let mut builder = PulseBuilder::new(2, Pulse(0), beacon);
        builder.end_after(Pulse(2));
        let reveal = |p: u64| {
            PayloadItem::Reveal {
                validator,
                pulse: Pulse(p),
                value: chain.reveal_for(Pulse(p)).unwrap(),
            }
            .to_bytes()
        };
        assert!(builder
            .on_commit(&commit(2, vec![reveal(4), b"late".to_vec()]))
            .is_empty());
        // Round 9 is past Pulses 1 to 4, but the session ends after Pulse 2.
        let closed = builder.on_commit(&commit(9, vec![b"after".to_vec()]));
        assert_eq!(
            closed.iter().map(|c| c.pulse).collect::<Vec<_>>(),
            vec![Pulse(1), Pulse(2)]
        );
        assert!(!builder.is_open());
        assert!(builder
            .on_commit(&commit(11, vec![b"more".to_vec()]))
            .is_empty());
        assert_eq!(
            builder.take_leftover(),
            vec![b"after".to_vec(), b"more".to_vec()],
            "what the old session committed late is proposed again"
        );
        // The next session starts after Pulse 2 with the reveal for Pulse 4.
        let mut next = builder.next_session(Pulse(2), builder.beacon().clone());
        assert_eq!(next.next_pulse(), Pulse(3));
        let closed = next.on_commit(&commit(5, vec![]));
        assert_eq!(
            closed.iter().map(|c| c.pulse).collect::<Vec<_>>(),
            vec![Pulse(3), Pulse(4)]
        );
        assert_eq!(closed[1].beacon.revealed, vec![validator]);
    }
}
