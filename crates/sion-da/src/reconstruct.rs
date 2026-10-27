//! Rebuilding the square, and the blob, from a subset of shares.
//!
//! Decoding is iterative: any row or column with at least `k` known symbols
//! is completed, which may give other rows and columns enough symbols to be
//! completed in turn. Light clients that store and serve the shares they
//! sampled collectively hold enough to rebuild the whole Pulse, which is why
//! capacity grows with the number of samplers.

use std::collections::BTreeMap;

use crate::error::{Axis, DaError};
use crate::field::Fp;
use crate::fraud::verify_extended_square;
use crate::rs::decode_axis;
use crate::square::{DaHeader, ExtendedSquare};

/// Completes as much of the square as possible from `shares`
/// (`(row, column) → value`) and returns the cells, `None` where unknown.
///
/// Values are trusted as given; callers should pass verified shares. A
/// wrong value is caught either as an inconsistent axis here or as a root
/// mismatch in [`reconstruct`].
pub fn complete(
    k: usize,
    shares: &BTreeMap<(usize, usize), Fp>,
) -> Result<Vec<Option<Fp>>, DaError> {
    let width = 2 * k;
    let mut grid: Vec<Option<Fp>> = vec![None; width * width];
    for (&(row, column), value) in shares {
        if row >= width || column >= width {
            return Err(DaError::OutOfRange { row, column, width });
        }
        grid[row * width + column] = Some(*value);
    }
    // An axis is "done" once it is complete and checked; its cells never
    // change afterwards, because cells only go from unknown to known.
    let mut done = vec![false; 2 * width];
    loop {
        let mut progress = false;
        for (axis_slot, axis) in [Axis::Row, Axis::Column].into_iter().enumerate() {
            for index in 0..width {
                if done[axis_slot * width + index] {
                    continue;
                }
                let cell = |position: usize| match axis {
                    Axis::Row => index * width + position,
                    Axis::Column => position * width + index,
                };
                let known: Vec<(usize, Fp)> = (0..width)
                    .filter_map(|position| grid[cell(position)].map(|value| (position, value)))
                    .collect();
                if known.len() < k {
                    continue;
                }
                // Decoding also checks every known symbol beyond the first k
                // against the recovered polynomial.
                let full = decode_axis(k, &known, axis, index)?;
                for (position, value) in full.into_iter().enumerate() {
                    let slot = &mut grid[cell(position)];
                    if slot.is_none() {
                        *slot = Some(value);
                        progress = true;
                    }
                }
                done[axis_slot * width + index] = true;
            }
        }
        if !progress {
            break;
        }
    }
    Ok(grid)
}

/// Rebuilds the extended square and checks it against `header`: the roots
/// must match and every axis must be a codeword.
pub fn reconstruct(
    header: &DaHeader,
    shares: &BTreeMap<(usize, usize), Fp>,
) -> Result<ExtendedSquare, DaError> {
    header.verify_self_consistent()?;
    let k = header.k as usize;
    let grid = complete(k, shares)?;
    let missing = grid.iter().filter(|cell| cell.is_none()).count();
    if missing > 0 {
        return Err(DaError::Unrecoverable { missing });
    }
    let cells: Vec<Fp> = grid
        .into_iter()
        .map(|cell| cell.unwrap_or(Fp::ZERO))
        .collect();
    let square = ExtendedSquare::from_cells(k, cells)?;
    if square.header() != *header {
        return Err(DaError::RootMismatch);
    }
    verify_extended_square(&square)?;
    Ok(square)
}

/// Rebuilds the blob from a subset of shares.
pub fn reconstruct_blob(
    header: &DaHeader,
    shares: &BTreeMap<(usize, usize), Fp>,
) -> Result<Vec<u8>, DaError> {
    reconstruct(header, shares)?.decode_blob()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_shares(square: &ExtendedSquare) -> BTreeMap<(usize, usize), Fp> {
        let width = square.width();
        let mut map = BTreeMap::new();
        for r in 0..width {
            for c in 0..width {
                map.insert((r, c), square.get(r, c));
            }
        }
        map
    }

    #[test]
    fn half_missing_patterns_recover() {
        let blob: Vec<u8> = (0..400u32).map(|i| (i * 7 % 256) as u8).collect();
        let square = ExtendedSquare::encode_with_k(&blob, 8).unwrap();
        let header = square.header();
        let width = square.width();
        let k = square.k();
        let patterns: Vec<Box<dyn Fn(usize, usize) -> bool>> = vec![
            Box::new(move |_, c| c >= k),         // right half missing
            Box::new(move |r, _| r >= k),         // bottom half missing
            Box::new(|r, c| (r + c) % 2 == 0),    // checkerboard
            Box::new(move |_, c| c < k),          // all original columns missing
            Box::new(move |r, c| r < k && c < k), // the whole data quadrant
        ];
        for missing in patterns {
            let mut shares = all_shares(&square);
            for r in 0..width {
                for c in 0..width {
                    if missing(r, c) {
                        shares.remove(&(r, c));
                    }
                }
            }
            assert_eq!(reconstruct_blob(&header, &shares).unwrap(), blob);
        }
    }

    #[test]
    fn minimal_withholding_is_unrecoverable() {
        let blob = vec![9u8; 100];
        let square = ExtendedSquare::encode_with_k(&blob, 4).unwrap();
        let header = square.header();
        let k = square.k();
        let mut shares = all_shares(&square);
        for r in 0..=k {
            for c in 0..=k {
                shares.remove(&(r, c));
            }
        }
        assert_eq!(
            reconstruct(&header, &shares),
            Err(DaError::Unrecoverable {
                missing: (k + 1) * (k + 1)
            })
        );
        // One share fewer withheld and it recovers.
        shares.insert((k, k), square.get(k, k));
        assert_eq!(reconstruct_blob(&header, &shares).unwrap(), blob);
    }

    #[test]
    fn wrong_values_are_caught() {
        let blob = vec![3u8; 64];
        let square = ExtendedSquare::encode(&blob).unwrap();
        let header = square.header();
        // A wrong value in a complete set: inconsistent axis.
        let mut shares = all_shares(&square);
        let value = shares[&(1, 1)];
        shares.insert((1, 1), value + Fp::ONE);
        assert!(matches!(
            reconstruct(&header, &shares),
            Err(DaError::InconsistentAxis { .. })
        ));
        // A wrong value that decoding cannot cross-check: root mismatch.
        let k = square.k();
        let width = square.width();
        let mut shares = BTreeMap::new();
        for r in 0..width {
            for c in 0..k {
                shares.insert((r, c), square.get(r, c));
            }
        }
        // Row 0 is decoded from exactly k values, so a bad value there
        // produces a different codeword, caught by the column roots.
        shares.insert((0, 0), square.get(0, 0) + Fp::ONE);
        let result = reconstruct(&header, &shares);
        assert!(matches!(
            result,
            Err(DaError::RootMismatch) | Err(DaError::InconsistentAxis { .. })
        ));
    }
}
