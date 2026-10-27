//! Systematic Reed–Solomon coding of one axis (a row or a column).
//!
//! An axis of `k` data symbols is read as the evaluations of a polynomial
//! `f` of degree `< k` at the `k`-th roots of unity `ω_k^j`. Extension
//! evaluates `f` at the other half of the `2k`-th roots, `ω_{2k}^{2j+1}`.
//! The extended axis is laid out as `[data (k) | parity (k)]`:
//!
//! - position `c < k` is the point `ω_{2k}^{2c} = ω_k^c` (original data);
//! - position `k + j` is the point `ω_{2k}^{2j+1}` (parity).
//!
//! Any `k` of the `2k` symbols determine `f`, hence the whole axis. Both
//! halves are computed with size-`k` NTTs (the parity half as a coset NTT),
//! so extension costs `O(k log k)`.

use crate::error::{Axis, DaError};
use crate::field::{batch_inverse, intt, ntt, Fp};

fn log2(n: usize) -> u32 {
    debug_assert!(n.is_power_of_two());
    n.trailing_zeros()
}

/// The evaluation point of position `c` on an extended axis of width `2k`.
pub fn position_point(k: usize, c: usize) -> Fp {
    debug_assert!(c < 2 * k);
    let w = Fp::root_of_unity(log2(2 * k));
    let natural = if c < k { 2 * c } else { 2 * (c - k) + 1 };
    w.pow(natural as u64)
}

/// Extends `k` data symbols to `2k` (`[data | parity]`).
///
/// # Panics
/// Panics if `data.len()` is not a power of two.
pub fn extend_axis(data: &[Fp]) -> Vec<Fp> {
    let k = data.len();
    assert!(k.is_power_of_two(), "axis length must be a power of two");
    let log_k = log2(k);
    let root_k = Fp::root_of_unity(log_k);
    let mut coefficients = data.to_vec();
    intt(&mut coefficients, root_k);
    // Shift onto the coset ω_{2k}·⟨ω_k⟩: coefficient i is scaled by ω_{2k}^i.
    let shift = Fp::root_of_unity(log_k + 1);
    let mut factor = Fp::ONE;
    for coefficient in coefficients.iter_mut() {
        *coefficient *= factor;
        factor *= shift;
    }
    ntt(&mut coefficients, root_k);
    let mut extended = Vec::with_capacity(2 * k);
    extended.extend_from_slice(data);
    extended.extend_from_slice(&coefficients);
    extended
}

/// Whether a full axis of `2k` symbols is a codeword (its parity half is
/// the extension of its data half).
///
/// # Panics
/// Panics if the length is not twice a power of two.
pub fn is_codeword(axis: &[Fp]) -> bool {
    let k = axis.len() / 2;
    assert!(
        k.is_power_of_two() && axis.len() == 2 * k,
        "axis length must be 2k"
    );
    extend_axis(&axis[..k])[k..] == axis[k..]
}

/// The first position at which a full axis departs from the codeword
/// determined by its data half, if any.
pub fn first_inconsistency(axis: &[Fp]) -> Option<usize> {
    let k = axis.len() / 2;
    let expected = extend_axis(&axis[..k]);
    (k..2 * k).find(|&c| expected[c] != axis[c])
}

/// Evaluates the polynomial through `(xs[i], ys[i])` (degree `< xs.len()`)
/// at each target, using barycentric Lagrange interpolation.
///
/// # Panics
/// Panics if the `xs` are not distinct or the lengths differ.
pub fn interpolate_at(xs: &[Fp], ys: &[Fp], targets: &[Fp]) -> Vec<Fp> {
    assert_eq!(xs.len(), ys.len(), "one value per point");
    let n = xs.len();
    let mut denominators = vec![Fp::ONE; n];
    for j in 0..n {
        for m in 0..n {
            if m != j {
                denominators[j] *= xs[j] - xs[m];
            }
        }
    }
    let weights = batch_inverse(&denominators);
    targets
        .iter()
        .map(|target| {
            if let Some(index) = xs.iter().position(|x| x == target) {
                return ys[index];
            }
            let differences: Vec<Fp> = xs.iter().map(|x| *target - *x).collect();
            let inverses = batch_inverse(&differences);
            let ell = differences.iter().fold(Fp::ONE, |acc, d| acc * *d);
            let sum = weights
                .iter()
                .zip(ys)
                .zip(&inverses)
                .fold(Fp::ZERO, |acc, ((w, y), inv)| acc + *w * *y * *inv);
            ell * sum
        })
        .collect()
}

