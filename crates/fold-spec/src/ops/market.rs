//! Markets: CreateMarket (§5.22), SubmitOrder (§5.23), RevealBatch (§5.24)
//! and ClearBatch (§5.25). Certificate checking is the shared
//! `fold_clearing::verify_clearing`; everything around it is written from
//! the specification (this crate never calls `OrderTerms::escrow`,
//! `Market::clearing_batch` or `Market::batch_index`).

use std::collections::BTreeMap;

use fold_clearing::{verify_clearing, ClearingResult, Order, Side};
use fold_core::encode::decode;
use fold_core::hash::{hash_parts, Hash32};
use fold_core::ids::{AssetId, ObjectId};
use fold_core::Amount;
use fold_crypto::{KemPublicKey, KeyShare};
use fold_exec::execute::{Deferred, Event};
use fold_exec::market::{
    sealing_aad, BatchStatus, Market, OrderTerms, PendingOrder, Submission, MAX_PERIOD,
    SEALED_ORDER_DOMAIN,
};
use fold_exec::object::{Object, ObjectData, Owner};
use fold_exec::tx::object_domain;

use crate::error::{fail, Step};
use crate::tx::TxRun;

/// Domain of order identifiers.
const ORDER_DOMAIN: &str = "fold/market/order";

/// The parameters of a CreateMarket transaction.
pub(super) struct MarketParams<'a> {
    /// Name.
    pub label: &'a str,
    /// The good.
    pub good: AssetId,
    /// The numeraire.
    pub numeraire: AssetId,
    /// Pulses per batch.
    pub batch_length: u64,
    /// Pulses allowed for clearing after sealing.
    pub clear_deadline: u64,
    /// Committee for sealed orders.
    pub committee: &'a [KemPublicKey],
    /// Reveal threshold.
    pub threshold: u8,
}

/// The escrow plain terms need (§5.23): `(numeraire, limit_price ×
/// quantity)` (saturating) for buys, `(good, units(quantity))` for sells.
fn escrow_need(good: AssetId, numeraire: AssetId, terms: &OrderTerms) -> (AssetId, Amount) {
    match terms.side {
        Side::Buy => (
            numeraire,
            Amount(
                terms
                    .limit_price
                    .0
                    .saturating_mul(u128::from(terms.quantity)),
            ),
        ),
        Side::Sell => (good, Amount::units(terms.quantity)),
    }
}

/// Loads a market object.
fn load_market(run: &mut TxRun<'_>, id: ObjectId) -> Step<(Object, Market)> {
    let object = run.load(&id)?;
    let ObjectData::Market(market) = &object.data else {
        return Err(fail(format!("{id} is not a market")));
    };
    let market = (**market).clone();
    Ok((object, market))
}

/// Stores a market object with updated contents.
fn store_market(run: &mut TxRun<'_>, object: Object, market: Market) -> Step {
    run.store(Object {
        data: ObjectData::Market(Box::new(market)),
        ..object
    })
}

/// §5.22 CreateMarket.
pub(super) fn create_market(run: &mut TxRun<'_>, params: MarketParams<'_>) -> Step {
    if !(1..=MAX_PERIOD).contains(&params.batch_length) {
        return Err(fail(format!(
            "batch length must be between 1 and {MAX_PERIOD} Pulses"
        )));
    }
    if !(1..=MAX_PERIOD).contains(&params.clear_deadline) {
        return Err(fail(format!(
            "clearing deadline must be between 1 and {MAX_PERIOD} Pulses"
        )));
    }
    if !params.committee.is_empty()
        && (params.threshold == 0 || usize::from(params.threshold) > params.committee.len())
    {
        return Err(fail("threshold must be between 1 and the committee size"));
    }
    if params.good == params.numeraire {
        return Err(fail("the good and the numeraire must differ"));
    }
    let market_id = run.next_created_id();
    let domain = object_domain("market", &market_id);
    run.create(
        Owner::Shared(domain),
        ObjectData::Market(Box::new(Market {
            label: params.label.to_owned(),
            domain,
            good: params.good,
            numeraire: params.numeraire,
            batch_length: params.batch_length,
            clear_deadline: params.clear_deadline,
            committee: params.committee.to_vec(),
            threshold: params.threshold,
            batches: BTreeMap::new(),
        })),
    )?;
    run.event(Event::MarketCreated { market: market_id });
    Ok(())
}

/// §5.23 SubmitOrder: escrows the order's funds and defers adding it to the
/// market's open batch.
pub(super) fn submit_order(
    run: &mut TxRun<'_>,
    market_id: ObjectId,
    submission: &Submission,
) -> Step {
    let (_, market) = load_market(run, market_id)?;
    let (escrow_asset, escrow, terms, sealed) = match submission {
        Submission::Plain(terms) => {
            if terms.quantity == 0 {
                return Err(fail("zero quantity"));
            }
            let (asset, amount) = escrow_need(market.good, market.numeraire, terms);
            (asset, amount, Some(*terms), None)
        }
        Submission::Sealed {
            envelope,
            side,
            escrow,
        } => {
            if market.committee.is_empty() {
                return Err(fail("the market takes no sealed orders"));
            }
            let members: Vec<Hash32> = market
                .committee
                .iter()
                .map(KemPublicKey::fingerprint)
                .collect();
            if envelope.members != members || envelope.threshold != market.threshold {
                return Err(fail("the order is not sealed to the market's committee"));
            }
            if *escrow == Amount::ZERO {
                return Err(fail("a sealed order needs escrow"));
            }
            let asset = match side {
                Side::Buy => market.numeraire,
                Side::Sell => market.good,
            };
            (asset, *escrow, None, Some(envelope.clone()))
        }
    };
    run.draw(escrow_asset, escrow)?;
    let order_id = hash_parts(ORDER_DOMAIN, &[&run.digest.0 .0]);
    let owner = run.sender();
    run.defer(Deferred::AddOrder {
        market: market_id,
        order: PendingOrder {
            id: order_id,
            owner,
            escrow_asset,
            escrow,
            terms,
            sealed,
        },
    });
    run.event(Event::OrderSubmitted {
        market: market_id,
        order: order_id,
    });
    Ok(())
}

