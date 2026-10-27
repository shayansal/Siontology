//! Sealed single-good double auctions that clear with a certificate.
//!
//! A contention domain (say, "GPU-hours deliverable before 18:00, priced in
//! USD") collects encrypted orders during a batch window. When the batch
//! seals and decrypts, any solver computes an allocation and submits it with
//! an optimality certificate; the ledger verifies the certificate in linear
//! time and settles everyone at one uniform price, so a position inside the
//! batch is worth nothing to front-run.
//!
//! The program (variables in batch order, one per order):
//!
//! ```text
//! maximize   Σ_buy c'_k x_k  +  Σ_sell c'_k x_k
//! subject to Σ_buy x_k − Σ_sell x_k ≤ 0          (row 0: nobody buys what was not sold)
//!            x_k ≤ quantity_k                      (row 1 + k)
//!            x ≥ 0
//! c'_k = limit_k + δ·π_k            for buyers
//! c'_k = −(limit_k + δ·(1 − π_k))   for sellers
//! ```
//!
//! `π_k` is the order's beacon priority and `δ` is small enough that the
//! perturbation only ever breaks ties (see [`crate::priority`]). Sellers are
//! perturbed by `1 − π` rather than `−π` so that their coefficient stays
//! strictly negative: a seller can never profit from "selling into the void"
//! under the `≤` balance row, and a higher priority wins on both sides.
//!
//! The constraint matrix is an interval matrix (totally unimodular), so with
//! integer quantities every vertex — in particular the certified optimum —
//! is integral.

use std::cmp::Ordering;

use borsh::{BorshDeserialize, BorshSerialize};
use num_bigint::BigInt;
use num_traits::ToPrimitive;
use serde::{Deserialize, Serialize};
use sion_core::{AccountId, Amount, DomainId, Hash32, Pulse};
use sion_lp::{verify_certificate, Certificate, LinearProgram, Q};

use crate::error::ClearingError;
use crate::priority::{delta, priority};

/// Which side of the market an order is on.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    BorshSerialize,
    BorshDeserialize,
    Serialize,
    Deserialize,
)]
pub enum Side {
    /// Wants to acquire units, paying at most the limit per unit.
    Buy,
    /// Wants to deliver units, receiving at least the limit per unit.
    Sell,
}

/// One sealed intent, revealed after the batch seals.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct Order {
    /// Unique within the batch.
    pub id: Hash32,
    /// Who pays or gets paid.
    pub owner: AccountId,
    /// Buy or sell.
    pub side: Side,
    /// Units wanted or offered.
    pub quantity: u64,
    /// Price per unit in micro-units of the numeraire: a ceiling for buyers,
    /// a floor for sellers.
    pub limit_price: Amount,
}

/// One sealed batch of a contention domain.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct Batch {
    /// The market.
    pub domain: DomainId,
    /// The Pulse the batch clears in.
    pub pulse: Pulse,
    /// Randomness revealed after sealing; drives tie-breaking.
    pub beacon: Hash32,
    /// The decrypted orders, in canonical (submission) order.
    pub orders: Vec<Order>,
}

/// Units allocated to one order.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize,
)]
pub struct Fill {
    /// The order.
    pub order_id: Hash32,
    /// Units bought or sold.
    pub quantity: u64,
}

/// The interval of uniform prices that support an allocation.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize,
)]
pub struct PriceBand {
    /// Lowest supporting price.
    pub low: Amount,
    /// Highest supporting price.
    pub high: Amount,
}

/// A solver's submission: an allocation, its certificate and the price.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct ClearingResult {
    /// One fill per order, in batch order (zero for unfilled orders).
    pub fills: Vec<Fill>,
    /// Supporting price band from the original limits; `None` with no trade.
    pub band: Option<PriceBand>,
    /// Canonical uniform price (band midpoint, rounded down); `None` with no trade.
    pub price: Option<Amount>,
    /// Optimality certificate for the perturbed program.
    pub certificate: Certificate,
    /// The certified (perturbed) objective.
    pub objective: Q,
    /// Gains from trade at the original limits: Σ buy limits − Σ sell limits.
    pub welfare: Amount,
    /// Units traded.
    pub volume: u64,
}

/// A payment instruction produced by settlement.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize,
)]
pub struct Transfer {
    /// Payer.
    pub from: AccountId,
    /// Payee.
    pub to: AccountId,
    /// Amount in micro-units of the numeraire.
    pub amount: Amount,
}

