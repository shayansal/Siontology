//! Committees, stake thresholds and committee sampling.
//!
//! Safety in every protocol here rests on quorum intersection: any two sets
//! holding more than two thirds of a committee's stake overlap in more than
//! one third of it, so they share at least one honest member as long as
//! Byzantine stake stays at or below the tolerated fault stake
//! (`total − quorum`). That honest member never signs two conflicting
//! things, so two conflicting certificates cannot both exist.

use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use fold_core::{hash_parts, Epoch, Hash32, ShardId, ValidatorId};
use serde::{Deserialize, Serialize};

use crate::error::ConsensusError;

/// A committee: the validators (with voting weight) that order one shard
/// during one epoch.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct Committee {
    epoch: Epoch,
    shard: ShardId,
    members: Vec<(ValidatorId, u64)>,
    total: u64,
}

impl Committee {
    /// Builds a committee. Members keep the given order, which fixes leader
    /// rotation; each must be unique with non-zero weight.
    pub fn new(
        epoch: Epoch,
        shard: ShardId,
        members: Vec<(ValidatorId, u64)>,
    ) -> Result<Self, ConsensusError> {
        if members.is_empty() {
            return Err(ConsensusError::EmptyCommittee);
        }
        let mut seen = BTreeMap::new();
        let mut total: u64 = 0;
        for (validator, stake) in &members {
            if *stake == 0 {
                return Err(ConsensusError::ZeroStake(*validator));
            }
            if seen.insert(*validator, ()).is_some() {
                return Err(ConsensusError::DuplicateMember(*validator));
            }
            total = total
                .checked_add(*stake)
                .ok_or(ConsensusError::StakeOverflow)?;
        }
        Ok(Committee {
            epoch,
            shard,
            members,
            total,
        })
    }

    /// A committee where every member has weight one.
    pub fn equal_weight(
        epoch: Epoch,
        shard: ShardId,
        validators: impl IntoIterator<Item = ValidatorId>,
    ) -> Result<Self, ConsensusError> {
        Self::new(
            epoch,
            shard,
            validators.into_iter().map(|v| (v, 1)).collect(),
        )
    }

    /// The epoch.
    pub fn epoch(&self) -> Epoch {
        self.epoch
    }

    /// The shard.
    pub fn shard(&self) -> ShardId {
        self.shard
    }

    /// Members and their weights, in leader-rotation order.
    pub fn members(&self) -> &[(ValidatorId, u64)] {
        &self.members
    }

    /// Number of members.
    pub fn size(&self) -> usize {
        self.members.len()
    }

    /// Total weight.
    pub fn total_stake(&self) -> u64 {
        self.total
    }

    /// A member's weight, or zero for non-members.
    pub fn stake_of(&self, validator: &ValidatorId) -> u64 {
        self.members
            .iter()
            .find(|(v, _)| v == validator)
            .map(|(_, s)| *s)
            .unwrap_or(0)
    }

    /// Whether `validator` belongs to the committee.
    pub fn contains(&self, validator: &ValidatorId) -> bool {
        self.stake_of(validator) > 0
    }

    /// The smallest stake strictly greater than two thirds of the total.
    pub fn quorum_threshold(&self) -> u64 {
        (2 * u128::from(self.total) / 3 + 1) as u64
    }

    /// The smallest stake strictly greater than one third of the total: any
    /// such set contains at least one honest member when faults are within
    /// tolerance.
    pub fn validity_threshold(&self) -> u64 {
        self.total / 3 + 1
    }

    /// The largest Byzantine stake the committee tolerates.
    pub fn fault_tolerance(&self) -> u64 {
        self.total - self.quorum_threshold()
    }