/// §5.24 RevealBatch: opens sealed orders of a sealed batch with published
/// committee shares.
pub(super) fn reveal_batch(
    run: &mut TxRun<'_>,
    market_id: ObjectId,
    index: u64,
    shares: &[(Hash32, Vec<KeyShare>)],
) -> Step {
    let (object, mut market) = load_market(run, market_id)?;
    let (good, numeraire) = (market.good, market.numeraire);
    let aad = sealing_aad(&market.domain, index);
    let batch = market
        .batches
        .get_mut(&index)
        .ok_or_else(|| fail("no such batch"))?;
    if !matches!(batch.status, BatchStatus::Sealed { .. }) {
        return Err(fail("the batch is not sealed"));
    }
    let mut provided: BTreeMap<Hash32, &[KeyShare]> = BTreeMap::new();
    for (order, order_shares) in shares {
        provided.insert(*order, order_shares);
    }
    let mut revealed = 0u64;
    for order in &mut batch.orders {
        if order.terms.is_some() {
            continue;
        }
        let Some(envelope) = &order.sealed else {
            continue;
        };
        let Some(order_shares) = provided.get(&order.id) else {
            continue;
        };
        let Ok(plaintext) = envelope.open(order_shares, SEALED_ORDER_DOMAIN, &aad) else {
            continue;
        };
        let Ok(terms) = decode::<OrderTerms>(&plaintext) else {
            continue;
        };
        let (asset, need) = escrow_need(good, numeraire, &terms);
        if asset == order.escrow_asset && need <= order.escrow && terms.quantity > 0 {
            order.terms = Some(terms);
            revealed += 1;
        }
    }
    store_market(run, object, market)?;
    run.event(Event::BatchRevealed {
        market: market_id,
        batch: index,
        revealed,
    });
    Ok(())
}

/// §5.25 ClearBatch: settles a sealed batch with a certified clearing.
pub(super) fn clear_batch(
    run: &mut TxRun<'_>,
    market_id: ObjectId,
    index: u64,
    result: &ClearingResult,
) -> Step {
    let (object, mut market) = load_market(run, market_id)?;
    let batch = market
        .batches
        .get(&index)
        .ok_or_else(|| fail("no such batch"))?;
    let BatchStatus::Sealed { beacon, at } = batch.status else {
        return Err(fail("the batch is not sealed"));
    };
    if batch.orders.iter().any(|order| order.terms.is_none()) {
        // §5.25 step 2, the addition saturating.
        let earliest = at.0.saturating_add(market.clear_deadline.saturating_sub(1));
        if run.ctx.pulse.0 < earliest {
            return Err(fail("sealed orders are still being revealed"));
        }
    }
    let orders: Vec<Order> = batch
        .orders
        .iter()
        .filter_map(|pending| {
            pending.terms.map(|terms| Order {
                id: pending.id,
                owner: pending.owner,
                side: terms.side,
                quantity: terms.quantity,
                limit_price: terms.limit_price,
            })
        })
        .collect();
    let clearing = fold_clearing::Batch {
        domain: market.domain,
        pulse: at,
        beacon,
        orders,
    };
    let verified = verify_clearing(&clearing, result)?;
    let price = verified.price;
    let fills: BTreeMap<Hash32, u64> = result
        .fills
        .iter()
        .map(|fill| (fill.order_id, fill.quantity))
        .collect();
    for order in &batch.orders {
        let filled = fills.get(&order.id).copied().unwrap_or(0);
        let (receive_asset, receive, refund) = match (order.terms, price) {
            (Some(terms), Some(price)) if filled > 0 => {
                let value = Amount(
                    price
                        .0
                        .checked_mul(u128::from(filled))
                        .ok_or_else(|| fail("trade value overflows"))?,
                );
                match terms.side {
                    Side::Buy => (
                        market.good,
                        Amount::units(filled),
                        order.escrow.checked_sub(value)?,
                    ),
                    Side::Sell => (
                        market.numeraire,
                        value,
                        order.escrow.checked_sub(Amount::units(filled))?,
                    ),
                }
            }
            _ => (order.escrow_asset, Amount::ZERO, order.escrow),
        };
        run.mint(order.owner, receive_asset, receive)?;
        run.mint(order.owner, order.escrow_asset, refund)?;
    }
    let solver = run.sender();
    if let Some(batch) = market.batches.get_mut(&index) {
        batch.status = BatchStatus::Cleared {
            price,
            volume: verified.volume,
            welfare: verified.welfare,
            solver,
        };
    }
    store_market(run, object, market)?;
    run.event(Event::BatchCleared {
        market: market_id,
        batch: index,
        price,
        volume: verified.volume,
        welfare: verified.welfare,
    });
    Ok(())
}