/// What a successful verification established.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedClearing {
    /// Gains from trade.
    pub welfare: Amount,
    /// Units traded.
    pub volume: u64,
    /// The uniform price, if anything traded.
    pub price: Option<Amount>,
    /// The certified objective.
    pub objective: Q,
}

fn validate(batch: &Batch) -> Result<(), ClearingError> {
    let mut ids: Vec<&Hash32> = batch.orders.iter().map(|order| &order.id).collect();
    ids.sort();
    if ids.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(ClearingError::InvalidBatch("duplicate order id".into()));
    }
    Ok(())
}

fn total_units(batch: &Batch) -> u128 {
    batch
        .orders
        .iter()
        .map(|order| u128::from(order.quantity))
        .sum()
}

/// The perturbed objective coefficient of every order, in batch order.
pub fn perturbed_coefficients(batch: &Batch) -> Vec<Q> {
    let scale = delta(total_units(batch));
    batch
        .orders
        .iter()
        .map(|order| {
            let pi = priority(&batch.beacon, &order.id);
            let limit = Q::from(order.limit_price.micros());
            match order.side {
                Side::Buy => limit + &scale * &pi,
                Side::Sell => -(limit + &scale * &(Q::one() - &pi)),
            }
        })
        .collect()
}

/// Builds the batch's clearing program deterministically from the batch
/// alone, so every verifier checks the same program.
pub fn build_program(batch: &Batch) -> Result<LinearProgram, ClearingError> {
    validate(batch)?;
    let n = batch.orders.len();
    let mut program = LinearProgram::new(n);
    for (k, coefficient) in perturbed_coefficients(batch).into_iter().enumerate() {
        program.set_objective(k, coefficient);
    }
    let balance: Vec<(usize, Q)> = batch
        .orders
        .iter()
        .enumerate()
        .map(|(k, order)| {
            let sign = match order.side {
                Side::Buy => Q::one(),
                Side::Sell => Q::from(-1i64),
            };
            (k, sign)
        })
        .collect();
    program.add_le(&balance, Q::zero());
    for (k, order) in batch.orders.iter().enumerate() {
        program.add_le(&[(k, Q::one())], Q::from(order.quantity));
    }
    Ok(program)
}

/// Buyers in canonical priority order (highest perturbed bid first) and
/// sellers likewise (lowest perturbed ask first).
fn canonical_sides(batch: &Batch, coefficients: &[Q]) -> (Vec<usize>, Vec<usize>) {
    let mut buyers: Vec<usize> = (0..batch.orders.len())
        .filter(|&k| batch.orders[k].side == Side::Buy)
        .collect();
    let mut sellers: Vec<usize> = (0..batch.orders.len())
        .filter(|&k| batch.orders[k].side == Side::Sell)
        .collect();
    // Buyer value is c'; seller effective ask is −c'. Both sort descending by
    // c' (higher is better), with the index as a final tiebreak.
    let order_by = |a: &usize, b: &usize| -> Ordering {
        coefficients[*b]
            .cmp(&coefficients[*a])
            .then_with(|| a.cmp(b))
    };
    buyers.sort_by(order_by);
    sellers.sort_by(order_by);
    (buyers, sellers)
}

/// The band of uniform prices supporting `fills` at the original limits:
/// filled buyers must pay at most their limit, unfilled or partially filled
/// buyers must not want more at that price, and symmetrically for sellers.
pub fn price_band(batch: &Batch, fills: &[u64]) -> Option<PriceBand> {
    let volume: u64 = batch
        .orders
        .iter()
        .zip(fills)
        .filter(|(order, _)| order.side == Side::Buy)
        .map(|(_, fill)| *fill)
        .sum();
    if volume == 0 {
        return None;
    }
    let mut low = Amount::ZERO;
    let mut high: Option<Amount> = None;
    for (order, fill) in batch.orders.iter().zip(fills) {
        let limit = order.limit_price;
        let (raises_low, caps_high) = match order.side {
            // A buyer not fully served would buy more at any price below its limit.
            Side::Buy => (*fill < order.quantity, *fill > 0),
            // A seller that delivered must be paid at least its limit.
            Side::Sell => (*fill > 0, *fill < order.quantity),
        };
        if raises_low && limit > low {
            low = limit;
        }
        if caps_high {
            high = Some(match high {
                Some(current) if current <= limit => current,
                _ => limit,
            });
        }
    }
    high.map(|high| PriceBand { low, high })
}

