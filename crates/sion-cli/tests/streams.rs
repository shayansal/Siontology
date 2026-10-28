//! Paying an x402 server from a payment stream, on a real devnet: what the
//! server's desk refuses, how an honest client recovers, and what the desk
//! settles when the server stops.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::json;
use sion_cli::demos::fund_all;
use sion_cli::stream::{load_stream, PayerBook, PayerRecord, Payment, StreamDesk};
use sion_cli::x402::{
    encode_header, fetch, fetch_with, replay, Facilitator, Fetched, PaidRoute, PaidServer, Payer,
    PaymentPayload, X402_VERSION,
};
use sion_cli::Session;
use sion_core::hash::Hash32;
use sion_core::ids::{AssetId, Pulse};
use sion_core::Amount;
use sion_exec::{Event, TxKind};
use sion_node::{Devnet, DevnetConfig};

fn refusal(fetched: &Fetched) -> String {
    assert_eq!(fetched.status, 402, "{fetched:?}");
    fetched
        .settlement
        .as_ref()
        .and_then(|s| s.error_reason.clone())
        .unwrap_or_default()
}

fn position(fetched: &Fetched) -> u64 {
    assert_eq!(fetched.status, 200, "{fetched:?}");
    fetched
        .settlement
        .as_ref()
        .unwrap()
        .extensions
        .as_ref()
        .unwrap()["sion"]["position"]
        .as_u64()
        .unwrap()
}

