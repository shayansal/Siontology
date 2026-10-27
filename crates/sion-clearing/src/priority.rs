//! Beacon-derived priorities for canonical tie-breaking.
//!
//! When two allocations are equally good, a solver must not get to pick the
//! winner. Each order (or job-slot pair) gets a priority `π ∈ (0, 1)` from a
//! hash of the batch's beacon and its identifier; the beacon is revealed only
//! after the batch seals, so nobody can buy or predict a priority. Adding a
//! tiny `δ·π` to each objective coefficient makes the optimum unique without
//! ever overturning a real improvement.

use num_bigint::BigInt;
use sion_core::{hash_parts, Hash32};
use sion_lp::Q;

/// Domain tag for priority hashing.
pub const PRIORITY_DOMAIN: &str = "sion/clearing/priority";

/// `π = (h + 1) / (2¹²⁸ + 1)` where `h` is the first 16 bytes of
/// `H(beacon ‖ id)` read as a big-endian integer. Always strictly between 0
/// and 1; a shared denominator keeps the exact arithmetic small.
pub fn priority(beacon: &Hash32, id: &Hash32) -> Q {
    let digest = hash_parts(PRIORITY_DOMAIN, &[&beacon.0, &id.0]);
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest.0[..16]);
    let h = u128::from_be_bytes(bytes);
    let numerator = BigInt::from(h) + 1;
    let denominator = (BigInt::from(1u8) << 128) + 1;
    Q::new(numerator, denominator)
}

/// The perturbation scale `δ = 1 / (2·(units + 1))` (in micro-units).
///
/// If at most `units` integral units can move and every price is a whole
/// micro-unit, two allocations whose unperturbed objectives differ differ by
/// at least one micro-unit, while the total perturbation of either is below
/// `δ·units < 1/2`. So the perturbed optimum is always an unperturbed
/// optimum, and ties are broken by priority.
pub fn delta(units: u128) -> Q {
    Q::new(1u8, BigInt::from(units) * 2 + 2)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sion_core::hash_domain;

    #[test]
    fn priorities_are_strictly_inside_the_unit_interval() {
        for i in 0..100u64 {
            let p = priority(&Hash32::ZERO, &hash_domain("t", &i.to_be_bytes()));
            assert!(p.is_positive());
            assert!(p < Q::one());
        }
    }

    #[test]
    fn beacon_changes_priorities() {
        let id = hash_domain("t", b"order");
        let a = priority(&hash_domain("b", b"1"), &id);
        let b = priority(&hash_domain("b", b"2"), &id);
        assert_ne!(a, b);
    }

    #[test]
    fn delta_bounds_total_perturbation() {
        let units = 1_000u128;
        let total = delta(units) * Q::from(units);
        assert!(total < Q::new(1, 2));
    }
}
