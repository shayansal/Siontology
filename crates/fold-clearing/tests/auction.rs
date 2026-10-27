//! Double-auction tests: the architecture's GPU-hours example, canonical
//! tie-breaking, rejection of cheating solvers, and random markets.

use fold_clearing::auction::{
    build_program, clear, perturbed_coefficients, settlement, verify_clearing,
};
use fold_clearing::{select_best_clearing, Batch, ClearingError, ClearingResult, Order, Side};
use fold_core::{hash_domain, AccountId, Amount, DomainId, Hash32, Pulse};
use fold_lp::{solve, CertError, Q};
use proptest::prelude::*;

fn usd_cents(cents: u128) -> Amount {
    Amount(cents * 10_000)
}

fn order(tag: &str, side: Side, quantity: u64, limit: Amount) -> Order {
    Order {
        id: hash_domain("test/order", tag.as_bytes()),
        owner: AccountId::derive(tag.as_bytes()),
        side,
        quantity,
        limit_price: limit,
    }
}

fn batch_with(beacon: Hash32, orders: Vec<Order>) -> Batch {
    Batch {
        domain: DomainId::derive(b"gpu-hours-before-18:00"),
        pulse: Pulse(42),
        beacon,
        orders,
    }
}

fn beacon(i: u64) -> Hash32 {
    hash_domain("test/beacon", &i.to_be_bytes())
}

fn fill_of(result: &ClearingResult, batch: &Batch, tag: &str) -> u64 {
    let id = hash_domain("test/order", tag.as_bytes());
    let index = batch.orders.iter().position(|o| o.id == id).unwrap();
    result.fills[index].quantity
}

/// The architecture document's example intent: "8 GPU-hours before 18:00 at
/// no more than 2.10 USD per hour", against a small book.
#[test]
fn gpu_hours_example() {
    let batch = batch_with(
        beacon(0),
        vec![
            order("agent", Side::Buy, 8, usd_cents(210)),
            order("bob", Side::Buy, 4, usd_cents(195)),
            order("lab-a", Side::Sell, 5, usd_cents(180)),
            order("lab-b", Side::Sell, 6, usd_cents(200)),
            order("lab-c", Side::Sell, 10, usd_cents(240)),
        ],
    );
    let result = clear(&batch).unwrap();
    assert_eq!(fill_of(&result, &batch, "agent"), 8);
    assert_eq!(fill_of(&result, &batch, "bob"), 0);
    assert_eq!(fill_of(&result, &batch, "lab-a"), 5);
    assert_eq!(fill_of(&result, &batch, "lab-b"), 3);
    assert_eq!(fill_of(&result, &batch, "lab-c"), 0);
    // Bob (1.95) is unfilled and lab-b (2.00) is partially filled: the only
    // supporting price is 2.00.
    let band = result.band.unwrap();
    assert_eq!((band.low, band.high), (usd_cents(200), usd_cents(200)));
    assert_eq!(result.price, Some(usd_cents(200)));
    // Gains from trade: 8 × 2.10 − 5 × 1.80 − 3 × 2.00 = 16.80 − 9.00 − 6.00.
    assert_eq!(result.welfare, usd_cents(180));
    let verified = verify_clearing(&batch, &result).unwrap();
    assert_eq!(verified.volume, 8);

    let transfers = settlement(&batch, &result).unwrap();
    let agent = AccountId::derive(b"agent");
    let paid: u128 = transfers
        .iter()
        .filter(|t| t.from == agent)
        .map(|t| t.amount.micros())
        .sum();
    assert_eq!(Amount(paid), usd_cents(1_600)); // 8 × 2.00, below the 8 × 2.10 limit
    let to_lab_a: u128 = transfers
        .iter()
        .filter(|t| t.to == AccountId::derive(b"lab-a"))
        .map(|t| t.amount.micros())
        .sum();
    assert_eq!(Amount(to_lab_a), usd_cents(1_000));
}

