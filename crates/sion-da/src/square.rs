//! The extended `2k × 2k` square, its commitments and its shares.
//!
//! Construction (a tensor Reed–Solomon code):
//!
//! 1. the blob is laid out as a `k × k` data square ([`crate::layout`]);
//! 2. each of the `k` data rows is extended to `2k` symbols;
//! 3. each of the `2k` columns of that `k × 2k` block is extended to `2k`.
//!
//! The top-left `k × k` quadrant is the original data. Because the code is
//! linear, every row *and* every column of the result is a codeword, so the
//! data survives the loss of any pattern that leaves each row or column
//! with `k` known symbols, iterated. Making the data unrecoverable requires
//! withholding at least `(k + 1)²` of the `4k²` shares — over a quarter —
//! which is what makes random sampling effective.
//!
//! Commitments are hash-based: a Merkle root per row and per column (SHA3
//! with domain separation, via `sion-core`), and a data root over all of
//! them. Unlike KZG polynomial commitments, this needs no trusted setup and
//! no pairings, so nothing here is broken by a quantum computer. The price
//! is that commitments do not prove the encoding is correct; in the Fold the
//! World Proof proves correct encoding (ZODA-style), and
//! [`crate::fraud`] proofs let sampling-only nodes catch a bad encoding.

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use sion_core::merkle::{inclusion_proof, root, verify_inclusion};
use sion_core::{hash_domain, hash_parts, Hash32};

use crate::error::{Axis, DaError};
use crate::field::Fp;
use crate::layout::{blob_to_square, choose_k, square_to_blob, validate_k};
use crate::rs::extend_axis;

const SHARE_DOMAIN: &str = "sion/da/share";
const AXIS_ROOT_DOMAIN: &str = "sion/da/axis-root";
const DATA_ROOT_DOMAIN: &str = "sion/da/data-root";

/// Leaf hash of one share value. Position is bound by the Merkle index and
/// by which axis root the proof verifies against.
pub fn share_leaf(value: Fp) -> Hash32 {
    hash_domain(SHARE_DOMAIN, &value.to_bytes())
}

/// One share with a Merkle inclusion proof to its row root (or, for
/// column shares, its column root).
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct Share {
    /// The symbol.
    pub value: Fp,
    /// Audit path from the share's leaf to the axis root.
    pub proof: Vec<Hash32>,
}

/// The commitment to one extended square, small enough for a light client.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct DaHeader {
    /// Side of the data square; the extended square is `2k` wide.
    pub k: u32,
    /// Merkle root of each of the `2k` rows.
    pub row_roots: Vec<Hash32>,
    /// Merkle root of each of the `2k` columns.
    pub col_roots: Vec<Hash32>,
    /// Commitment to `k` and all row and column roots.
    pub data_root: Hash32,
}

impl DaHeader {
    /// Width of the extended square.
    pub fn width(&self) -> usize {
        2 * self.k as usize
    }

    /// Builds a header from axis roots, computing the data root.
    pub fn new(k: u32, row_roots: Vec<Hash32>, col_roots: Vec<Hash32>) -> DaHeader {
        let data_root = Self::compute_data_root(k, &row_roots, &col_roots);
        DaHeader {
            k,
            row_roots,
            col_roots,
            data_root,
        }
    }

    /// The data root for `k` and the given axis roots: the Merkle root of
    /// tagged axis roots (rows, then columns), bound to `k`.
    pub fn compute_data_root(k: u32, row_roots: &[Hash32], col_roots: &[Hash32]) -> Hash32 {
        let tagged = |tag: u8, index: usize, axis_root: &Hash32| {
            hash_parts(
                AXIS_ROOT_DOMAIN,
                &[&[tag], &(index as u32).to_be_bytes(), &axis_root.0],
            )
        };
        let leaves: Vec<Hash32> = row_roots
            .iter()
            .enumerate()
            .map(|(i, r)| tagged(0, i, r))
            .chain(col_roots.iter().enumerate().map(|(i, c)| tagged(1, i, c)))
            .collect();
        hash_parts(DATA_ROOT_DOMAIN, &[&k.to_be_bytes(), &root(&leaves).0])
    }

    /// Checks sizes and the data root.
    pub fn verify_self_consistent(&self) -> Result<(), DaError> {
        validate_k(self.k as usize)?;
        let width = self.width();
        if self.row_roots.len() != width || self.col_roots.len() != width {
            return Err(DaError::HeaderInconsistent(
                "expected 2k row and 2k column roots",
            ));
        }
        if Self::compute_data_root(self.k, &self.row_roots, &self.col_roots) != self.data_root {
            return Err(DaError::HeaderInconsistent(
                "data root does not match axis roots",
            ));
        }
        Ok(())
    }

    /// The root of one axis.
    pub fn axis_root(&self, axis: Axis, index: usize) -> Option<&Hash32> {
        match axis {
            Axis::Row => self.row_roots.get(index),
            Axis::Column => self.col_roots.get(index),
        }
    }

