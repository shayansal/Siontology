//! Encoding checks and bad-encoding fraud proofs.
//!
//! Hash commitments bind the shares but not the *encoding*: a producer can
//! commit to a square whose rows are not Reed–Solomon codewords, which would
//! make reconstruction from samples fail even though every sampled share
//! verifies. In the Fold the World Proof proves that each Pulse's square is
//! correctly encoded (in the style of ZODA), so light clients that check the
//! World Proof need nothing more. Nodes that only sample, or that follow the
//! chain before proof finality, use the fallback here:
//!
//! - [`verify_extended_square`] lets anyone holding a full square check it;
//! - a [`BadEncodingProof`] is `k + 1` shares of one axis, each with a Merkle
//!   proof, that lie on no polynomial of degree `< k` — or one cell whose
//!   row and column commitments disagree. It is small and checkable against
//!   the header alone.

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};

use crate::error::{Axis, DaError};
use crate::field::Fp;
use crate::rs::{first_inconsistency, interpolate_at, is_codeword, position_point};
use crate::square::{DaHeader, ExtendedSquare, Share};

/// Checks that every row and every column of a full square is a codeword.
pub fn verify_extended_square(square: &ExtendedSquare) -> Result<(), DaError> {
    for axis in [Axis::Row, Axis::Column] {
        for index in 0..square.width() {
            if !is_codeword(&square.axis(axis, index)) {
                return Err(DaError::BadEncoding { axis, index });
            }
        }
    }
    Ok(())
}

/// One share of an axis, with its position along the axis.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct AxisShare {
    /// Position along the axis (`0..2k`).
    pub position: u32,
    /// The share and its proof to the axis root.
    pub share: Share,
}

/// Evidence that a committed square is not correctly encoded.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub enum BadEncodingProof {
    /// `k + 1` committed shares of one axis that lie on no polynomial of
    /// degree `< k`.
    NotACodeword {
        /// The axis.
        axis: Axis,
        /// The axis index.
        index: u32,
        /// Exactly `k + 1` shares at distinct positions.
        shares: Vec<AxisShare>,
    },
    /// The row commitment and the column commitment disagree on one cell.
    AxisMismatch {
        /// Row of the cell.
        row: u32,
        /// Column of the cell.
        column: u32,
        /// The cell as committed by its row root.
        row_share: Share,
        /// The cell as committed by its column root.
        column_share: Share,
    },
}

impl ExtendedSquare {
    /// Produces a fraud proof if this square is not correctly encoded.
    pub fn find_bad_encoding(&self) -> Option<BadEncodingProof> {
        let k = self.k();
        for axis in [Axis::Row, Axis::Column] {
            for index in 0..self.width() {
                let values = self.axis(axis, index);
                let Some(bad_position) = first_inconsistency(&values) else {
                    continue;
                };
                let positions = (0..k).chain(std::iter::once(bad_position));
                let mut shares = Vec::with_capacity(k + 1);
                for position in positions {
                    let (row, column) = match axis {
                        Axis::Row => (index, position),
                        Axis::Column => (position, index),
                    };
                    let share = self.axis_share(axis, row, column).ok()?;
                    shares.push(AxisShare {
                        position: position as u32,
                        share,
                    });
                }
                return Some(BadEncodingProof::NotACodeword {
                    axis,
                    index: index as u32,
                    shares,
                });
            }
        }
        None
    }
}

