//! Deterministic randomness for simulations and tests.
//!
//! Production key generation uses the operating system's generator; every
//! simulation in this repository is seeded so that runs are reproducible.

use rand_chacha::ChaCha20Rng;
use rand_core::SeedableRng;

use crate::hash::hash_parts;

/// A seeded ChaCha20 generator.
pub fn seeded(seed: u64) -> ChaCha20Rng {
    ChaCha20Rng::seed_from_u64(seed)
}

/// A generator seeded from a label, so independent components of one
/// simulation draw independent streams.
pub fn labeled(seed: u64, label: &str) -> ChaCha20Rng {
    let digest = hash_parts("sion/rng", &[&seed.to_be_bytes(), label.as_bytes()]);
    ChaCha20Rng::from_seed(digest.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand_core::RngCore;

    #[test]
    fn seeded_streams_are_reproducible_and_independent() {
        let mut a = seeded(1);
        let mut b = seeded(1);
        assert_eq!(a.next_u64(), b.next_u64());
        let mut c = labeled(1, "x");
        let mut d = labeled(1, "y");
        assert_ne!(c.next_u64(), d.next_u64());
    }
}
