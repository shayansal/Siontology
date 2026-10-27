//! Linear programs in the form `maximize cᵀx subject to Ax ≤ b, x ≥ 0`.
//!
//! The constraint matrix is stored row by row in sparse form, so checking a
//! certificate costs time linear in the number of nonzero coefficients.

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::rational::Q;

/// One constraint `Σ coefficient · x[var] ≤ rhs`.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct Row {
    /// Sparse coefficients as `(variable index, coefficient)`. Repeated
    /// indices are summed.
    pub coefficients: Vec<(u32, Q)>,
    /// The right-hand side.
    pub rhs: Q,
}

/// `maximize cᵀx subject to Ax ≤ b, x ≥ 0`.
#[derive(
    Clone, Debug, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize,
)]
pub struct LinearProgram {
    /// The objective vector `c`; its length is the number of variables.
    pub objective: Vec<Q>,
    /// The constraint rows of `A` with their bounds `b`.
    pub rows: Vec<Row>,
}

/// A malformed program.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum LpError {
    /// A row refers to a variable that does not exist.
    #[error("row {row} refers to variable {var}, but the program has {num_vars} variables")]
    VariableOutOfRange {
        /// The offending row.
        row: usize,
        /// The variable index used.
        var: u32,
        /// Number of variables in the program.
        num_vars: usize,
    },
    /// The program has more variables than a `u32` index can address.
    #[error("too many variables: {0}")]
    TooManyVariables(usize),
}

impl LinearProgram {
    /// A program with `num_vars` variables, a zero objective and no rows.
    pub fn new(num_vars: usize) -> Self {
        LinearProgram {
            objective: vec![Q::zero(); num_vars],
            rows: Vec::new(),
        }
    }

    /// Number of variables.
    pub fn num_vars(&self) -> usize {
        self.objective.len()
    }

    /// Number of constraint rows.
    pub fn num_rows(&self) -> usize {
        self.rows.len()
    }

    /// Sets the objective coefficient of `var`.
    pub fn set_objective(&mut self, var: usize, coefficient: Q) {
        self.objective[var] = coefficient;
    }

    /// Adds the constraint `Σ coefficient · x[var] ≤ rhs` and returns its index.
    pub fn add_le(&mut self, terms: &[(usize, Q)], rhs: Q) -> usize {
        let coefficients = terms
            .iter()
            .map(|(var, coefficient)| {
                let index = u32::try_from(*var).expect("variable index fits in u32");
                (index, coefficient.clone())
            })
            .collect();
        self.rows.push(Row { coefficients, rhs });
        self.rows.len() - 1
    }

    /// Checks that every row refers only to existing variables.
    pub fn validate(&self) -> Result<(), LpError> {
        let num_vars = self.num_vars();
        if u32::try_from(num_vars).is_err() {
            return Err(LpError::TooManyVariables(num_vars));
        }
        for (row_index, row) in self.rows.iter().enumerate() {
            for (var, _) in &row.coefficients {
                if *var as usize >= num_vars {
                    return Err(LpError::VariableOutOfRange {
                        row: row_index,
                        var: *var,
                        num_vars,
                    });
                }
            }
        }
        Ok(())
    }

    /// Number of stored nonzero coefficients.
    pub fn nonzeros(&self) -> usize {
        self.rows.iter().map(|row| row.coefficients.len()).sum()
    }

    /// `A[row] · x`.
    pub fn row_activity(&self, row: usize, x: &[Q]) -> Q {
        self.rows[row]
            .coefficients
            .iter()
            .map(|(var, coefficient)| coefficient * &x[*var as usize])
            .sum()
    }

    /// `cᵀx`.
    pub fn objective_value(&self, x: &[Q]) -> Q {
        self.objective
            .iter()
            .zip(x)
            .map(|(coefficient, value)| coefficient * value)
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_and_validation() {
        let mut lp = LinearProgram::new(2);
        lp.set_objective(0, Q::from(3u64));
        lp.add_le(&[(0, Q::one()), (1, Q::one())], Q::from(4u64));
        assert!(lp.validate().is_ok());
        assert_eq!(lp.nonzeros(), 2);
        assert_eq!(
            lp.row_activity(0, &[Q::one(), Q::from(2u64)]),
            Q::from(3u64)
        );
        lp.rows.push(Row {
            coefficients: vec![(5, Q::one())],
            rhs: Q::zero(),
        });
        assert!(matches!(
            lp.validate(),
            Err(LpError::VariableOutOfRange { row: 1, var: 5, .. })
        ));
    }
}