    fn check_position(&self, row: usize, column: usize) -> Result<(), DaError> {
        let width = self.width();
        if row >= width || column >= width {
            return Err(DaError::OutOfRange { row, column, width });
        }
        Ok(())
    }

    /// Verifies a share against its row root.
    pub fn verify_share(&self, row: usize, column: usize, share: &Share) -> Result<(), DaError> {
        self.verify_axis_share(Axis::Row, row, column, share)
    }

    /// Verifies a share against its column root.
    pub fn verify_column_share(
        &self,
        row: usize,
        column: usize,
        share: &Share,
    ) -> Result<(), DaError> {
        self.verify_axis_share(Axis::Column, row, column, share)
    }

    /// Verifies a share against the root of `axis` (row proof: index is the
    /// column; column proof: index is the row).
    pub fn verify_axis_share(
        &self,
        axis: Axis,
        row: usize,
        column: usize,
        share: &Share,
    ) -> Result<(), DaError> {
        self.check_position(row, column)?;
        let (axis_index, position) = match axis {
            Axis::Row => (row, column),
            Axis::Column => (column, row),
        };
        let axis_root = self
            .axis_root(axis, axis_index)
            .ok_or(DaError::HeaderInconsistent("missing axis root"))?;
        verify_inclusion(
            axis_root,
            self.width() as u64,
            position as u64,
            &share_leaf(share.value),
            &share.proof,
        )
        .map_err(|_| DaError::InvalidShare { row, column })
    }
}

/// Verifies a share against its row root (free-function form).
pub fn verify_share(
    header: &DaHeader,
    row: usize,
    column: usize,
    share: &Share,
) -> Result<(), DaError> {
    header.verify_share(row, column, share)
}

/// A fully materialized extended square.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtendedSquare {
    k: usize,
    /// `(2k)²` symbols, row-major.
    cells: Vec<Fp>,
}

impl ExtendedSquare {
    /// Encodes a blob with the smallest square that holds it.
    pub fn encode(blob: &[u8]) -> Result<ExtendedSquare, DaError> {
        ExtendedSquare::encode_with_k(blob, choose_k(blob.len())?)
    }

    /// Encodes a blob into a caller-chosen `k × k` data square.
    pub fn encode_with_k(blob: &[u8], k: usize) -> Result<ExtendedSquare, DaError> {
        let data = blob_to_square(blob, k)?;
        Ok(ExtendedSquare::from_data_square(k, &data))
    }

    /// Extends an arbitrary `k × k` symbol square (row-major).
    ///
    /// # Panics
    /// Panics if `data.len() != k * k` or `k` is not a power of two.
    pub fn from_data_square(k: usize, data: &[Fp]) -> ExtendedSquare {
        assert_eq!(data.len(), k * k, "data square must be k × k");
        let width = 2 * k;
        let mut cells = vec![Fp::ZERO; width * width];
        for r in 0..k {
            let extended = extend_axis(&data[r * k..(r + 1) * k]);
            cells[r * width..(r + 1) * width].copy_from_slice(&extended);
        }
        let mut column = vec![Fp::ZERO; k];
        for c in 0..width {
            for (r, slot) in column.iter_mut().enumerate() {
                *slot = cells[r * width + c];
            }
            let extended = extend_axis(&column);
            for (r, value) in extended.iter().enumerate().skip(k) {
                cells[r * width + c] = *value;
            }
        }
        ExtendedSquare { k, cells }
    }

    /// Wraps raw cells, for example from reconstruction or from an untrusted
    /// producer. No encoding check is made; see
    /// [`crate::fraud::verify_extended_square`].
    pub fn from_cells(k: usize, cells: Vec<Fp>) -> Result<ExtendedSquare, DaError> {
        validate_k(k)?;
        if cells.len() != 4 * k * k {
            return Err(DaError::HeaderInconsistent("cell count is not (2k)²"));
        }
        Ok(ExtendedSquare { k, cells })
    }

    /// Side of the data square.
    pub fn k(&self) -> usize {
        self.k
    }

    /// Side of the extended square (`2k`).
    pub fn width(&self) -> usize {
        2 * self.k
    }

    /// All cells, row-major.
    pub fn cells(&self) -> &[Fp] {
        &self.cells
    }

    /// The symbol at `(row, column)`.
    ///
    /// # Panics
    /// Panics if the position is outside the square.
    pub fn get(&self, row: usize, column: usize) -> Fp {
        self.cells[row * self.width() + column]
    }

    /// Replaces one symbol (for modeling a malicious producer in tests and
    /// simulations).
    ///
    /// # Panics
    /// Panics if the position is outside the square.
    pub fn set(&mut self, row: usize, column: usize, value: Fp) {
        let width = self.width();
        self.cells[row * width + column] = value;
    }

    /// One row.
    pub fn row(&self, row: usize) -> Vec<Fp> {
        let width = self.width();
        self.cells[row * width..(row + 1) * width].to_vec()
    }

