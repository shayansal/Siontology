//! Exact linear programming with verifiable optimality certificates.
//!
//! This crate is the arithmetic core of **Verifiable Clearing**: a market
//! clears by solving a linear program, and the solver must hand the ledger a
//! certificate that no better allocation exists. The ledger never re-solves;
//! it runs [`verify_certificate`], which checks five conditions in time
//! linear in the number of nonzero coefficients:
//!
//! ```text
//! Ax* ≤ b,   x* ≥ 0,   Aᵀy* ≥ c,   y* ≥ 0,   cᵀx* ≥ bᵀy* − ε
//! ```
//!
//! By weak duality no feasible allocation scores above `bᵀy*`, so `x*` is
//! within `ε` of optimal; with `ε = 0` it is exactly optimal. For market
//! programs the duals `y*` are the clearing prices.
//!
//! Everything is exact: values are [`Q`], reduced rationals with a single
//! canonical Borsh encoding, so certificates can be hashed, signed and
//! carried in transactions. There is no floating point anywhere in the
//! verification path.
//!
//! [`solve`] is an exact two-phase simplex with Bland's rule. It is a
//! reference solver, not part of the trust base: whatever it (or any other
//! solver) returns is accepted only if the certificate verifies.
//!
//! ```
//! use sion_lp::{solve, verify_certificate, LinearProgram, Q};
//!
//! // maximize 3x + 2y subject to x + y ≤ 4, x ≤ 3, x, y ≥ 0
//! let mut lp = LinearProgram::new(2);
//! lp.set_objective(0, Q::from(3u64));
//! lp.set_objective(1, Q::from(2u64));
//! lp.add_le(&[(0, Q::one()), (1, Q::one())], Q::from(4u64));
//! lp.add_le(&[(0, Q::one())], Q::from(3u64));
//!
//! let solution = solve(&lp).unwrap().optimal().unwrap();
//! assert_eq!(solution.objective, Q::from(11u64));
//! let report = verify_certificate(&lp, &solution.certificate(), &Q::zero()).unwrap();
//! assert!(report.gap.is_zero());
//! ```

pub mod certificate;
pub mod program;
pub mod rational;
pub mod simplex;

pub use certificate::{is_integral, verify_certificate, CertError, Certificate, CertificateReport};
pub use program::{LinearProgram, LpError, Row};
pub use rational::{ParseQError, Q};
pub use simplex::{solve, LpOutcome, LpSolution};