/// Recovers a full axis of width `2k` from at least `k` known positions.
///
/// The first `k` known positions determine the polynomial; every other known
/// position must agree with it, or the axis is reported as inconsistent
/// (which, for committed shares, is proof of bad encoding).
pub fn decode_axis(
    k: usize,
    known: &[(usize, Fp)],
    axis: Axis,
    index: usize,
) -> Result<Vec<Fp>, DaError> {
    if known.len() < k {
        return Err(DaError::NotEnoughShares {
            needed: k,
            have: known.len(),
        });
    }
    let base = &known[..k];
    let xs: Vec<Fp> = base.iter().map(|(c, _)| position_point(k, *c)).collect();
    let ys: Vec<Fp> = base.iter().map(|(_, v)| *v).collect();
    let mut data = vec![Fp::ZERO; k];
    let mut missing_positions = Vec::new();
    let mut known_data = vec![false; k];
    for (c, value) in base {
        if *c < k {
            data[*c] = *value;
            known_data[*c] = true;
        }
    }
    for (c, slot) in known_data.iter().enumerate() {
        if !slot {
            missing_positions.push(c);
        }
    }
    if !missing_positions.is_empty() {
        let targets: Vec<Fp> = missing_positions
            .iter()
            .map(|c| position_point(k, *c))
            .collect();
        let values = interpolate_at(&xs, &ys, &targets);
        for (c, value) in missing_positions.iter().zip(values) {
            data[*c] = value;
        }
    }
    let full = extend_axis(&data);
    if known.iter().any(|(c, value)| full[*c] != *value) {
        return Err(DaError::InconsistentAxis { axis, index });
    }
    Ok(full)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::field::evaluate;

    fn sample(k: usize, seed: u64) -> Vec<Fp> {
        (0..k as u64)
            .map(|i| Fp::new((i + 1) * 0x9e37_79b9 + seed))
            .collect()
    }

    #[test]
    fn extension_is_systematic_and_on_one_polynomial() {
        for log_k in 0..=6 {
            let k = 1usize << log_k;
            let data = sample(k, 5);
            let extended = extend_axis(&data);
            assert_eq!(&extended[..k], &data[..]);
            // All 2k symbols are evaluations of one polynomial of degree < k.
            let mut coefficients = data.clone();
            intt(&mut coefficients, Fp::root_of_unity(log_k));
            for (c, value) in extended.iter().enumerate() {
                assert_eq!(*value, evaluate(&coefficients, position_point(k, c)));
            }
            assert!(is_codeword(&extended));
        }
    }

    #[test]
    fn tampering_breaks_codewords() {
        let extended = extend_axis(&sample(8, 1));
        for c in 0..16 {
            let mut bad = extended.clone();
            bad[c] += Fp::ONE;
            assert!(!is_codeword(&bad));
            assert!(first_inconsistency(&bad).is_some());
        }
    }

    #[test]
    fn decode_from_any_k_positions() {
        let k = 8;
        let extended = extend_axis(&sample(k, 9));
        // Every contiguous window and a strided pattern.
        for start in 0..=k {
            let known: Vec<(usize, Fp)> = (start..start + k).map(|c| (c, extended[c])).collect();
            assert_eq!(decode_axis(k, &known, Axis::Row, 0).unwrap(), extended);
        }
        let known: Vec<(usize, Fp)> = (0..2 * k).step_by(2).map(|c| (c, extended[c])).collect();
        assert_eq!(decode_axis(k, &known, Axis::Row, 0).unwrap(), extended);
    }

    #[test]
    fn decode_detects_inconsistency_and_shortage() {
        let k = 4;
        let extended = extend_axis(&sample(k, 2));
        let mut known: Vec<(usize, Fp)> = (0..2 * k).map(|c| (c, extended[c])).collect();
        known[6].1 += Fp::ONE;
        assert_eq!(
            decode_axis(k, &known, Axis::Column, 3),
            Err(DaError::InconsistentAxis {
                axis: Axis::Column,
                index: 3
            })
        );
        let few: Vec<(usize, Fp)> = (0..k - 1).map(|c| (c, extended[c])).collect();
        assert!(matches!(
            decode_axis(k, &few, Axis::Row, 0),
            Err(DaError::NotEnoughShares { .. })
        ));
    }
}
