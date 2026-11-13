//! Exact two-phase simplex with Bland's rule.
//!
//! The solver is not part of the trust base: anything it returns is checked
//! by [`crate::verify_certificate`]. It exists so that a reference solver is
//! available to anyone, and so the tests can cross-check specialised market
//! solvers against a general one.
//!
//! Method: add a slack per row; rows with a negative bound are negated and
//! given an artificial variable. Phase I maximizes the negated sum of the
//! artificials; a negative optimum means the program is infeasible.
//! Artificials left in the basis at zero are pivoted out (always possible,
//! because `[A | I]` has full row rank), their columns dropped, and Phase II
//! optimizes `cᵀx`. Bland's rule (smallest entering index, smallest leaving
//! basic index among ties) guarantees termination on degenerate programs.
//!
//! Duals: at an optimal basis the objective-row entry under slack `i` is
//! `(c_Bᵀ B⁻¹)ᵢ`, the shadow price of row `i`. Negating a row for Phase I
//! does not change it, because the negation cancels between `B` and the
//! slack's column.

use num_rational::BigRational;
use num_traits::{Signed, Zero};
use serde::{Deserialize, Serialize};

use borsh::{BorshDeserialize, BorshSerialize};

use crate::certificate::Certificate;
use crate::program::{LinearProgram, LpError};
use crate::rational::Q;

/// An optimal solution with its dual.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct LpSolution {
    /// Optimal primal values.
    pub x: Vec<Q>,
    /// Optimal dual values (one per row).
    pub y: Vec<Q>,
    /// The optimal objective `cᵀx = bᵀy`.
    pub objective: Q,
}

impl LpSolution {
    /// The solution as a certificate.
    pub fn certificate(&self) -> Certificate {
        Certificate {
            x: self.x.clone(),
            y: self.y.clone(),
        }
    }
}

/// The result of solving a program.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LpOutcome {
    /// An optimal solution exists.
    Optimal(LpSolution),
    /// No `x ≥ 0` satisfies `Ax ≤ b`.
    Infeasible,
    /// The objective is unbounded above.
    Unbounded,
}

impl LpOutcome {
    /// The optimal solution, if any.
    pub fn optimal(self) -> Option<LpSolution> {
        match self {
            LpOutcome::Optimal(solution) => Some(solution),
            LpOutcome::Infeasible | LpOutcome::Unbounded => None,
        }
    }
}

struct Tableau {
    /// Constraint rows; the last entry of each is the right-hand side.
    rows: Vec<Vec<BigRational>>,
    /// Objective row `z + Σ d_j x_j = v`: reduced costs, then the value `v`.
    obj: Vec<BigRational>,
    /// The basic column of each row.
    basis: Vec<usize>,
}

impl Tableau {
    fn rhs_index(&self) -> usize {
        self.obj.len() - 1
    }

    fn pivot(&mut self, r: usize, c: usize) {
        let inverse = self.rows[r][c].recip();
        for value in self.rows[r].iter_mut() {
            if !value.is_zero() {
                *value *= &inverse;
            }
        }
        let pivot_row = self.rows[r].clone();
        let nonzero: Vec<usize> = pivot_row
            .iter()
            .enumerate()
            .filter(|(_, value)| !value.is_zero())
            .map(|(index, _)| index)
            .collect();
        for (i, row) in self.rows.iter_mut().enumerate() {
            if i == r || row[c].is_zero() {
                continue;
            }
            let factor = row[c].clone();
            for &k in &nonzero {
                let delta = &factor * &pivot_row[k];
                row[k] -= delta;
            }
        }
        if !self.obj[c].is_zero() {
            let factor = self.obj[c].clone();
            for &k in &nonzero {
                let delta = &factor * &pivot_row[k];
                self.obj[k] -= delta;
            }
        }
        self.basis[r] = c;
    }

    /// Runs simplex iterations; columns `>= allowed` never enter.
    /// Returns `false` if the objective is unbounded.
    fn optimize(&mut self, allowed: usize) -> bool {
        let rhs = self.rhs_index();
        loop {
            let Some(entering) = (0..allowed).find(|&j| self.obj[j].is_negative()) else {
                return true;
            };
            let mut leaving: Option<(usize, BigRational)> = None;
            for (i, row) in self.rows.iter().enumerate() {
                let entry = &row[entering];
                if !entry.is_positive() {
                    continue;
                }
                let ratio = &row[rhs] / entry;
                let better = match &leaving {
                    None => true,
                    Some((best_row, best_ratio)) => {
                        ratio < *best_ratio
                            || (ratio == *best_ratio && self.basis[i] < self.basis[*best_row])
                    }
                };
                if better {
                    leaving = Some((i, ratio));
                }
            }
            match leaving {
                Some((row, _)) => self.pivot(row, entering),
                None => return false,
            }
        }
    }
}