    /// Total stake of a set of validators (duplicates and non-members ignored).
    pub fn stake_of_set<'a>(&self, validators: impl IntoIterator<Item = &'a ValidatorId>) -> u64 {
        let mut seen = BTreeMap::new();
        validators
            .into_iter()
            .filter(|v| seen.insert(**v, ()).is_none())
            .map(|v| self.stake_of(v))
            .sum()
    }

    /// Whether `stake` reaches a quorum.
    pub fn is_quorum(&self, stake: u64) -> bool {
        stake >= self.quorum_threshold()
    }

    /// Whether `stake` reaches the validity threshold.
    pub fn is_valid_stake(&self, stake: u64) -> bool {
        stake >= self.validity_threshold()
    }

    /// The member at `index` in rotation order.
    pub fn member_at(&self, index: usize) -> ValidatorId {
        self.members[index % self.members.len()].0
    }
}

/// Samples a shard committee of `seats` validators from `validators`
/// (identity, stake), stake-weighted and without replacement, using the
/// beacon value. Each sampled validator gets one seat (weight one), so the
/// committee's quorum math counts seats, and the chance that a sampled
/// committee is captured is given by [`capture_probability`].
pub fn sample_committee(
    validators: &[(ValidatorId, u64)],
    seats: usize,
    beacon: &Hash32,
    epoch: Epoch,
    shard: ShardId,
) -> Result<Committee, ConsensusError> {
    let mut pool: Vec<(ValidatorId, u64)> = validators
        .iter()
        .copied()
        .filter(|(_, stake)| *stake > 0)
        .collect();
    pool.sort();
    let mut chosen = Vec::new();
    for draw in 0..seats.min(pool.len()) {
        let remaining: u128 = pool.iter().map(|(_, s)| u128::from(*s)).sum();
        let digest = hash_parts(
            "fold/committee/sample",
            &[
                &beacon.0,
                &epoch.0.to_be_bytes(),
                &shard.0.to_be_bytes(),
                &(draw as u64).to_be_bytes(),
            ],
        );
        let mut wide = [0u8; 16];
        wide.copy_from_slice(&digest.0[..16]);
        let mut target = u128::from_be_bytes(wide) % remaining;
        let mut pick = pool.len() - 1;
        for (index, (_, stake)) in pool.iter().enumerate() {
            let stake = u128::from(*stake);
            if target < stake {
                pick = index;
                break;
            }
            target -= stake;
        }
        chosen.push((pool.remove(pick).0, 1));
    }
    chosen.sort();
    Committee::new(epoch, shard, chosen)
}

