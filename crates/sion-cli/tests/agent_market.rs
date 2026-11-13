//! A market for agents' work on a devnet: a market is opened, buyers and
//! workers seal bids with proofs that they are inside its band, reveal
//! them, and the round clears at one price that the ledger's contents
//! give. The buyer posts a Task Charter for each unit to the worker it was
//! paired with, at that price, and a worker is paid for an accepted
//! result. A bid that is never revealed, one whose proof does not hold and
//! a reveal of something else are left out, and who followed through is
//! read from the ledger.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use sion_assurance::{AtomClass, Policy};
use sion_clearing::Side;
use sion_cli::ledger::LedgerExt;
use sion_cli::runtime::SharedLedger;
use sion_cli::session::Session;
use sion_cli::work_market::{self, Market, MarketCommand, Pair, Secret, Standing};
use sion_core::hash::Hash32;
use sion_core::ids::{AssetId, ClaimId, Pulse};
use sion_core::Amount;
use sion_exec::claim::ClaimBody;
use sion_exec::execute::{bond_atom, Event};
use sion_exec::task::{result_formula, Acceptance, TaskResult, RESULT_SCHEMA};
use sion_exec::{ExecStatus, TxKind};
use sion_node::{Devnet, DevnetConfig};

static ENVIRONMENT: Mutex<()> = Mutex::new(());

fn home(name: &str) -> std::path::PathBuf {
    let home = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    std::env::set_var("SION_HOME", &home);
    home
}

fn devnet() -> (Arc<Devnet>, SharedLedger) {
    let devnet = Arc::new(
        Devnet::start(DevnetConfig {
            rounds_per_pulse: 2,
            prove_chunk: None,
            // The rounds are read back from the node's events.
            history: 100_000,
            ..DevnetConfig::default()
        })
        .unwrap(),
    );
    let ledger: SharedLedger = devnet.clone();
    (devnet, ledger)
}

fn usd(n: u64) -> Amount {
    Amount::units(n)
}

/// Seals and publishes a bid of `wallet`'s in round 0, and returns its
/// secret.
fn bid(
    session: &mut Session<'_>,
    claim: &ClaimId,
    market: &Market,
    wallet: &str,
    side: Side,
    quantity: u64,
    price: u64,
) -> Secret {
    let bidder = session.id(wallet);
    let (sealed, mut secret) =
        work_market::seal(claim, market, 0, &bidder, side, quantity, usd(price)).unwrap();
    let published = work_market::bid(session, wallet, claim, &sealed).unwrap();
    secret.claim = Some(published.as_hash().to_hex());
    secret
}