/// Verifies a fraud proof against a header. `Ok(())` means the proof
/// establishes that the committed square is badly encoded.
pub fn verify_bad_encoding_proof(
    header: &DaHeader,
    proof: &BadEncodingProof,
) -> Result<(), DaError> {
    header.verify_self_consistent()?;
    let k = header.k as usize;
    let width = header.width();
    match proof {
        BadEncodingProof::NotACodeword {
            axis,
            index,
            shares,
        } => {
            let index = *index as usize;
            if index >= width {
                return Err(DaError::InvalidFraudProof("axis index out of range"));
            }
            if shares.len() != k + 1 {
                return Err(DaError::InvalidFraudProof("expected exactly k + 1 shares"));
            }
            let mut seen = vec![false; width];
            for axis_share in shares {
                let position = axis_share.position as usize;
                if position >= width || seen[position] {
                    return Err(DaError::InvalidFraudProof(
                        "positions must be distinct and in range",
                    ));
                }
                seen[position] = true;
                let (row, column) = match axis {
                    Axis::Row => (index, position),
                    Axis::Column => (position, index),
                };
                header
                    .verify_axis_share(*axis, row, column, &axis_share.share)
                    .map_err(|_| DaError::InvalidFraudProof("share fails its Merkle proof"))?;
            }
            let xs: Vec<Fp> = shares[..k]
                .iter()
                .map(|s| position_point(k, s.position as usize))
                .collect();
            let ys: Vec<Fp> = shares[..k].iter().map(|s| s.share.value).collect();
            let last = &shares[k];
            let predicted =
                interpolate_at(&xs, &ys, &[position_point(k, last.position as usize)])[0];
            if predicted == last.share.value {
                Err(DaError::InvalidFraudProof(
                    "shares lie on one polynomial of degree < k",
                ))
            } else {
                Ok(())
            }
        }
        BadEncodingProof::AxisMismatch {
            row,
            column,
            row_share,
            column_share,
        } => {
            let (row, column) = (*row as usize, *column as usize);
            header
                .verify_share(row, column, row_share)
                .map_err(|_| DaError::InvalidFraudProof("row share fails its proof"))?;
            header
                .verify_column_share(row, column, column_share)
                .map_err(|_| DaError::InvalidFraudProof("column share fails its proof"))?;
            if row_share.value == column_share.value {
                Err(DaError::InvalidFraudProof("row and column agree"))
            } else {
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tampered() -> (ExtendedSquare, DaHeader) {
        let mut square = ExtendedSquare::encode_with_k(b"tamper with me, then commit", 4).unwrap();
        let value = square.get(2, 5);
        square.set(2, 5, value + Fp::ONE);
        // The producer commits to the tampered square: every share verifies.
        let header = square.header();
        (square, header)
    }

    #[test]
    fn honest_square_passes_and_has_no_fraud_proof() {
        let square = ExtendedSquare::encode(b"honest").unwrap();
        verify_extended_square(&square).unwrap();
        assert!(square.find_bad_encoding().is_none());
    }

    #[test]
    fn tampered_square_yields_a_verifiable_fraud_proof() {
        let (square, header) = tampered();
        assert!(matches!(
            verify_extended_square(&square),
            Err(DaError::BadEncoding { .. })
        ));
        // Shares still verify against the (malicious) commitment...
        header
            .verify_share(2, 5, &square.share(2, 5).unwrap())
            .unwrap();
        // ...but the encoding fraud is provable from k + 1 of them.
        let proof = square.find_bad_encoding().expect("bad encoding found");
        verify_bad_encoding_proof(&header, &proof).unwrap();
        let encoded = borsh::to_vec(&proof).unwrap();
        let decoded: BadEncodingProof = borsh::from_slice(&encoded).unwrap();
        assert_eq!(decoded, proof);
    }

    #[test]
    fn fraud_proofs_against_honest_squares_are_rejected() {
        let square = ExtendedSquare::encode_with_k(b"honest square", 4).unwrap();
        let header = square.header();
        let k = square.k();
        let shares: Vec<AxisShare> = (0..=k)
            .map(|position| AxisShare {
                position: position as u32,
                share: square.share(1, position).unwrap(),
            })
            .collect();
        let proof = BadEncodingProof::NotACodeword {
            axis: Axis::Row,
            index: 1,
            shares: shares.clone(),
        };
        assert!(verify_bad_encoding_proof(&header, &proof).is_err());
        // Forging a value breaks the Merkle proof instead.
        let mut forged = shares;
        forged[k].share.value += Fp::ONE;
        let proof = BadEncodingProof::NotACodeword {
            axis: Axis::Row,
            index: 1,
            shares: forged,
        };
        assert_eq!(
            verify_bad_encoding_proof(&header, &proof),
            Err(DaError::InvalidFraudProof("share fails its Merkle proof"))
        );
    }

    #[test]
    fn axis_mismatch_proof() {
        // A producer that mixes row roots of one square with column roots of
        // another commits to no single square.
        let a = ExtendedSquare::encode_with_k(b"square a", 2).unwrap();
        let mut b = a.clone();
        let value = b.get(0, 1);
        b.set(0, 1, value + Fp::ONE);
        let header = DaHeader::new(2, a.header().row_roots, b.header().col_roots);
        let proof = BadEncodingProof::AxisMismatch {
            row: 0,
            column: 1,
            row_share: a.share(0, 1).unwrap(),
            column_share: b.column_share(0, 1).unwrap(),
        };
        verify_bad_encoding_proof(&header, &proof).unwrap();
        // Agreeing cells are not evidence.
        let proof = BadEncodingProof::AxisMismatch {
            row: 1,
            column: 1,
            row_share: a.share(1, 1).unwrap(),
            column_share: b.column_share(1, 1).unwrap(),
        };
        assert!(verify_bad_encoding_proof(&header, &proof).is_err());
    }
}
