//! Verifiable Clearing: markets that prove they are optimal.
//!
//! Contended markets clear in sealed batches. Any solver may submit an
//! allocation, but it must come with a certificate — a dual solution — that
//! no better allocation exists. The ledger checks the certificate in linear
//! time ([`sion_lp::verify_certificate`]) instead of trusting a matching
//! engine, keeps the best certified submission, and settles everyone at
//! certified prices. A position inside the batch is worth nothing, so there
//! is nothing to front-run, and the platform's rent falls toward the cost of
//! computation.
//!
//! # Markets
//!
//! - [`auction`]: single-good double auctions priced in a numeraire (for
//!   example, GPU-hours in USD). Exact certificate (zero duality gap),
//!   integral fills, one uniform price per batch chosen canonically from the
//!   supporting price band.
//! - [`assignment`]: matching jobs to slots. The program is totally
//!   unimodular, so the certified optimum is a 0/1 matching; slot duals are a
//!   certified competitive-equilibrium price vector.
//!
//! # Canonical outcomes
//!
//! Ties are broken by beacon priorities ([`priority`]): each order's
//! objective coefficient gets a tiny perturbation derived from a hash of the
//! post-seal beacon and the order's ID. The perturbation is small enough
//! never to overturn a real improvement, but it makes the optimum unique, so
//! a solver cannot choose which of two equal bidders wins. The certificate
//! is for the perturbed program; welfare and prices are reported at the
//! original limits.
//!
//! # What a certificate does not prove
//!
//! Optimality is for the orders as submitted. Bidders can still misreport,
//! and uniform-price batch auctions are not fully strategy-proof.
//!
//! # Example
//!
//! ```
//! use sion_clearing::auction::{clear, settlement, verify_clearing, Batch, Order, Side};
//! use sion_core::{hash_domain, AccountId, Amount, DomainId, Pulse};
//!
//! let order = |tag: &str, side, quantity, usd_cents: u128| Order {
//!     id: hash_domain("example/order", tag.as_bytes()),
//!     owner: AccountId::derive(tag.as_bytes()),
//!     side,
//!     quantity,
//!     limit_price: Amount(usd_cents * 10_000),
//! };
//! // 8 GPU-hours before 18:00 at no more than 2.10 USD per hour.
//! let batch = Batch {
//!     domain: DomainId::derive(b"gpu-hours-before-18:00"),
//!     pulse: Pulse(42),
//!     beacon: hash_domain("example/beacon", b"pulse-42"),
//!     orders: vec![
//!         order("agent", Side::Buy, 8, 210),
//!         order("lab-a", Side::Sell, 5, 180),
//!         order("lab-b", Side::Sell, 6, 200),
//!     ],
//! };
//! let result = clear(&batch).unwrap();
//! assert_eq!(result.volume, 8);
//! assert_eq!(result.price, Some(Amount(2_000_000))); // 2.00 USD per GPU-hour
//! verify_clearing(&batch, &result).unwrap();
//! assert_eq!(settlement(&batch, &result).unwrap().len(), 2);
//! ```

pub mod assignment;
pub mod auction;
pub mod error;
pub mod priority;
pub mod select;

pub use assignment::{
    settle_assignment, solve_assignment, verify_assignment, Assignment, AssignmentBatch,
    AssignmentResult, Bid, Job, Slot,
};
pub use auction::{
    clear, settlement, verify_clearing, Batch, ClearingResult, Fill, Order, PriceBand, Side,
    Transfer, VerifiedClearing,
};
pub use error::ClearingError;
pub use select::{select_best, select_best_assignment, select_best_clearing};