/// The canonical price: the band's midpoint, rounded down to a micro-unit.
pub fn canonical_price(band: &PriceBand) -> Amount {
    Amount(band.low.micros() + (band.high.micros().saturating_sub(band.low.micros())) / 2)
}

fn welfare_of(batch: &Batch, fills: &[u64]) -> Result<Amount, ClearingError> {
    let mut total = BigInt::from(0);
    for (order, fill) in batch.orders.iter().zip(fills) {
        let value = BigInt::from(order.limit_price.micros()) * BigInt::from(*fill);
        match order.side {
            Side::Buy => total += value,
            Side::Sell => total -= value,
        }
    }
    total.to_u128().map(Amount).ok_or(ClearingError::Overflow)
}

/// Clears a batch with a fast exact solver (greedy matching in priority
/// order) and builds the dual certificate analytically.
///
/// Duals: the balance row's dual is a uniform perturbed price
/// `p = max(0, max{c'_i : buyer not fully filled}, max{−c'_j : seller filled})`;
/// buyer bound duals are `max(0, c'_i − p)` and seller bound duals are
/// `max(0, p + c'_j)`. Greedy matching stops exactly when the next buyer
/// values a unit no more than the next seller asks, which makes this `p`
/// support the allocation, so the duality gap is zero.
pub fn clear(batch: &Batch) -> Result<ClearingResult, ClearingError> {
    validate(batch)?;
    let n = batch.orders.len();
    let coefficients = perturbed_coefficients(batch);
    let (buyers, sellers) = canonical_sides(batch, &coefficients);

    let mut fills = vec![0u64; n];
    let (mut bi, mut si) = (0usize, 0usize);
    let mut buyer_left = buyers.first().map(|&k| batch.orders[k].quantity);
    let mut seller_left = sellers.first().map(|&k| batch.orders[k].quantity);
    while bi < buyers.len() && si < sellers.len() {
        let (b, s) = (buyers[bi], sellers[si]);
        let remaining_buy = buyer_left.unwrap_or(0);
        let remaining_sell = seller_left.unwrap_or(0);
        if remaining_buy == 0 {
            bi += 1;
            buyer_left = buyers.get(bi).map(|&k| batch.orders[k].quantity);
            continue;
        }
        if remaining_sell == 0 {
            si += 1;
            seller_left = sellers.get(si).map(|&k| batch.orders[k].quantity);
            continue;
        }
        // Trade iff the buyer's perturbed value exceeds the seller's
        // perturbed ask: c'_b > −c'_s.
        if &coefficients[b] + &coefficients[s] <= Q::zero() {
            break;
        }
        let units = remaining_buy.min(remaining_sell);
        fills[b] += units;
        fills[s] += units;
        buyer_left = Some(remaining_buy - units);
        seller_left = Some(remaining_sell - units);
    }

    // Analytic duals.
    let mut price = Q::zero();
    for (k, order) in batch.orders.iter().enumerate() {
        let candidate = match order.side {
            Side::Buy if fills[k] < order.quantity => Some(coefficients[k].clone()),
            Side::Sell if fills[k] > 0 => Some(-coefficients[k].clone()),
            _ => None,
        };
        if let Some(candidate) = candidate {
            if candidate > price {
                price = candidate;
            }
        }
    }
    let mut y = Vec::with_capacity(n + 1);
    y.push(price.clone());
    for (k, order) in batch.orders.iter().enumerate() {
        let surplus = match order.side {
            Side::Buy => &coefficients[k] - &price,
            Side::Sell => &price + &coefficients[k],
        };
        y.push(surplus.max(Q::zero()));
    }
    let x: Vec<Q> = fills.iter().map(|fill| Q::from(*fill)).collect();
    let objective: Q = coefficients.iter().zip(&x).map(|(c, v)| c * v).sum();

    let band = price_band(batch, &fills);
    let result = ClearingResult {
        fills: batch
            .orders
            .iter()
            .zip(&fills)
            .map(|(order, fill)| Fill {
                order_id: order.id,
                quantity: *fill,
            })
            .collect(),
        band,
        price: band.as_ref().map(canonical_price),
        certificate: Certificate { x, y },
        objective,
        welfare: welfare_of(batch, &fills)?,
        volume: batch
            .orders
            .iter()
            .zip(&fills)
            .filter(|(order, _)| order.side == Side::Buy)
            .map(|(_, fill)| *fill)
            .sum(),
    };
    debug_assert!(
        verify_clearing(batch, &result).is_ok(),
        "greedy clearing produced an unverifiable result: {:?}",
        verify_clearing(batch, &result)
    );
    Ok(result)
}

