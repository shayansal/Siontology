//! Property tests: the simplex always returns verifying certificates, agrees
//! with brute-force vertex enumeration on tiny programs, and tampered
//! certificates are rejected.

use proptest::prelude::*;
use sion_lp::{solve, verify_certificate, CertError, Certificate, LinearProgram, LpOutcome, Q};

/// A random program with small integer data. When `boxed`, every variable
/// also gets `x ≤ 7`, so the program is bounded.
fn program_strategy(boxed: bool) -> impl Strategy<Value = LinearProgram> {
    (1usize..=3, 0usize..=4).prop_flat_map(move |(n, m)| {
        (
            proptest::collection::vec(-5i64..=5, n),
            proptest::collection::vec(proptest::collection::vec(-4i64..=4, n), m),
            proptest::collection::vec(-6i64..=10, m),
        )
            .prop_map(move |(c, a, b)| {
                let mut lp = LinearProgram::new(n);
                for (j, cost) in c.iter().enumerate() {
                    lp.set_objective(j, Q::from(*cost));
                }
                for (row, rhs) in a.iter().zip(&b) {
                    let terms: Vec<(usize, Q)> = row
                        .iter()
                        .enumerate()
                        .filter(|(_, v)| **v != 0)
                        .map(|(j, v)| (j, Q::from(*v)))
                        .collect();
                    lp.add_le(&terms, Q::from(*rhs));
                }
                if boxed {
                    for j in 0..n {
                        lp.add_le(&[(j, Q::one())], Q::from(7u64));
                    }
                }
                lp
            })
    })
}

/// All constraints as (dense coefficients, bound), including `−x_j ≤ 0`.
fn all_constraints(lp: &LinearProgram) -> Vec<(Vec<Q>, Q)> {
    let n = lp.num_vars();
    let mut constraints = Vec::new();
    for row in &lp.rows {
        let mut dense = vec![Q::zero(); n];
        for (var, coefficient) in &row.coefficients {
            dense[*var as usize] += coefficient;
        }
        constraints.push((dense, row.rhs.clone()));
    }
    for j in 0..n {
        let mut dense = vec![Q::zero(); n];
        dense[j] = Q::from(-1i64);
        constraints.push((dense, Q::zero()));
    }
    constraints
}

/// Solves a square system exactly; `None` if singular.
fn solve_square(mut a: Vec<Vec<Q>>, mut b: Vec<Q>) -> Option<Vec<Q>> {
    let n = b.len();
    for col in 0..n {
        let pivot = (col..n).find(|&r| !a[r][col].is_zero())?;
        a.swap(col, pivot);
        b.swap(col, pivot);
        let inverse = a[col][col].recip();
        for value in a[col].iter_mut().skip(col) {
            *value = &*value * &inverse;
        }
        b[col] = &b[col] * &inverse;
        let pivot_row = a[col].clone();
        let pivot_rhs = b[col].clone();
        for (r, (row, rhs)) in a.iter_mut().zip(b.iter_mut()).enumerate() {
            if r == col || row[col].is_zero() {
                continue;
            }
            let factor = row[col].clone();
            for (value, pivot_value) in row.iter_mut().zip(&pivot_row).skip(col) {
                let delta = &factor * pivot_value;
                *value = &*value - &delta;
            }
            let delta = &factor * &pivot_rhs;
            *rhs = &*rhs - &delta;
        }
    }
    Some(b)
}

fn combinations(n: usize, k: usize) -> Vec<Vec<usize>> {
    fn rec(start: usize, n: usize, k: usize, current: &mut Vec<usize>, out: &mut Vec<Vec<usize>>) {
        if current.len() == k {
            out.push(current.clone());
            return;
        }
        for i in start..n {
            current.push(i);
            rec(i + 1, n, k, current, out);
            current.pop();
        }
    }
    let mut out = Vec::new();
    rec(0, n, k, &mut Vec::new(), &mut out);
    out
}

/// Best objective over all vertices of a bounded program, or `None` if it
/// has no feasible vertex (hence, being pointed, is infeasible).
fn brute_force(lp: &LinearProgram) -> Option<Q> {
    let n = lp.num_vars();
    let constraints = all_constraints(lp);
    let mut best: Option<Q> = None;
    for subset in combinations(constraints.len(), n) {
        let a: Vec<Vec<Q>> = subset.iter().map(|&i| constraints[i].0.clone()).collect();
        let b: Vec<Q> = subset.iter().map(|&i| constraints[i].1.clone()).collect();
        let Some(x) = solve_square(a, b) else {
            continue;
        };
        let feasible = constraints.iter().all(|(row, bound)| {
            let activity: Q = row.iter().zip(&x).map(|(c, v)| c * v).sum();
            activity <= *bound
        });
        if feasible {
            let value = lp.objective_value(&x);
            best = Some(match best {
                Some(current) => current.max(value),
                None => value,
            });
        }
    }
    best
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(160))]

    #[test]
    fn optimal_outcomes_verify_with_zero_gap(lp in program_strategy(false)) {
        match solve(&lp).unwrap() {
            LpOutcome::Optimal(solution) => {
                let report = verify_certificate(&lp, &solution.certificate(), &Q::zero()).unwrap();
                prop_assert!(report.gap.is_zero());
                prop_assert_eq!(report.primal_objective, solution.objective);
            }
            LpOutcome::Infeasible | LpOutcome::Unbounded => {}
        }
    }

    #[test]
    fn simplex_matches_brute_force_on_bounded_programs(lp in program_strategy(true)) {
        let outcome = solve(&lp).unwrap();
        match (outcome, brute_force(&lp)) {
            (LpOutcome::Optimal(solution), Some(best)) => prop_assert_eq!(solution.objective, best),
            (LpOutcome::Infeasible, None) => {}
            (outcome, best) => prop_assert!(false, "mismatch: {:?} vs {:?}", outcome, best),
        }
    }

    #[test]
    fn tampered_certificates_are_rejected(lp in program_strategy(true), which in 0usize..64) {
        let LpOutcome::Optimal(solution) = solve(&lp).unwrap() else { return Ok(()) };
        let n = solution.x.len();
        let m = solution.y.len();
        // A negative primal value always fails.
        let mut bad = solution.certificate();
        bad.x[which % n] = Q::from(-1i64);
        let rejected = matches!(verify_certificate(&lp, &bad, &Q::zero()), Err(CertError::NegativePrimal { .. }));
        prop_assert!(rejected);
        if m > 0 {
            // A negative dual value always fails.
            let mut bad = solution.certificate();
            bad.y[which % m] = Q::new(-1, 2);
            let rejected = matches!(verify_certificate(&lp, &bad, &Q::zero()), Err(CertError::NegativeDual { .. }));
            prop_assert!(rejected);
            // Raising a dual on a row with a positive bound opens a gap or
            // breaks dual feasibility; either way the certificate fails.
            if let Some(row) = (0..m).find(|&i| lp.rows[i].rhs.is_positive()) {
                let mut bad = solution.certificate();
                bad.y[row] = &bad.y[row] + &Q::one();
                prop_assert!(verify_certificate(&lp, &bad, &Q::zero()).is_err());
            }
        }
        // Wrong dimensions always fail.
        let truncated = Certificate { x: solution.x[..n - 1].to_vec(), y: solution.y.clone() };
        prop_assert!(verify_certificate(&lp, &truncated, &Q::zero()).is_err());
    }
}