#[test]
fn work_is_bid_for_sealed_cleared_at_one_price_and_paid_for_when_accepted() {
    let _one = ENVIRONMENT.lock().unwrap_or_else(|e| e.into_inner());
    let _home = home("agent-market");
    let (_devnet, ledger) = devnet();
    let mut session = Session::open(ledger.as_ref()).unwrap();
    for who in ["mona", "bea", "bo", "wes", "wyn", "zed", "mal", "carol"] {
        session
            .fund(who, AssetId::usd(), Amount::units(1_000))
            .unwrap();
    }
    // An adjudicator results name, and the workers' task bonds.
    let started = ledger.as_ref().pulse().unwrap().0;
    session
        .must(
            "carol",
            TxKind::PostBond {
                class: AtomClass::Adjudicator,
                label: "adjudicator".into(),
                amount: usd(100),
            },
            Vec::new(),
        )
        .unwrap();
    let adjudicator = bond_atom(AtomClass::Adjudicator, &session.id("carol"), "adjudicator").id;
    for worker in ["wes", "wyn"] {
        session
            .must(
                worker,
                TxKind::PostBond {
                    class: AtomClass::Economic,
                    label: sion_exec::task::WORKER_BOND_LABEL.into(),
                    amount: usd(100),
                },
                Vec::new(),
            )
            .unwrap();
    }

    // Mona opens a market for summaries, 1 to 10 USD each. Its windows are
    // sized to how many Pulses a transaction takes on this machine.
    let now = ledger.as_ref().pulse().unwrap().0;
    let per_tx = (now - started).div_ceil(3).max(1);
    let terms = Market {
        name: "summaries".into(),
        unit: "A summary of one contract of at most 20 pages, in 200 words.".into(),
        acceptance: Acceptance {
            policy: Policy::standard(),
            requires: BTreeSet::new(),
            verifiers: BTreeSet::new(),
            must_attest: BTreeSet::new(),
            min_attestations: 0,
            adjudicator: Some(adjudicator),
            detection_permille: 900,
            reliance: 1_000,
        },
        first: now + 4 * per_tx,
        // Seven bids and five reveals, two transactions each, twice over.
        bidding: 28 * per_tx + 10,
        revealing: 20 * per_tx + 10,
        min_price: 1_000_000,
        max_price: 10_000_000,
        max_quantity: 5,
        post_within: 400,
        deadline: 2_000,
    };
    let claim = work_market::open(&mut session, "mona", &terms).unwrap();
    let (found, market, convener) =
        work_market::market(ledger.as_ref(), &claim.as_hash().to_hex()).unwrap();
    assert_eq!(
        (found, &market, convener),
        (claim, &terms, session.id("mona"))
    );
    // A market that cannot hold is not opened, and a bid outside its band
    // or cap is not sealed.
    let mut upside_down = terms.clone();
    upside_down.min_price = 20_000_000;
    assert!(work_market::open(&mut session, "mona", &upside_down).is_err());
    let bea = session.id("bea");
    assert!(work_market::seal(&claim, &market, 0, &bea, Side::Buy, 1, usd(11)).is_err());
    assert!(work_market::seal(&claim, &market, 0, &bea, Side::Buy, 6, usd(5)).is_err());

    // Round 0. Bea buys 3 at up to 5 USD, bo 2 at up to 3; wes offers 2 at
    // 2 USD or more, wyn 4 at 4 or more. Zed's bid will never be revealed.
    session.wait_until(Pulse(market.bids_from(0))).unwrap();
    let mut secrets = Vec::new();
    for (wallet, side, quantity, price) in [
        ("bea", Side::Buy, 3, 5),
        ("bo", Side::Buy, 2, 3),
        ("wes", Side::Sell, 2, 2),
        ("wyn", Side::Sell, 4, 4),
        ("zed", Side::Buy, 5, 9),
    ] {
        secrets.push((
            wallet,
            bid(&mut session, &claim, &market, wallet, side, quantity, price),
        ));
    }
    // A second bid of bea's on the same side does not count.
    bid(&mut session, &claim, &market, "bea", Side::Buy, 5, 10);
    // Mal publishes a bid whose proof is of another bid: it is no bid.
    let mal = session.id("mal");
    let (mut forged, _) =
        work_market::seal(&claim, &market, 0, &mal, Side::Sell, 5, usd(1)).unwrap();
    let (other, _) = work_market::seal(&claim, &market, 0, &mal, Side::Sell, 1, usd(2)).unwrap();
    forged.price = other.price;
    assert!(!work_market::proven(&claim, &market, &mal, &forged));
    work_market::bid(&mut session, "mal", &claim, &forged).unwrap();
    let landed = ledger.as_ref().pulse().unwrap().0;
    assert!(
        landed < market.reveals_from(0),
        "the bids took until Pulse {landed}, past the window that ends at {}",
        market.reveals_from(0)
    );

    // Sealed, the ledger shows who bid on which side and nothing of what.
    let sealed = work_market::bids(ledger.as_ref(), claim, &market, 0).unwrap();
    assert_eq!(sealed.len(), 7);
    assert_eq!(
        sealed
            .iter()
            .filter(|b| b.standing == Standing::Sealed)
            .count(),
        5
    );
    let out: Vec<&str> = sealed
        .iter()
        .filter_map(|b| match &b.standing {
            Standing::Out(why) => Some(why.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(out.len(), 2, "{out:?}");
    assert!(out.iter().any(|why| why.contains("already has a bid")));
    assert!(out
        .iter()
        .any(|why| why.contains("its proof does not show")));
    assert!(work_market::cleared(ledger.as_ref(), claim, &market, 0)
        .unwrap()
        .is_none());
    // A reveal before the window opens does not count.
    work_market::reveal(&mut session, "bo", &claim, &secrets[1].1).unwrap();

    // The reveals: everyone but zed. Wyn first reveals something else than
    // it committed to, which opens nothing.
    session.wait_until(Pulse(market.reveals_from(0))).unwrap();
    let mut lie = secrets[3].1.clone();
    lie.quantity = secrets[2].1.quantity.clone();
    work_market::reveal(&mut session, "wyn", &claim, &lie).unwrap();
    for (wallet, secret) in &secrets[..4] {
        // Bo's early reveal is the same claim: published again, it would
        // be refused as one that exists, so its reveal is made anew.
        if *wallet == "bo" {
            continue;
        }
        work_market::reveal(&mut session, wallet, &claim, secret).unwrap();
    }
    let landed = ledger.as_ref().pulse().unwrap().0;
    assert!(landed < market.closes(0), "the reveals took until {landed}");
    let revealed = work_market::bids(ledger.as_ref(), claim, &market, 0).unwrap();
    let standing = |wallet: &str, session: &mut Session<'_>| {
        let who = session.id(wallet);
        revealed
            .iter()
            .find(|b| b.bidder == who && !matches!(b.standing, Standing::Out(_)))
            .map(|b| b.standing.clone())
    };
    assert_eq!(
        standing("bea", &mut session),
        Some(Standing::Open {
            quantity: 3,
            price: 5_000_000
        })
    );
    // Bo revealed too early and not again; wyn's false reveal used up its
    // bid.
    assert_eq!(standing("bo", &mut session), Some(Standing::Sealed));
    assert_eq!(standing("wyn", &mut session), None);
    assert_eq!(standing("zed", &mut session), Some(Standing::Sealed));

    // Closed, the round clears from what the ledger holds: bea's 3 units
    // against wes's 2 at 2 USD. One price for every unit.
    session.wait_until(Pulse(market.closes(0))).unwrap();
    let (bids, cleared) = work_market::cleared(ledger.as_ref(), claim, &market, 0)
        .unwrap()
        .unwrap();
    assert_eq!(cleared.batch.orders.len(), 2);
    assert_eq!(cleared.result.volume, 2);
    let price = cleared.price.unwrap();
    assert!(price >= usd(2) && price <= usd(5), "{price}");
    assert_eq!(
        cleared.pairs,
        vec![Pair {
            buyer: session.id("bea"),
            worker: session.id("wes"),
            units: 2
        }]
    );
    // Anyone computes the same, and a record of it is checked against that.
    let again = work_market::clear(cleared.batch.clone()).unwrap();
    assert_eq!(
        (again.price, again.digest()),
        (cleared.price, cleared.digest())
    );
    sion_clearing::verify_clearing(&cleared.batch, &cleared.result).unwrap();
    work_market::command(
        &mut session,
        ledger.as_ref(),
        MarketCommand::Clear {
            market: claim.as_hash().to_hex(),
            round: 0,
            wallet: Some("mona".into()),
        },
    )
    .unwrap();
    assert_eq!(bids.len(), 7);

    // Bea posts what she owes: a task for each unit, to wes, at the price.
    let before = session.balance("bea", AssetId::usd()).unwrap();
    work_market::command(
        &mut session,
        ledger.as_ref(),
        MarketCommand::Post {
            market: claim.as_hash().to_hex(),
            round: 0,
            item: None,
            wallet: "bea".into(),
        },
    )
    .unwrap();
    let escrowed = before.micros() - session.balance("bea", AssetId::usd()).unwrap().micros();
    assert!(
        escrowed >= 2 * price.micros() && escrowed < 2 * price.micros() + 100_000,
        "{escrowed}"
    );
    let followed =
        work_market::followed(ledger.as_ref(), &market, 0, price, &cleared.pairs).unwrap();
    assert_eq!((followed[0].posted.len(), followed[0].settled), (2, 0));
    // Posting again posts nothing more, and wes, who bought nothing, owes
    // nothing.
    for wallet in ["bea", "wes"] {
        let posted =
            work_market::post(&mut session, wallet, &claim, &market, 0, &cleared, b"").unwrap();
        assert!(posted.is_empty());
    }

    // Wes delivers one: a result on its task bond, accepted and paid at
    // the round's price.
    let task = followed[0].posted[0];
    let wes = session.id("wes");
    let result = ClaimBody {
        schema: RESULT_SCHEMA.into(),
        subject: "summary of contract 7".into(),
        payload: TaskResult {
            task,
            output: sion_core::hash::hash_domain("test/output", b"The contract runs a year."),
        }
        .payload(),
        formula: result_formula(&wes, &[], &BTreeSet::new()),
        adjudicator: Some(adjudicator),
        detection_permille: 900,
        valid_for: 5_000,
        challenge_period: 2_000,
    };
    session
        .must(
            "wes",
            TxKind::IssueClaim {
                claim: result.clone(),
            },
            Vec::new(),
        )
        .unwrap();
    let paid_before = session.balance("wes", AssetId::usd()).unwrap();
    let view = session
        .send(
            "wes",
            TxKind::SettleTask {
                task,
                result,
                attestations: Vec::new(),
            },
            Vec::new(),
        )
        .unwrap();
    assert!(
        matches!(view.effects.status, ExecStatus::Success),
        "{:?}",
        view.effects.status
    );
    assert!(view
        .effects
        .events
        .iter()
        .any(|e| matches!(e, Event::TaskSettled { amount, .. } if *amount == price)));
    let gained = session.balance("wes", AssetId::usd()).unwrap().micros() - paid_before.micros();
    assert!(gained + 100_000 > price.micros() && gained <= price.micros());

    let lines = work_market::lines(ledger.as_ref(), claim, &market, &convener, [0])
        .unwrap()
        .join("\n");
    assert!(lines.contains("round 0: closed at Pulse"), "{lines}");
    assert!(lines.contains("left out: bid"), "{lines}");
    assert!(lines.contains("never revealed: bid"), "{lines}");
    assert!(
        lines.contains(&format!("cleared: 2 unit(s) at {price} USD a unit")),
        "{lines}"
    );
    assert!(
        lines.contains("buys 2 unit(s) from") && lines.contains("2 task(s) posted, 1 paid for"),
        "{lines}"
    );
    assert!(lines.contains("recorded by"), "{lines}");
    assert_eq!(
        work_market::prices(ledger.as_ref(), claim, &market, [0])
            .unwrap()
            .get(&0),
        Some(&price)
    );
    let _ = (SharedLedger::clone, Hash32::ZERO, Arc::new(0));
}

/// A node keeps the events of its last Pulses only. A round whose bids it
/// no longer holds is not cleared from the part it does hold.
#[test]
fn a_round_is_not_read_from_a_node_that_no_longer_keeps_it() {
    let _one = ENVIRONMENT.lock().unwrap_or_else(|e| e.into_inner());
    let _home = home("agent-market-pruned");
    let devnet = Arc::new(
        Devnet::start(DevnetConfig {
            rounds_per_pulse: 2,
            prove_chunk: None,
            history: 8,
            ..DevnetConfig::default()
        })
        .unwrap(),
    );
    let ledger: SharedLedger = devnet.clone();
    let mut session = Session::open(ledger.as_ref()).unwrap();
    session
        .fund("mona", AssetId::usd(), Amount::units(100))
        .unwrap();
    let now = ledger.as_ref().pulse().unwrap().0;
    let terms = Market {
        name: "pruned".into(),
        unit: "A summary.".into(),
        acceptance: Acceptance {
            policy: Policy::standard(),
            requires: BTreeSet::new(),
            verifiers: BTreeSet::new(),
            must_attest: BTreeSet::new(),
            min_attestations: 0,
            adjudicator: None,
            detection_permille: 900,
            reliance: 1_000,
        },
        first: now + 1,
        bidding: 2,
        revealing: 2,
        min_price: 1_000_000,
        max_price: 10_000_000,
        max_quantity: 5,
        post_within: 100,
        deadline: 2_000,
    };
    let claim = work_market::open(&mut session, "mona", &terms).unwrap();
    // While the node keeps the round it is read, empty as it is.
    session.wait_until(Pulse(terms.closes(0))).unwrap();
    let (bids, cleared) = work_market::cleared(ledger.as_ref(), claim, &terms, 0)
        .unwrap()
        .unwrap();
    assert!(bids.is_empty() && cleared.price.is_none() && cleared.pairs.is_empty());
    // Once it does not, the round is refused, not read in part.
    session.wait_until(Pulse(terms.closes(0) + 20)).unwrap();
    let refused = work_market::bids(ledger.as_ref(), claim, &terms, 0)
        .unwrap_err()
        .to_string();
    assert!(refused.contains("no longer answers for Pulse"), "{refused}");
    assert!(work_market::cleared(ledger.as_ref(), claim, &terms, 0).is_err());
}
