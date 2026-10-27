//! Optimality certificates and their linear-time verifier.
//!
//! A certificate is a primal solution `x` and a dual solution `y`. By weak
//! duality, every feasible `x'` satisfies `cᵀx' ≤ bᵀy` for any dual-feasible
//! `y`. So if `x` is feasible, `y` is dual feasible and `cᵀx ≥ bᵀy − ε`, then
//! no feasible allocation beats `x` by more than `ε`. The verifier checks
//! exactly these five conditions and nothing else; it never solves anything,
//! which is why a ledger can afford to run it on every submission.

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::program::{LinearProgram, LpError};
use crate::rational::Q;

/// A primal-dual pair claimed to be optimal.
#[derive(
    Clone, Debug, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize,
)]
pub struct Certificate {
    /// Primal values, one per variable.
    pub x: Vec<Q>,
    /// Dual values, one per row. For market programs these are the
    /// clearing prices and surpluses.
    pub y: Vec<Q>,
}

/// What a successful verification established.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CertificateReport {
    /// `cᵀx`.
    pub primal_objective: Q,
    /// `bᵀy`, an upper bound on every feasible objective.
    pub dual_objective: Q,
    /// `bᵀy − cᵀx ≥ 0`: how far `x` could be from optimal at most.
    pub gap: Q,
}

/// Why a certificate was rejected.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum CertError {
    /// The program itself is malformed.
    #[error("malformed program: {0}")]
    Malformed(#[from] LpError),
    /// The tolerance is negative.
    #[error("tolerance must be non-negative")]
    NegativeTolerance,
    /// `x` has the wrong length.
    #[error("certificate has {got} primal values for {expected} variables")]
    PrimalDimension {
        /// Number of variables.
        expected: usize,
        /// Number of values supplied.
        got: usize,
    },
    /// `y` has the wrong length.
    #[error("certificate has {got} dual values for {expected} rows")]
    DualDimension {
        /// Number of rows.
        expected: usize,
        /// Number of values supplied.
        got: usize,
    },
    /// `x ≥ 0` fails.
    #[error("primal value x[{var}] = {value} is negative")]
    NegativePrimal {
        /// The variable.
        var: usize,
        /// Its value.
        value: Box<Q>,
    },
    /// `y ≥ 0` fails.
    #[error("dual value y[{row}] = {value} is negative")]
    NegativeDual {
        /// The row.
        row: usize,
        /// Its value.
        value: Box<Q>,
    },
    /// `Ax ≤ b` fails.
    #[error("row {row} is violated: activity {activity} exceeds bound {bound}")]
    PrimalInfeasible {
        /// The row.
        row: usize,
        /// `A[row] · x`.
        activity: Box<Q>,
        /// `b[row]`.
        bound: Box<Q>,
    },
    /// `Aᵀy ≥ c` fails.
    #[error("dual constraint for variable {var} is violated: {activity} < {cost}")]
    DualInfeasible {
        /// The variable.
        var: usize,
        /// `(Aᵀy)[var]`.
        activity: Box<Q>,
        /// `c[var]`.
        cost: Box<Q>,
    },
    /// `cᵀx ≥ bᵀy − ε` fails.
    #[error("duality gap {gap} exceeds tolerance {epsilon}")]
    GapTooLarge {
        /// `bᵀy − cᵀx`.
        gap: Box<Q>,
        /// The tolerance.
        epsilon: Box<Q>,
    },
}