/// Probability that a committee of `seats` seats, each drawn independently
/// and Byzantine with probability `byzantine_fraction`, has at least one
/// third Byzantine seats (that is, at least `ceil(seats / 3)`), the point at
/// which it can stall or, at more than a third, equivocate. Computed as a
/// binomial tail in log space. This is a reporting statistic, not consensus
/// logic, so it uses floating point.
pub fn capture_probability(byzantine_fraction: f64, seats: u32) -> f64 {
    if seats == 0 {
        return 0.0;
    }
    if byzantine_fraction <= 0.0 {
        return 0.0;
    }
    if byzantine_fraction >= 1.0 {
        return 1.0;
    }
    let n = seats as usize;
    let threshold = n.div_ceil(3);
    let mut log_factorial = vec![0.0f64; n + 1];
    for i in 1..=n {
        log_factorial[i] = log_factorial[i - 1] + (i as f64).ln();
    }
    let ln_p = byzantine_fraction.ln();
    let ln_q = (1.0 - byzantine_fraction).ln();
    let terms: Vec<f64> = (threshold..=n)
        .map(|k| {
            log_factorial[n] - log_factorial[k] - log_factorial[n - k]
                + k as f64 * ln_p
                + (n - k) as f64 * ln_q
        })
        .collect();
    let max = terms.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let sum: f64 = terms.iter().map(|t| (t - max).exp()).sum();
    (max + sum.ln()).exp().min(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(n: usize) -> Vec<ValidatorId> {
        (0..n)
            .map(|i| ValidatorId::derive(&(i as u64).to_be_bytes()))
            .collect()
    }

    #[test]
    fn thresholds_for_equal_weights() {
        for (n, quorum, validity, faults) in [
            (1, 1, 1, 0),
            (4, 3, 2, 1),
            (7, 5, 3, 2),
            (10, 7, 4, 3),
            (301, 201, 101, 100),
        ] {
            let committee = Committee::equal_weight(Epoch(0), ShardId(0), ids(n)).unwrap();
            assert_eq!(committee.quorum_threshold(), quorum, "quorum for n={n}");
            assert_eq!(
                committee.validity_threshold(),
                validity,
                "validity for n={n}"
            );
            assert_eq!(committee.fault_tolerance(), faults, "faults for n={n}");
        }
    }

    #[test]
    fn quorums_intersect_in_more_than_the_fault_stake() {
        for total in 1u64..200 {
            let committee = Committee::new(
                Epoch(0),
                ShardId(0),
                vec![(ValidatorId::derive(b"a"), total)],
            )
            .unwrap();
            let q = committee.quorum_threshold();
            let f = committee.fault_tolerance();
            assert!(
                2 * q > total + f,
                "two quorums share an honest member at total {total}"
            );
            assert!(q + committee.validity_threshold() > total);
        }
    }

    #[test]
    fn rejects_bad_committees() {
        let v = ids(2);
        assert_eq!(
            Committee::new(Epoch(0), ShardId(0), vec![]),
            Err(ConsensusError::EmptyCommittee)
        );
        assert_eq!(
            Committee::new(Epoch(0), ShardId(0), vec![(v[0], 1), (v[0], 2)]),
            Err(ConsensusError::DuplicateMember(v[0]))
        );
        assert_eq!(
            Committee::new(Epoch(0), ShardId(0), vec![(v[0], 0)]),
            Err(ConsensusError::ZeroStake(v[0]))
        );
    }

    #[test]
    fn weighted_thresholds() {
        let v = ids(3);
        let committee = Committee::new(
            Epoch(1),
            ShardId(2),
            vec![(v[0], 50), (v[1], 30), (v[2], 20)],
        )
        .unwrap();
        assert_eq!(committee.total_stake(), 100);
        assert_eq!(committee.quorum_threshold(), 67);
        assert!(committee.is_quorum(committee.stake_of_set(&[v[0], v[2]])));
        assert!(!committee.is_quorum(committee.stake_of_set(&[v[0], v[0], v[1]][..2])));
        assert_eq!(committee.stake_of(&ValidatorId::derive(b"stranger")), 0);
    }

    #[test]
    fn sampling_is_deterministic_and_stake_weighted() {
        let validators: Vec<(ValidatorId, u64)> = ids(20)
            .into_iter()
            .enumerate()
            .map(|(i, v)| (v, if i == 0 { 1_000 } else { 1 }))
            .collect();
        let beacon = fold_core::hash_domain("test/beacon", b"1");
        let a = sample_committee(&validators, 7, &beacon, Epoch(3), ShardId(1)).unwrap();
        let b = sample_committee(&validators, 7, &beacon, Epoch(3), ShardId(1)).unwrap();
        assert_eq!(a, b);
        assert_eq!(a.size(), 7);
        // The heavy validator is almost surely sampled.
        assert!(a.contains(&validators[0].0));
        let other = sample_committee(&validators, 7, &beacon, Epoch(3), ShardId(2)).unwrap();
        assert_ne!(a.members(), other.members());
        // Asking for more seats than validators yields everyone.
        let all = sample_committee(&validators, 50, &beacon, Epoch(3), ShardId(1)).unwrap();
        assert_eq!(all.size(), 20);
    }

    #[test]
    fn capture_probability_matches_the_architecture_document() {
        let p20 = capture_probability(0.20, 301);
        let p25 = capture_probability(0.25, 301);
        assert!((p20 / 2.525e-8 - 1.0).abs() < 0.01, "got {p20}");
        assert!((p25 / 5.48e-4 - 1.0).abs() < 0.01, "got {p25}");
        assert_eq!(capture_probability(0.0, 10), 0.0);
        assert_eq!(capture_probability(1.0, 10), 1.0);
        assert!(capture_probability(0.4, 31) > capture_probability(0.2, 31));
    }
}
