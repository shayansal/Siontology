//! Random sampling by light clients.
//!
//! A validity proof shows a transition was correct; it cannot show that the
//! transition's data was published. Sampling closes that gap: to make a
//! Pulse unrecoverable, a producer must withhold at least `(k + 1)²` of the
//! `4k²` shares — more than a quarter — so each uniformly random sample hits
//! a withheld share with probability above `1/4`, and a client that gets all
//! of its `s` samples back is fooled with probability below `(3/4)^s`
//! (about 1% at `s = 16`, about 0.02% at `s = 30`). Thousands of clients make
//! undetected withholding vanishingly unlikely, provided their requests are
//! unlinkable so a producer cannot answer only the clients it chooses.
//!
//! Probabilities here are `f64` because they are reports, not consensus
//! values.

use std::collections::BTreeSet;

use rand_core::RngCore;
use sion_core::rng::labeled;

use crate::error::DaError;
use crate::square::{DaHeader, Share};

/// The fewest withheld shares that make a `2k × 2k` square unrecoverable.
pub fn min_withholding(k: usize) -> usize {
    (k + 1) * (k + 1)
}

/// Draws `s` distinct positions of the `2k × 2k` square from a seed
/// (capped at the number of cells).
pub fn sample_positions(seed: u64, s: usize, k: usize) -> Vec<(usize, usize)> {
    let width = 2 * k;
    let total = width * width;
    let wanted = s.min(total);
    let mut rng = labeled(seed, "sion/da/sample");
    let mut seen = BTreeSet::new();
    let mut positions = Vec::with_capacity(wanted);
    while positions.len() < wanted {
        let cell = (rng.next_u64() % total as u64) as usize;
        if seen.insert(cell) {
            positions.push((cell / width, cell % width));
        }
    }
    positions
}

/// Probability that one client drawing `s` distinct samples hits at least
/// one share of a minimal unrecoverable withholding (`(k + 1)²` shares):
/// `1 − ∏_{i < s} (1 − (k+1)² / (4k² − i))`.
pub fn detection_probability(k: usize, s: usize) -> f64 {
    1.0 - undetected_probability(k, s)
}

/// Probability that one client's `s` samples all miss a minimal
/// unrecoverable withholding.
pub fn undetected_probability(k: usize, s: usize) -> f64 {
    let total = (4 * k * k) as f64;
    let withheld = min_withholding(k) as f64;
    let mut miss = 1.0f64;
    for i in 0..s {
        let remaining = total - i as f64;
        if remaining <= withheld {
            return 0.0;
        }
        miss *= 1.0 - withheld / remaining;
    }
    miss
}

/// The size-independent bound: one client is fooled with probability at
/// most `(3/4)^s`.
pub fn undetected_bound(s: usize) -> f64 {
    0.75f64.powi(i32::try_from(s).unwrap_or(i32::MAX))
}

/// Probability that every one of `clients` independent clients is fooled.
pub fn crowd_undetected_probability(k: usize, s: usize, clients: u32) -> f64 {
    undetected_probability(k, s).powi(i32::try_from(clients).unwrap_or(i32::MAX))
}

/// The verdict of one sampling round.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Availability {
    /// Every sampled share was returned and verified.
    Available,
    /// Some samples were not returned: treat the Pulse as unavailable.
    Unavailable,
    /// Some returned share failed its proof: the source is serving bad data.
    Invalid,
}

/// The outcome of one sampling round.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SampleReport {
    /// The verdict.
    pub outcome: Availability,
    /// Positions sampled.
    pub positions: Vec<(usize, usize)>,
    /// Shares returned and verified.
    pub verified: usize,
    /// Shares not returned.
    pub missing: usize,
    /// Shares returned that failed verification.
    pub invalid: usize,
}

/// A light client's sampler.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LightSampler {
    /// Samples per Pulse.
    pub samples: usize,
    /// Seed for this client's private randomness.
    pub seed: u64,
}

impl LightSampler {
    /// A sampler drawing `samples` positions per Pulse.
    pub fn new(samples: usize, seed: u64) -> LightSampler {
        LightSampler { samples, seed }
    }

