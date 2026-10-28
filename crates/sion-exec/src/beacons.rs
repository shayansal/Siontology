//! Beacon records (specification §6.3): the beacon of every Pulse that is a
//! multiple of [`BEACON_INTERVAL`], kept in [`BEACON_SLOTS`] reusable
//! slots, so a transaction can read the beacon of a Pulse fixed in advance
//! for [`BEACON_INTERVAL`] × [`BEACON_SLOTS`] Pulses after it.
//!
//! A transaction that drew its randomness from the beacon of the Pulse it
//! executes in would let whoever times it choose among beacons: a storage
//! provider, with a validator's help, could steer its challenges towards the
//! leaves it kept. Reading the beacon of a Pulse fixed beforehand leaves
//! nothing to choose. Storage deals use it: each round's challenge comes
//! from the beacon of the round's first Pulse (§5.52).
//!
//! The deferred phase of a recorded Pulse writes its record last
//! ([`crate::pulse::apply_deferred`]); the slot of Pulse `p` is
//! `(p / BEACON_INTERVAL) mod BEACON_SLOTS` ([`slot_of`]), the object
//! [`crate::object::well_known::beacon`]. The beacons themselves stay in the
//! certified Pulse headers; a slot is overwritten every
//! [`BEACON_INTERVAL`] × [`BEACON_SLOTS`] Pulses.

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use sion_core::hash::Hash32;
use sion_core::ids::Pulse;

/// Every Pulse that is a multiple of this has its beacon recorded.
pub const BEACON_INTERVAL: u64 = 8;

/// How many slots hold beacon records: with [`BEACON_INTERVAL`], they cover
/// 100,000 Pulses.
pub const BEACON_SLOTS: u64 = 12_500;

/// The beacon of one recorded Pulse.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize,
)]
pub struct BeaconRecord {
    /// The Pulse (a multiple of [`BEACON_INTERVAL`]).
    pub pulse: Pulse,
    /// Its beacon.
    pub beacon: Hash32,
}

/// The slot that records the beacon of `pulse`, or `None` if `pulse` is not
/// a multiple of [`BEACON_INTERVAL`].
pub fn slot_of(pulse: Pulse) -> Option<u32> {
    if pulse.0 % BEACON_INTERVAL != 0 {
        return None;
    }
    u32::try_from((pulse.0 / BEACON_INTERVAL) % BEACON_SLOTS).ok()
}

/// The first multiple of [`BEACON_INTERVAL`] after `pulse` (saturating).
pub fn next_boundary(pulse: Pulse) -> Pulse {
    Pulse(
        (pulse.0 / BEACON_INTERVAL)
            .saturating_add(1)
            .saturating_mul(BEACON_INTERVAL),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_multiples_of_the_interval_are_recorded() {
        assert_eq!(slot_of(Pulse(0)), Some(0));
        assert_eq!(slot_of(Pulse(7)), None);
        assert_eq!(slot_of(Pulse(8)), Some(1));
        assert_eq!(slot_of(Pulse(BEACON_INTERVAL * BEACON_SLOTS)), Some(0));
        assert_eq!(
            slot_of(Pulse(BEACON_INTERVAL * (BEACON_SLOTS - 1))),
            Some((BEACON_SLOTS - 1) as u32)
        );
    }

    #[test]
    fn the_next_boundary_is_strictly_later() {
        assert_eq!(next_boundary(Pulse(0)), Pulse(8));
        assert_eq!(next_boundary(Pulse(1)), Pulse(8));
        assert_eq!(next_boundary(Pulse(7)), Pulse(8));
        assert_eq!(next_boundary(Pulse(8)), Pulse(16));
        assert_eq!(next_boundary(Pulse(u64::MAX)), Pulse(u64::MAX));
    }

    #[test]
    fn the_slots_cover_the_longest_storage_round() {
        assert_eq!(
            BEACON_INTERVAL * BEACON_SLOTS,
            crate::storage::MAX_ROUND_LENGTH
        );
        assert_eq!(crate::storage::MIN_ROUND_LENGTH % BEACON_INTERVAL, 0);
    }
}