/// Verifies a submission against the batch, in time linear in the number of
/// orders. Checks, each for a reason:
///
/// 1. the program is rebuilt from the batch alone, so the solver cannot
///    choose which program it certifies;
/// 2. fills line up with orders and with the certificate's `x`, are
///    integral and within quantities;
/// 3. the certificate passes all five checks with zero tolerance, so the
///    allocation is exactly optimal for the perturbed program — which also
///    pins down the tie-break, because the perturbed optimum is unique;
/// 4. buy and sell volumes balance;
/// 5. the band and price follow the canonical rule from the allocation and
///    the original limits, and every filled order trades within its limit;
/// 6. the claimed objective and welfare are the recomputed ones.
pub fn verify_clearing(
    batch: &Batch,
    result: &ClearingResult,
) -> Result<VerifiedClearing, ClearingError> {
    let program = build_program(batch)?;
    let n = batch.orders.len();
    if result.fills.len() != n {
        return Err(ClearingError::FillMismatch(format!(
            "{} fills for {} orders",
            result.fills.len(),
            n
        )));
    }
    if result.certificate.x.len() != n {
        return Err(ClearingError::FillMismatch(
            "certificate dimension differs from the batch".into(),
        ));
    }
    let mut fills = Vec::with_capacity(n);
    for ((order, fill), value) in batch
        .orders
        .iter()
        .zip(&result.fills)
        .zip(&result.certificate.x)
    {
        if fill.order_id != order.id {
            return Err(ClearingError::FillMismatch(format!(
                "fill for {} where order {} was expected",
                fill.order_id, order.id
            )));
        }
        if !value.is_integer() {
            return Err(ClearingError::NotIntegral);
        }
        if *value != Q::from(fill.quantity) {
            return Err(ClearingError::FillMismatch(format!(
                "fill for {} differs from the certificate",
                order.id
            )));
        }
        if fill.quantity > order.quantity {
            return Err(ClearingError::Overfilled {
                order: order.id,
                filled: fill.quantity,
                quantity: order.quantity,
            });
        }
        fills.push(fill.quantity);
    }

    let report = verify_certificate(&program, &result.certificate, &Q::zero())?;
    if report.primal_objective != result.objective {
        return Err(ClearingError::ObjectiveMismatch {
            claimed: Box::new(result.objective.clone()),
            certified: Box::new(report.primal_objective),
        });
    }

    let (mut buy, mut sell) = (0u64, 0u64);
    for (order, fill) in batch.orders.iter().zip(&fills) {
        match order.side {
            Side::Buy => buy = buy.checked_add(*fill).ok_or(ClearingError::Overflow)?,
            Side::Sell => sell = sell.checked_add(*fill).ok_or(ClearingError::Overflow)?,
        }
    }
    if buy != sell {
        return Err(ClearingError::Unbalanced { buy, sell });
    }
    if result.volume != buy {
        return Err(ClearingError::FillMismatch(format!(
            "claimed volume {} but {} units traded",
            result.volume, buy
        )));
    }

    let band = price_band(batch, &fills);
    if band != result.band {
        return Err(ClearingError::BandMismatch {
            expected: band,
            got: result.band,
        });
    }
    if let Some(band) = &band {
        if band.low > band.high {
            return Err(ClearingError::EmptyBand {
                low: band.low,
                high: band.high,
            });
        }
    }
    let price = band.as_ref().map(canonical_price);
    if price != result.price {
        return Err(ClearingError::NonCanonicalPrice {
            expected: price,
            got: result.price,
        });
    }
    if let Some(price) = price {
        for (order, fill) in batch.orders.iter().zip(&fills) {
            let within = match order.side {
                Side::Buy => price <= order.limit_price,
                Side::Sell => price >= order.limit_price,
            };
            if *fill > 0 && !within {
                return Err(ClearingError::LimitViolated { order: order.id });
            }
        }
    }

    let welfare = welfare_of(batch, &fills)?;
    if welfare != result.welfare {
        return Err(ClearingError::WelfareMismatch {
            expected: welfare,
            got: result.welfare,
        });
    }
    Ok(VerifiedClearing {
        welfare,
        volume: buy,
        price,
        objective: report.primal_objective,
    })
}