/// Solves `maximize cᵀx subject to Ax ≤ b, x ≥ 0` exactly.
///
/// Returns an error only for malformed programs (variable indices out of
/// range). Every optimal outcome satisfies [`crate::verify_certificate`] with
/// zero tolerance.
pub fn solve(lp: &LinearProgram) -> Result<LpOutcome, LpError> {
    lp.validate()?;
    let n = lp.num_vars();
    let m = lp.num_rows();

    let negated: Vec<bool> = lp.rows.iter().map(|row| row.rhs.is_negative()).collect();
    let artificial_count = negated.iter().filter(|negative| **negative).count();
    let structural = n + m;
    let width = structural + artificial_count + 1;

    let mut rows = Vec::with_capacity(m);
    let mut basis = Vec::with_capacity(m);
    let mut next_artificial = structural;
    for (i, row) in lp.rows.iter().enumerate() {
        let mut dense = vec![BigRational::zero(); width];
        for (var, coefficient) in &row.coefficients {
            dense[*var as usize] += coefficient.as_ratio();
        }
        dense[n + i] = BigRational::from_integer(1.into());
        dense[width - 1] = row.rhs.as_ratio().clone();
        if negated[i] {
            for value in dense.iter_mut() {
                *value = -value.clone();
            }
            dense[next_artificial] = BigRational::from_integer(1.into());
            basis.push(next_artificial);
            next_artificial += 1;
        } else {
            basis.push(n + i);
        }
        rows.push(dense);
    }

    let mut tableau = Tableau {
        rows,
        obj: vec![BigRational::zero(); width],
        basis,
    };

    if artificial_count > 0 {
        // Phase I: maximize −Σ artificials, i.e. objective row +1 on each
        // artificial column, made canonical for the starting basis.
        for entry in tableau
            .obj
            .iter_mut()
            .skip(structural)
            .take(artificial_count)
        {
            *entry = BigRational::from_integer(1.into());
        }
        for i in 0..m {
            if tableau.basis[i] >= structural {
                let row = tableau.rows[i].clone();
                for (k, value) in row.iter().enumerate() {
                    if !value.is_zero() {
                        tableau.obj[k] -= value;
                    }
                }
            }
        }
        let bounded = tableau.optimize(width - 1);
        debug_assert!(bounded, "phase I is bounded by zero");
        if tableau.obj[width - 1].is_negative() {
            return Ok(LpOutcome::Infeasible);
        }
        // Pivot any zero-level artificials out of the basis.
        let mut i = 0;
        while i < tableau.rows.len() {
            if tableau.basis[i] >= structural {
                match (0..structural).find(|&j| !tableau.rows[i][j].is_zero()) {
                    Some(j) => {
                        tableau.pivot(i, j);
                        i += 1;
                    }
                    None => {
                        // Unreachable for [A | I] (full row rank); drop the
                        // redundant row defensively.
                        tableau.rows.remove(i);
                        tableau.basis.remove(i);
                    }
                }
            } else {
                i += 1;
            }
        }
        // Drop the artificial columns.
        for row in tableau.rows.iter_mut() {
            let rhs = row[width - 1].clone();
            row.truncate(structural);
            row.push(rhs);
        }
    }

    // Phase II: objective row −c on structural columns, made canonical.
    let rhs = structural;
    tableau.obj = vec![BigRational::zero(); structural + 1];
    for (var, cost) in lp.objective.iter().enumerate() {
        tableau.obj[var] = -cost.as_ratio().clone();
    }
    for i in 0..tableau.rows.len() {
        let column = tableau.basis[i];
        if tableau.obj[column].is_zero() {
            continue;
        }
        let factor = tableau.obj[column].clone();
        let row = tableau.rows[i].clone();
        for (k, value) in row.iter().enumerate() {
            if !value.is_zero() {
                tableau.obj[k] -= &factor * value;
            }
        }
    }
    if !tableau.optimize(structural) {
        return Ok(LpOutcome::Unbounded);
    }

    let mut x = vec![Q::zero(); n];
    for (i, column) in tableau.basis.iter().enumerate() {
        if *column < n {
            x[*column] = Q::from_ratio(tableau.rows[i][rhs].clone());
        }
    }
    let y: Vec<Q> = (0..m)
        .map(|i| Q::from_ratio(tableau.obj[n + i].clone()))
        .collect();
    let objective = Q::from_ratio(tableau.obj[rhs].clone());
    let solution = LpSolution { x, y, objective };
    debug_assert!(
        crate::verify_certificate(lp, &solution.certificate(), &Q::zero()).is_ok(),
        "simplex produced an unverifiable certificate"
    );
    Ok(LpOutcome::Optimal(solution))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::verify_certificate;

    fn q(v: i64) -> Q {
        Q::from(v)
    }

    fn check_optimal(lp: &LinearProgram, expected: Q) -> LpSolution {
        let solution = solve(lp).unwrap().optimal().expect("optimal");
        assert_eq!(solution.objective, expected);
        let report = verify_certificate(lp, &solution.certificate(), &Q::zero()).unwrap();
        assert!(report.gap.is_zero());
        solution
    }

    #[test]
    fn textbook_program() {
        // max 3x + 5y s.t. x ≤ 4, 2y ≤ 12, 3x + 2y ≤ 18 → (2, 6), 36.
        let mut lp = LinearProgram::new(2);
        lp.set_objective(0, q(3));
        lp.set_objective(1, q(5));
        lp.add_le(&[(0, q(1))], q(4));
        lp.add_le(&[(1, q(2))], q(12));
        lp.add_le(&[(0, q(3)), (1, q(2))], q(18));
        let solution = check_optimal(&lp, q(36));
        assert_eq!(solution.x, vec![q(2), q(6)]);
        assert_eq!(solution.y, vec![q(0), Q::new(3, 2), q(1)]);
    }

    #[test]
    fn negative_bounds_need_phase_one() {
        // max −x − y s.t. x + y ≥ 2 (−x − y ≤ −2), x ≤ 3 → value −2.
        let mut lp = LinearProgram::new(2);
        lp.set_objective(0, q(-1));
        lp.set_objective(1, q(-1));
        lp.add_le(&[(0, q(-1)), (1, q(-1))], q(-2));
        lp.add_le(&[(0, q(1))], q(3));
        check_optimal(&lp, q(-2));
    }

    #[test]
    fn infeasible_and_unbounded() {
        let mut infeasible = LinearProgram::new(1);
        infeasible.add_le(&[(0, q(-1))], q(-2)); // x ≥ 2
        infeasible.add_le(&[(0, q(1))], q(1)); // x ≤ 1
        assert_eq!(solve(&infeasible).unwrap(), LpOutcome::Infeasible);

        let mut unbounded = LinearProgram::new(2);
        unbounded.set_objective(0, q(1));
        unbounded.add_le(&[(1, q(1))], q(1));
        assert_eq!(solve(&unbounded).unwrap(), LpOutcome::Unbounded);

        let mut no_rows = LinearProgram::new(1);
        no_rows.set_objective(0, q(-1));
        check_optimal(&no_rows, q(0));
    }

    #[test]
    fn beale_cycling_example_terminates() {
        // Beale (1955): cycles under Dantzig's rule; Bland's rule terminates.
        // max 3/4 x1 − 20 x2 + 1/2 x3 − 6 x4
        //   1/4 x1 − 8 x2 − x3 + 9 x4 ≤ 0
        //   1/2 x1 − 12 x2 − 1/2 x3 + 3 x4 ≤ 0
        //   x3 ≤ 1
        let mut lp = LinearProgram::new(4);
        lp.set_objective(0, Q::new(3, 4));
        lp.set_objective(1, q(-20));
        lp.set_objective(2, Q::new(1, 2));
        lp.set_objective(3, q(-6));
        lp.add_le(
            &[(0, Q::new(1, 4)), (1, q(-8)), (2, q(-1)), (3, q(9))],
            q(0),
        );
        lp.add_le(
            &[
                (0, Q::new(1, 2)),
                (1, q(-12)),
                (2, Q::new(-1, 2)),
                (3, q(3)),
            ],
            q(0),
        );
        lp.add_le(&[(2, q(1))], q(1));
        check_optimal(&lp, Q::new(5, 4));
    }

    #[test]
    fn zero_rows_with_negative_rhs_and_no_variables() {
        let mut lp = LinearProgram::new(0);
        lp.add_le(&[], q(-1));
        assert_eq!(solve(&lp).unwrap(), LpOutcome::Infeasible);
        let mut ok = LinearProgram::new(0);
        ok.add_le(&[], q(0));
        check_optimal(&ok, q(0));
    }

    #[test]
    fn repeated_indices_are_summed() {
        // x + x ≤ 4 means x ≤ 2.
        let mut lp = LinearProgram::new(1);
        lp.set_objective(0, q(1));
        lp.add_le(&[(0, q(1)), (0, q(1))], q(4));
        let solution = check_optimal(&lp, q(2));
        assert_eq!(solution.x, vec![q(2)]);
    }
}