/// Verifies a certificate in time linear in the number of nonzeros.
///
/// The checks, in order: dimensions, `x ≥ 0`, `y ≥ 0`, `Ax ≤ b`, `Aᵀy ≥ c`,
/// and `cᵀx ≥ bᵀy − ε`.
pub fn verify_certificate(
    lp: &LinearProgram,
    certificate: &Certificate,
    epsilon: &Q,
) -> Result<CertificateReport, CertError> {
    lp.validate()?;
    if epsilon.is_negative() {
        return Err(CertError::NegativeTolerance);
    }
    let num_vars = lp.num_vars();
    let num_rows = lp.num_rows();
    if certificate.x.len() != num_vars {
        return Err(CertError::PrimalDimension {
            expected: num_vars,
            got: certificate.x.len(),
        });
    }
    if certificate.y.len() != num_rows {
        return Err(CertError::DualDimension {
            expected: num_rows,
            got: certificate.y.len(),
        });
    }
    for (var, value) in certificate.x.iter().enumerate() {
        if value.is_negative() {
            return Err(CertError::NegativePrimal {
                var,
                value: Box::new(value.clone()),
            });
        }
    }
    for (row, value) in certificate.y.iter().enumerate() {
        if value.is_negative() {
            return Err(CertError::NegativeDual {
                row,
                value: Box::new(value.clone()),
            });
        }
    }

    // Primal feasibility and the dual objective in one pass over the rows,
    // accumulating Aᵀy column by column at the same time.
    let mut dual_activity = vec![Q::zero(); num_vars];
    let mut dual_objective = Q::zero();
    for (index, (row, dual)) in lp.rows.iter().zip(&certificate.y).enumerate() {
        let mut activity = Q::zero();
        for (var, coefficient) in &row.coefficients {
            let var = *var as usize;
            activity += coefficient * &certificate.x[var];
            if !dual.is_zero() {
                dual_activity[var] += coefficient * dual;
            }
        }
        if activity > row.rhs {
            return Err(CertError::PrimalInfeasible {
                row: index,
                activity: Box::new(activity),
                bound: Box::new(row.rhs.clone()),
            });
        }
        dual_objective += &row.rhs * dual;
    }
    for (var, (activity, cost)) in dual_activity.into_iter().zip(&lp.objective).enumerate() {
        if activity < *cost {
            return Err(CertError::DualInfeasible {
                var,
                activity: Box::new(activity),
                cost: Box::new(cost.clone()),
            });
        }
    }

    let primal_objective = lp.objective_value(&certificate.x);
    let gap = &dual_objective - &primal_objective;
    if gap > *epsilon {
        return Err(CertError::GapTooLarge {
            gap: Box::new(gap),
            epsilon: Box::new(epsilon.clone()),
        });
    }
    Ok(CertificateReport {
        primal_objective,
        dual_objective,
        gap,
    })
}

/// Whether every value is an integer.
pub fn is_integral(values: &[Q]) -> bool {
    values.iter().all(Q::is_integer)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// max 3x + 2y s.t. x + y ≤ 4, x ≤ 3 → x = 3, y = 1, value 11; duals (2, 1).
    fn small() -> LinearProgram {
        let mut lp = LinearProgram::new(2);
        lp.set_objective(0, Q::from(3u64));
        lp.set_objective(1, Q::from(2u64));
        lp.add_le(&[(0, Q::one()), (1, Q::one())], Q::from(4u64));
        lp.add_le(&[(0, Q::one())], Q::from(3u64));
        lp
    }

    fn q(values: &[i64]) -> Vec<Q> {
        values.iter().map(|v| Q::from(*v)).collect()
    }

    #[test]
    fn accepts_an_optimal_pair() {
        let cert = Certificate {
            x: q(&[3, 1]),
            y: q(&[2, 1]),
        };
        let report = verify_certificate(&small(), &cert, &Q::zero()).unwrap();
        assert_eq!(report.primal_objective, Q::from(11u64));
        assert_eq!(report.dual_objective, Q::from(11u64));
        assert!(report.gap.is_zero());
    }

    #[test]
    fn rejects_each_kind_of_failure() {
        let lp = small();
        let check = |x: &[i64], y: &[i64]| {
            verify_certificate(&lp, &Certificate { x: q(x), y: q(y) }, &Q::zero())
        };
        assert!(matches!(
            check(&[3], &[2, 1]),
            Err(CertError::PrimalDimension { .. })
        ));
        assert!(matches!(
            check(&[3, 1], &[2]),
            Err(CertError::DualDimension { .. })
        ));
        assert!(matches!(
            check(&[-1, 1], &[2, 1]),
            Err(CertError::NegativePrimal { var: 0, .. })
        ));
        assert!(matches!(
            check(&[3, 1], &[2, -1]),
            Err(CertError::NegativeDual { row: 1, .. })
        ));
        assert!(matches!(
            check(&[4, 1], &[2, 1]),
            Err(CertError::PrimalInfeasible { row: 0, .. })
        ));
        assert!(matches!(
            check(&[3, 1], &[1, 2]),
            Err(CertError::DualInfeasible { var: 1, .. })
        ));
        // Suboptimal x with a valid dual: the gap gives it away.
        assert!(matches!(
            check(&[2, 2], &[2, 1]),
            Err(CertError::GapTooLarge { .. })
        ));
        // A tolerance can admit it.
        let cert = Certificate {
            x: q(&[2, 2]),
            y: q(&[2, 1]),
        };
        let report = verify_certificate(&lp, &cert, &Q::one()).unwrap();
        assert_eq!(report.gap, Q::one());
        assert!(matches!(
            verify_certificate(&lp, &cert, &Q::from(-1i64)),
            Err(CertError::NegativeTolerance)
        ));
    }

    #[test]
    fn integrality() {
        assert!(is_integral(&q(&[1, 2, 0])));
        assert!(!is_integral(&[Q::new(1, 2)]));
    }
}