/// Turns a verified clearing into payments: every buyer pays the uniform
/// price for each unit, every seller receives it. Buyers and sellers are
/// paired unit by unit in canonical priority order, so the instructions are
/// deterministic, and the totals balance exactly.
pub fn settlement(batch: &Batch, result: &ClearingResult) -> Result<Vec<Transfer>, ClearingError> {
    let verified = verify_clearing(batch, result)?;
    let Some(price) = verified.price else {
        return Ok(Vec::new());
    };
    let coefficients = perturbed_coefficients(batch);
    let (buyers, sellers) = canonical_sides(batch, &coefficients);
    let fills: Vec<u64> = result.fills.iter().map(|fill| fill.quantity).collect();
    let mut buyer_iter = buyers.iter().filter(|&&k| fills[k] > 0);
    let mut seller_iter = sellers.iter().filter(|&&k| fills[k] > 0);
    let mut transfers = Vec::new();
    let mut buyer = buyer_iter.next().map(|&k| (k, fills[k]));
    let mut seller = seller_iter.next().map(|&k| (k, fills[k]));
    while let (Some((b, b_left)), Some((s, s_left))) = (buyer, seller) {
        let units = b_left.min(s_left);
        let amount = price
            .micros()
            .checked_mul(u128::from(units))
            .map(Amount)
            .ok_or(ClearingError::Overflow)?;
        transfers.push(Transfer {
            from: batch.orders[b].owner,
            to: batch.orders[s].owner,
            amount,
        });
        buyer = if b_left == units {
            buyer_iter.next().map(|&k| (k, fills[k]))
        } else {
            Some((b, b_left - units))
        };
        seller = if s_left == units {
            seller_iter.next().map(|&k| (k, fills[k]))
        } else {
            Some((s, s_left - units))
        };
    }
    let paid =
        Amount::sum(transfers.iter().map(|t| &t.amount)).map_err(|_| ClearingError::Overflow)?;
    let expected = price
        .micros()
        .checked_mul(u128::from(verified.volume))
        .map(Amount)
        .ok_or(ClearingError::Overflow)?;
    if paid != expected {
        return Err(ClearingError::Solver(format!(
            "settlement total {paid} differs from price × volume {expected}"
        )));
    }
    Ok(transfers)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sion_core::hash_domain;

    fn order(tag: &str, side: Side, quantity: u64, limit_micros: u128) -> Order {
        Order {
            id: hash_domain("test/order", tag.as_bytes()),
            owner: AccountId::derive(tag.as_bytes()),
            side,
            quantity,
            limit_price: Amount(limit_micros),
        }
    }

    fn batch(orders: Vec<Order>) -> Batch {
        Batch {
            domain: DomainId::derive(b"gpu-hours"),
            pulse: Pulse(1),
            beacon: hash_domain("test/beacon", b"0"),
            orders,
        }
    }

    #[test]
    fn empty_and_one_sided_batches() {
        for b in [
            batch(vec![]),
            batch(vec![order("b", Side::Buy, 3, 5)]),
            batch(vec![order("s", Side::Sell, 3, 5)]),
        ] {
            let result = clear(&b).unwrap();
            assert_eq!(result.volume, 0);
            assert_eq!(result.price, None);
            verify_clearing(&b, &result).unwrap();
            assert!(settlement(&b, &result).unwrap().is_empty());
        }
    }

    #[test]
    fn duplicate_ids_are_rejected() {
        let o = order("x", Side::Buy, 1, 1);
        assert!(matches!(
            clear(&batch(vec![o.clone(), o])),
            Err(ClearingError::InvalidBatch(_))
        ));
    }

    #[test]
    fn crossing_orders_trade_at_a_price_within_both_limits() {
        let b = batch(vec![
            order("buyer", Side::Buy, 10, 3_000_000),
            order("seller", Side::Sell, 10, 1_000_000),
        ]);
        let result = clear(&b).unwrap();
        assert_eq!(result.volume, 10);
        let band = result.band.unwrap();
        assert_eq!(
            band,
            PriceBand {
                low: Amount(1_000_000),
                high: Amount(3_000_000)
            }
        );
        assert_eq!(result.price, Some(Amount(2_000_000)));
        assert_eq!(result.welfare, Amount(20_000_000));
    }
}