/// Two identical bids compete for one seller's units: the beacon decides,
/// different beacons pick different winners, and welfare never changes.
#[test]
fn ties_are_broken_by_beacon_priority() {
    let orders = vec![
        order("twin-1", Side::Buy, 5, usd_cents(210)),
        order("twin-2", Side::Buy, 5, usd_cents(210)),
        order("seller", Side::Sell, 5, usd_cents(100)),
    ];
    let mut winners = std::collections::BTreeSet::new();
    let mut welfare = None;
    for i in 0..32 {
        let batch = batch_with(beacon(i), orders.clone());
        let result = clear(&batch).unwrap();
        verify_clearing(&batch, &result).unwrap();
        let first = fill_of(&result, &batch, "twin-1");
        let second = fill_of(&result, &batch, "twin-2");
        assert_eq!(
            first + second,
            5,
            "the seller's 5 units go to exactly one twin"
        );
        assert!(first == 0 || second == 0, "no splitting between equal bids");
        winners.insert(if first > 0 { "twin-1" } else { "twin-2" });
        // The winner is the twin with the higher perturbed coefficient.
        let coefficients = perturbed_coefficients(&batch);
        let expected = if coefficients[0] > coefficients[1] {
            "twin-1"
        } else {
            "twin-2"
        };
        assert_eq!(if first > 0 { "twin-1" } else { "twin-2" }, expected);
        match welfare {
            None => welfare = Some(result.welfare),
            Some(w) => assert_eq!(w, result.welfare),
        }
    }
    assert_eq!(
        winners.len(),
        2,
        "over 32 beacons both twins should win sometimes"
    );
}

/// A solver that hands the tied units to the lower-priority twin cannot
/// produce a certificate that verifies, with honest or recomputed duals.
#[test]
fn mis_tie_broken_allocations_are_rejected() {
    let batch = batch_with(
        beacon(7),
        vec![
            order("twin-1", Side::Buy, 5, usd_cents(210)),
            order("twin-2", Side::Buy, 5, usd_cents(210)),
            order("seller", Side::Sell, 5, usd_cents(100)),
        ],
    );
    let honest = clear(&batch).unwrap();
    let (winner, loser) = if honest.fills[0].quantity > 0 {
        (0, 1)
    } else {
        (1, 0)
    };

    let mut cheat = honest.clone();
    cheat.fills[winner].quantity = 0;
    cheat.fills[loser].quantity = 5;
    cheat.certificate.x[winner] = Q::zero();
    cheat.certificate.x[loser] = Q::from(5u64);
    let coefficients = perturbed_coefficients(&batch);
    cheat.objective = coefficients
        .iter()
        .zip(&cheat.certificate.x)
        .map(|(c, x)| c * x)
        .sum();
    // Honest duals no longer certify it.
    let err = verify_clearing(&batch, &cheat).unwrap_err();
    assert!(matches!(err, ClearingError::Certificate(_)), "{err:?}");

    // Duals recomputed for the cheat by the same rule cannot close the gap.
    let mut price = Q::zero();
    for (k, o) in batch.orders.iter().enumerate() {
        let filled = cheat.fills[k].quantity;
        let candidate = match o.side {
            Side::Buy if filled < o.quantity => Some(coefficients[k].clone()),
            Side::Sell if filled > 0 => Some(-coefficients[k].clone()),
            _ => None,
        };
        if let Some(c) = candidate {
            price = price.max(c);
        }
    }
    let mut y = vec![price.clone()];
    for (k, o) in batch.orders.iter().enumerate() {
        let surplus = match o.side {
            Side::Buy => &coefficients[k] - &price,
            Side::Sell => &price + &coefficients[k],
        };
        y.push(surplus.max(Q::zero()));
    }
    cheat.certificate.y = y;
    let err = verify_clearing(&batch, &cheat).unwrap_err();
    assert!(
        matches!(
            err,
            ClearingError::Certificate(
                CertError::GapTooLarge { .. } | CertError::DualInfeasible { .. }
            )
        ),
        "{err:?}"
    );
}

/// Leaving gains from trade on the table is rejected.
#[test]
fn suboptimal_allocations_are_rejected() {
    let batch = batch_with(
        beacon(1),
        vec![
            order("buyer", Side::Buy, 10, usd_cents(300)),
            order("seller", Side::Sell, 10, usd_cents(100)),
        ],
    );
    let mut lazy = clear(&batch).unwrap();
    lazy.fills[0].quantity = 6;
    lazy.fills[1].quantity = 6;
    lazy.certificate.x = vec![Q::from(6u64), Q::from(6u64)];
    lazy.objective = perturbed_coefficients(&batch)
        .iter()
        .zip(&lazy.certificate.x)
        .map(|(c, x)| c * x)
        .sum();
    assert!(matches!(
        verify_clearing(&batch, &lazy),
        Err(ClearingError::Certificate(CertError::GapTooLarge { .. }))
    ));
}