    /// Samples a Pulse: draws positions, fetches each share from `fetch`
    /// (`None` = not served), and verifies each against its row root.
    pub fn run<F>(&self, header: &DaHeader, mut fetch: F) -> Result<SampleReport, DaError>
    where
        F: FnMut(usize, usize) -> Option<Share>,
    {
        header.verify_self_consistent()?;
        let positions = sample_positions(self.seed, self.samples, header.k as usize);
        let (mut verified, mut missing, mut invalid) = (0, 0, 0);
        for &(row, column) in &positions {
            match fetch(row, column) {
                None => missing += 1,
                Some(share) => match header.verify_share(row, column, &share) {
                    Ok(()) => verified += 1,
                    Err(_) => invalid += 1,
                },
            }
        }
        let outcome = if invalid > 0 {
            Availability::Invalid
        } else if missing > 0 {
            Availability::Unavailable
        } else {
            Availability::Available
        };
        Ok(SampleReport {
            outcome,
            positions,
            verified,
            missing,
            invalid,
        })
    }

    /// This sampler's chance of being fooled by a minimal withholding.
    pub fn undetected_probability(&self, k: usize) -> f64 {
        undetected_probability(k, self.samples)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::square::ExtendedSquare;

    #[test]
    fn positions_are_distinct_in_range_and_deterministic() {
        let positions = sample_positions(7, 50, 8);
        assert_eq!(positions.len(), 50);
        let unique: BTreeSet<_> = positions.iter().collect();
        assert_eq!(unique.len(), 50);
        assert!(positions.iter().all(|(r, c)| *r < 16 && *c < 16));
        assert_eq!(positions, sample_positions(7, 50, 8));
        assert_ne!(positions, sample_positions(8, 50, 8));
        assert_eq!(sample_positions(1, 1000, 2).len(), 16);
    }

    #[test]
    fn detection_matches_the_architecture_figures() {
        // (3/4)^16 ≈ 1% and (3/4)^30 ≈ 0.018%.
        assert!((undetected_bound(16) - 0.010_022).abs() < 1e-5);
        assert!((undetected_bound(30) - 0.000_178_6).abs() < 1e-6);
        for k in [2usize, 4, 16, 64, 256] {
            for s in [1usize, 8, 16, 30, 60] {
                let exact = undetected_probability(k, s);
                assert!(exact <= undetected_bound(s) + 1e-12, "k={k} s={s}");
                assert!(detection_probability(k, s) >= 1.0 - undetected_bound(s) - 1e-12);
            }
        }
        // Large squares approach the bound from below.
        assert!((undetected_probability(1024, 16) - undetected_bound(16)).abs() < 2e-4);
        // More samples never hurt.
        for s in 1..40 {
            assert!(undetected_probability(32, s + 1) <= undetected_probability(32, s));
        }
        assert!(crowd_undetected_probability(64, 16, 100) < 1e-150);
    }

    #[test]
    fn sampler_classifies_sources() {
        let square = ExtendedSquare::encode_with_k(&[42u8; 300], 8).unwrap();
        let header = square.header();
        let sampler = LightSampler::new(60, 11);

        let honest = sampler
            .run(&header, |r, c| square.share(r, c).ok())
            .unwrap();
        assert_eq!(honest.outcome, Availability::Available);
        assert_eq!(honest.verified, 60);

        // Withhold the minimal unrecoverable sub-square.
        let k = square.k();
        let withholding = sampler
            .run(&header, |r, c| {
                if r <= k && c <= k {
                    None
                } else {
                    square.share(r, c).ok()
                }
            })
            .unwrap();
        assert_eq!(withholding.outcome, Availability::Unavailable);
        assert!(withholding.missing > 0);

        // Serve a corrupted share.
        let corrupt = sampler
            .run(&header, |r, c| {
                let mut share = square.share(r, c).ok()?;
                if (r + c) % 5 == 0 {
                    share.value += crate::field::Fp::ONE;
                }
                Some(share)
            })
            .unwrap();
        assert_eq!(corrupt.outcome, Availability::Invalid);
    }
}
