//! Why a clearing or assignment was rejected.

use sion_core::{Amount, Hash32};
use sion_lp::{CertError, Q};
use thiserror::Error;

use crate::auction::PriceBand;

/// Errors from building, solving or verifying a market.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ClearingError {
    /// The batch itself is malformed (for example, duplicate order IDs).
    #[error("invalid batch: {0}")]
    InvalidBatch(String),
    /// The optimality certificate failed one of the five checks.
    #[error("certificate rejected: {0}")]
    Certificate(#[from] CertError),
    /// The claimed fills do not line up with the batch or the certificate.
    #[error("fills do not match: {0}")]
    FillMismatch(String),
    /// An order was filled beyond its quantity.
    #[error("order {order} filled {filled} beyond its quantity {quantity}")]
    Overfilled {
        /// The order.
        order: Hash32,
        /// Claimed fill.
        filled: u64,
        /// The order's quantity.
        quantity: u64,
    },
    /// The certified allocation is fractional.
    #[error("allocation is not integral")]
    NotIntegral,
    /// Buyers received a different volume from what sellers delivered.
    #[error("buy volume {buy} differs from sell volume {sell}")]
    Unbalanced {
        /// Total bought.
        buy: u64,
        /// Total sold.
        sell: u64,
    },
    /// The result's objective is not the certified one.
    #[error("claimed objective {claimed} differs from certified objective {certified}")]
    ObjectiveMismatch {
        /// What the submission claimed.
        claimed: Box<Q>,
        /// What the certificate establishes.
        certified: Box<Q>,
    },
    /// The price band is not the one the allocation implies.
    #[error("price band mismatch: expected {expected:?}, got {got:?}")]
    BandMismatch {
        /// The band implied by the allocation.
        expected: Option<PriceBand>,
        /// The band claimed.
        got: Option<PriceBand>,
    },
    /// No uniform price supports the allocation.
    #[error("no uniform price supports the allocation (band {low} > {high})")]
    EmptyBand {
        /// Lower edge.
        low: Amount,
        /// Upper edge.
        high: Amount,
    },
    /// The price is not the canonical midpoint of the band.
    #[error("non-canonical price: expected {expected:?}, got {got:?}")]
    NonCanonicalPrice {
        /// The canonical price.
        expected: Option<Amount>,
        /// The price claimed.
        got: Option<Amount>,
    },
    /// A filled order would trade outside its limit price.
    #[error("order {order} would trade outside its limit price")]
    LimitViolated {
        /// The order.
        order: Hash32,
    },
    /// The claimed welfare is not the allocation's welfare.
    #[error("welfare mismatch: expected {expected}, got {got}")]
    WelfareMismatch {
        /// Recomputed welfare.
        expected: Amount,
        /// Claimed welfare.
        got: Amount,
    },
    /// The claimed prices are not the certificate's duals.
    #[error("prices do not match the certified duals")]
    PriceMismatch,
    /// Money arithmetic overflowed.
    #[error("arithmetic overflow")]
    Overflow,
    /// The reference solver failed (should not happen for valid batches).
    #[error("solver failure: {0}")]
    Solver(String),
}