#[test]
fn a_stream_desk_refuses_what_it_cannot_settle_and_clients_recover() {
    let devnet = Devnet::start(DevnetConfig {
        rounds_per_pulse: 2,
        prove_chunk: None,
        ..DevnetConfig::default()
    })
    .unwrap();
    let mut session = Session::open(&devnet).unwrap();
    let chain = sion_cli::verify::served_chain(&devnet).unwrap();
    // carol needs a little USD of her own: settlements pay fees like any
    // transaction.
    fund_all(
        &mut session,
        &[
            ("agent", AssetId::usd(), Amount::units(10)),
            ("carol", AssetId::usd(), Amount::units(1)),
        ],
    )
    .unwrap();
    let carol = session.id("carol");
    let unit = Amount(10_000);
    let (seed, payer_chain) = PayerBook::new_chain(1_000, 0);
    let until = Pulse(devnet.pulse().unwrap().0 + 1_000);
    // Five steps' worth of escrow.
    let view = session
        .must(
            "agent",
            TxKind::OpenStream {
                payee: carol,
                asset: AssetId::usd(),
                unit,
                length: 1_000,
                anchor: payer_chain.anchor(),
                deposit: Amount(5 * unit.0),
                until,
                checkpoints: None,
            },
            Vec::new(),
        )
        .unwrap();
    let stream = view
        .effects
        .events
        .iter()
        .find_map(|e| match e {
            Event::StreamOpened { stream, .. } => Some(*stream),
            _ => None,
        })
        .unwrap();
    session
        .streams
        .add(
            PayerRecord {
                stream,
                wallet: "agent".into(),
                payee: carol,
                unit,
                length: 1_000,
                seed,
                revealed: 0,
                spacing: 0,
            },
            payer_chain,
        )
        .unwrap();
    let carol_before = session.balance("carol", AssetId::usd()).unwrap();

    let mut desk = StreamDesk::new(sion_cli::session::wallet_for("carol"));
    // Settle only when the server stops.
    desk.settle_every = u64::MAX;
    desk.settle_after = Duration::from_secs(3_600);
    let facilitator = Facilitator::new(&devnet, chain).unwrap().with_streams(desk);
    let server = PaidServer::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", server.address);
    let route = |path: &str, price: Amount| PaidRoute {
        path: path.into(),
        price,
        description: path.into(),
        mime_type: "text/plain".into(),
        body: Box::new(|_| "ok".to_string()),
    };
    let routes = vec![
        route("/one", unit),
        route("/three", Amount(3 * unit.0)),
        route("/odd", Amount(15_000)),
    ];
    let stop = AtomicBool::new(false);
    std::thread::scope(|scope| {
        let serving = scope
            .spawn(|| server.serve(&facilitator, carol, &routes, Duration::from_secs(60), &stop));
        let payer = Payer::Stream(stream);
        let max = Amount::units(1);
        let one = format!("{base}/one");

        // A one-step call, then a three-step call.
        let first = fetch(&mut session, &payer, &one, &chain, max).unwrap();
        assert_eq!(position(&first), 1);
        let known = first.accepted.clone().unwrap();
        let three = fetch(&mut session, &payer, &format!("{base}/three"), &chain, max).unwrap();
        assert_eq!(position(&three), 4);

        // The client loses a payment on the way (it revealed position 5 but
        // the request never arrived): the server refuses position 6, names
        // position 4, and the client goes back and pays 5 again.
        session.streams.pay(&stream, 1).unwrap();
        let recovered = fetch_with(&mut session, &payer, &one, &chain, max, Some(&known)).unwrap();
        assert_eq!(position(&recovered), 5);

        // Five steps used up the escrow: the next call is refused, and the
        // client stays where the server is.
        let broke = fetch_with(&mut session, &payer, &one, &chain, max, Some(&known)).unwrap();
        assert!(
            refusal(&broke).contains("escrow or time does not cover"),
            "{broke:?}"
        );
        assert_eq!(session.streams.record(&stream).unwrap().revealed, 5);

        // The payer tops up; the desk reads the new terms and accepts.
        session
            .must(
                "agent",
                TxKind::TopUpStream {
                    stream,
                    deposit: Amount::units(1),
                    until,
                },
                Vec::new(),
            )
            .unwrap();
        std::thread::sleep(Duration::from_millis(600));
        let topped = fetch_with(&mut session, &payer, &one, &chain, max, Some(&known)).unwrap();
        assert_eq!(position(&topped), 6);

        // A price that is not a whole number of steps cannot be paid.
        assert!(fetch(&mut session, &payer, &format!("{base}/odd"), &chain, max).is_err());

        // A forged element at the right position, and a replayed one.
        let forged = PaymentPayload {
            x402_version: X402_VERSION,
            resource: None,
            accepted: known.clone(),
            payload: json!(Payment {
                stream,
                position: 7,
                value: Hash32([0xee; 32]),
            }),
            extensions: None,
        };
        let refused = replay(&one, &encode_header(&forged)).unwrap();
        assert!(refusal(&refused).contains("does not hash"), "{refused:?}");
        let replayed = replay(&one, topped.payment.as_deref().unwrap()).unwrap();
        assert!(
            refusal(&replayed).contains("expected the element at position 7"),
            "{replayed:?}"
        );

        // Requirements for another payee are refused before any hashing.
        let mut elsewhere = known.clone();
        elsewhere.pay_to = session.id("bob").to_string();
        let mut misdirected = forged.clone();
        misdirected.accepted = elsewhere;
        let refused = replay(&one, &encode_header(&misdirected)).unwrap();
        assert!(refusal(&refused).contains("paid to"), "{refused:?}");

        stop.store(true, Ordering::Relaxed);
        serving.join().unwrap();
    });

    // Stopping the server settled the six accepted steps.
    let report = facilitator.streams().unwrap().report();
    assert_eq!(report.payments, 4);
    assert_eq!(report.hashes, 6);
    assert_eq!(report.settled_steps, 6);
    assert_eq!(report.refused, 4);
    let settled = load_stream(&devnet, &stream).unwrap();
    assert_eq!(settled.position, 6);
    assert_eq!(settled.escrow, Amount(5 * unit.0 + 1_000_000 - 6 * unit.0));
    let carol_after = session.balance("carol", AssetId::usd()).unwrap();
    assert_eq!(
        carol_after.0,
        carol_before.0 + 6 * unit.0 - report.fees.0,
        "carol is paid six steps, less the settlement fees"
    );
    devnet.shutdown();
}