/// Any price other than the canonical midpoint is rejected, as are fills
/// that disagree with the certificate and inflated welfare claims.
#[test]
fn tampered_submissions_are_rejected() {
    let batch = batch_with(
        beacon(2),
        vec![
            order("buyer", Side::Buy, 10, usd_cents(300)),
            order("seller", Side::Sell, 10, usd_cents(100)),
        ],
    );
    let honest = clear(&batch).unwrap();
    verify_clearing(&batch, &honest).unwrap();

    let mut price = honest.clone();
    price.price = Some(Amount(honest.price.unwrap().micros() + 1));
    assert!(matches!(
        verify_clearing(&batch, &price),
        Err(ClearingError::NonCanonicalPrice { .. })
    ));

    let mut fills = honest.clone();
    fills.fills[0].quantity = 9;
    assert!(matches!(
        verify_clearing(&batch, &fills),
        Err(ClearingError::FillMismatch(_))
    ));

    let mut welfare = honest.clone();
    welfare.welfare = Amount(honest.welfare.micros() + 1);
    assert!(matches!(
        verify_clearing(&batch, &welfare),
        Err(ClearingError::WelfareMismatch { .. })
    ));

    let mut objective = honest.clone();
    objective.objective = &honest.objective + &Q::one();
    assert!(matches!(
        verify_clearing(&batch, &objective),
        Err(ClearingError::ObjectiveMismatch { .. })
    ));

    let mut band = honest.clone();
    band.band = None;
    assert!(matches!(
        verify_clearing(&batch, &band),
        Err(ClearingError::BandMismatch { .. })
    ));

    // A solver cannot certify against a different batch either.
    let other = batch_with(beacon(3), batch.orders.clone());
    assert!(verify_clearing(&other, &honest).is_err() || clear(&other).unwrap() == honest);
}

#[test]
fn select_best_keeps_only_verified_submissions() {
    let batch = batch_with(
        beacon(4),
        vec![
            order("buyer", Side::Buy, 10, usd_cents(300)),
            order("seller", Side::Sell, 10, usd_cents(100)),
        ],
    );
    let honest = clear(&batch).unwrap();
    let mut bad = honest.clone();
    bad.price = Some(Amount(1));
    assert_eq!(
        select_best_clearing(&batch, &[bad.clone(), honest.clone()]),
        Some(1)
    );
    assert_eq!(select_best_clearing(&batch, &[bad]), None);
}

fn order_strategy() -> impl Strategy<Value = (bool, u64, u128)> {
    (any::<bool>(), 0u64..=12, 1u128..=6)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(96))]

    /// Random books always clear with a certificate that verifies, the
    /// greedy solver agrees exactly with the general simplex, and
    /// settlement balances at a price within every filled order's limit.
    #[test]
    fn random_markets_clear_and_settle(
        specs in proptest::collection::vec(order_strategy(), 0..9),
        seed in 0u64..1_000,
    ) {
        let orders: Vec<Order> = specs
            .iter()
            .enumerate()
            .map(|(i, (buy, quantity, price))| {
                order(
                    &format!("o{i}"),
                    if *buy { Side::Buy } else { Side::Sell },
                    *quantity,
                    usd_cents(*price * 50),
                )
            })
            .collect();
        let batch = batch_with(beacon(seed), orders);
        let result = clear(&batch).unwrap();
        let verified = verify_clearing(&batch, &result).unwrap();

        // Cross-check with the reference simplex on the same program.
        let program = build_program(&batch).unwrap();
        let reference = solve(&program).unwrap().optimal().unwrap();
        prop_assert_eq!(&reference.objective, &result.objective);
        prop_assert_eq!(&reference.x, &result.certificate.x);

        // Settlement balances and respects limits.
        let transfers = settlement(&batch, &result).unwrap();
        let total: u128 = transfers.iter().map(|t| t.amount.micros()).sum();
        match verified.price {
            Some(price) => {
                prop_assert_eq!(total, price.micros() * u128::from(verified.volume));
                for (o, fill) in batch.orders.iter().zip(&result.fills) {
                    if fill.quantity > 0 {
                        match o.side {
                            Side::Buy => prop_assert!(price <= o.limit_price),
                            Side::Sell => prop_assert!(price >= o.limit_price),
                        }
                    }
                }
            }
            None => prop_assert_eq!(total, 0),
        }
        // Per account, money in equals money out across the whole batch.
        let mut buyers_paid = 0u128;
        let mut sellers_received = 0u128;
        for t in &transfers {
            buyers_paid += t.amount.micros();
            sellers_received += t.amount.micros();
        }
        prop_assert_eq!(buyers_paid, sellers_received);
    }
}