    /// One column.
    pub fn column(&self, column: usize) -> Vec<Fp> {
        let width = self.width();
        (0..width).map(|r| self.cells[r * width + column]).collect()
    }

    /// One axis.
    pub fn axis(&self, axis: Axis, index: usize) -> Vec<Fp> {
        match axis {
            Axis::Row => self.row(index),
            Axis::Column => self.column(index),
        }
    }

    /// The top-left `k × k` data quadrant, row-major.
    pub fn data_symbols(&self) -> Vec<Fp> {
        let width = self.width();
        let mut out = Vec::with_capacity(self.k * self.k);
        for r in 0..self.k {
            out.extend_from_slice(&self.cells[r * width..r * width + self.k]);
        }
        out
    }

    /// Decodes the blob from the data quadrant.
    pub fn decode_blob(&self) -> Result<Vec<u8>, DaError> {
        square_to_blob(&self.data_symbols())
    }

    fn axis_leaves(&self, axis: Axis, index: usize) -> Vec<Hash32> {
        self.axis(axis, index).into_iter().map(share_leaf).collect()
    }

    /// Computes the commitment header.
    pub fn header(&self) -> DaHeader {
        let width = self.width();
        let leaves: Vec<Hash32> = self.cells.iter().map(|value| share_leaf(*value)).collect();
        let row_roots: Vec<Hash32> = (0..width)
            .map(|r| root(&leaves[r * width..(r + 1) * width]))
            .collect();
        let mut column_leaves = vec![Hash32::ZERO; width];
        let col_roots: Vec<Hash32> = (0..width)
            .map(|c| {
                for (r, slot) in column_leaves.iter_mut().enumerate() {
                    *slot = leaves[r * width + c];
                }
                root(&column_leaves)
            })
            .collect();
        DaHeader::new(self.k as u32, row_roots, col_roots)
    }

    fn check_position(&self, row: usize, column: usize) -> Result<(), DaError> {
        let width = self.width();
        if row >= width || column >= width {
            return Err(DaError::OutOfRange { row, column, width });
        }
        Ok(())
    }

    /// The share at `(row, column)` with a proof to its row root.
    pub fn share(&self, row: usize, column: usize) -> Result<Share, DaError> {
        self.axis_share(Axis::Row, row, column)
    }

    /// The share at `(row, column)` with a proof to its column root.
    pub fn column_share(&self, row: usize, column: usize) -> Result<Share, DaError> {
        self.axis_share(Axis::Column, row, column)
    }

    /// The share at `(row, column)` with a proof to the root of `axis`.
    pub fn axis_share(&self, axis: Axis, row: usize, column: usize) -> Result<Share, DaError> {
        self.check_position(row, column)?;
        let (index, position) = match axis {
            Axis::Row => (row, column),
            Axis::Column => (column, row),
        };
        let leaves = self.axis_leaves(axis, index);
        let proof = inclusion_proof(&leaves, position).map_err(|_| DaError::OutOfRange {
            row,
            column,
            width: self.width(),
        })?;
        Ok(Share {
            value: self.get(row, column),
            proof,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rs::is_codeword;

    #[test]
    fn every_axis_is_a_codeword() {
        for len in [0usize, 1, 50, 300, 2000] {
            let blob: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
            let square = ExtendedSquare::encode(&blob).unwrap();
            for i in 0..square.width() {
                assert!(is_codeword(&square.row(i)), "row {i} of blob {len}");
                assert!(is_codeword(&square.column(i)), "column {i} of blob {len}");
            }
            assert_eq!(square.decode_blob().unwrap(), blob);
        }
    }

    #[test]
    fn header_is_self_consistent_and_shares_verify() {
        let blob = b"the fold makes withholding detectable".to_vec();
        let square = ExtendedSquare::encode_with_k(&blob, 4).unwrap();
        let header = square.header();
        header.verify_self_consistent().unwrap();
        for r in 0..square.width() {
            for c in 0..square.width() {
                let share = square.share(r, c).unwrap();
                header.verify_share(r, c, &share).unwrap();
                let column_share = square.column_share(r, c).unwrap();
                header.verify_column_share(r, c, &column_share).unwrap();
                // A row proof does not verify as a column proof elsewhere.
                if r != c {
                    assert!(
                        header.verify_share(c, r, &share).is_err()
                            || square.get(c, r) == share.value
                    );
                }
            }
        }
    }

    #[test]
    fn tampered_header_is_inconsistent() {
        let square = ExtendedSquare::encode(b"x").unwrap();
        let mut header = square.header();
        header.row_roots[1] = Hash32::ZERO;
        assert!(header.verify_self_consistent().is_err());
        let mut header = square.header();
        header.k = 4;
        assert!(header.verify_self_consistent().is_err());
        let mut header = square.header();
        header.col_roots.pop();
        assert!(header.verify_self_consistent().is_err());
    }
}
