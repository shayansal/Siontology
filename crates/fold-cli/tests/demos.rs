//! Every demo runs end to end on a real in-process devnet, and the MCP
//! server answers an agent's session.

use fold_cli::demos::{self, Narrator};
use fold_cli::{mcp, Session};
use fold_node::{Devnet, DevnetConfig};
use serde_json::{json, Value};

/// The tests run one devnet at a time: each runs several validator threads
/// and a prover, and small machines run out of cores.
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn devnet() -> Devnet {
    Devnet::start(DevnetConfig {
        rounds_per_pulse: 2,
        prove_chunk: Some(8),
        ..DevnetConfig::default()
    })
    .unwrap()
}

/// Runs a demo; on failure, shows what it had narrated so far.
fn run(devnet: &Devnet, name: &str) -> String {
    let mut out = Narrator::default();
    let result = {
        let mut session = Session::open(devnet).unwrap();
        demos::run(name, &mut session, &mut out)
    };
    let text = out.lines.join("\n");
    if let Err(error) = result {
        panic!("demo {name} failed: {error}\n{text}");
    }
    // Shown with `cargo test -- --nocapture`.
    println!("{text}");
    text
}

#[test]
fn all_demos_run_and_tell_the_truth() {
    let _serial = serial();
    let devnet = devnet();
    let text = run(&devnet, "all");
    assert!(
        text.contains("cost of corruption (standard policy): 50,000 USD, exact"),
        "{text}"
    );
    assert!(text.contains("financier compensated 10,000 USD"), "{text}");
    assert!(text.contains("payee not allowed"), "{text}");
    assert!(text.contains("charter revoked"), "{text}");
    assert!(text.contains("gpu-cloud received 100 USD"), "{text}");
    assert!(text.contains("at one price 1.95 USD"), "{text}");
    assert!(
        text.contains("assumes TEE broken relying on 3000 USD → REJECT"),
        "{text}"
    );
    assert!(
        text.contains("certificate: 3 of 4 validators signed"),
        "{text}"
    );
    assert!(text.contains("world proof:"), "{text}");
    assert!(
        text.contains("not executed; the gateway acknowledged it and did nothing"),
        "{text}"
    );
    assert!(text.contains("posted as a forced intent"), "{text}");
    assert!(
        text.contains("light client: forced inclusion: 1 intent(s) OVERDUE"),
        "{text}"
    );
    assert!(
        text.contains("the state replaces shard"),
        "a missed deadline rotates the committee: {text}"
    );
    assert!(text.contains("censorship lifted"), "{text}");
    assert!(
        text.contains("→ 402 Payment Required: 0.25 USD to carol"),
        "{text}"
    );
    assert!(text.contains(r#"200 OK {"region":"eu-west""#), "{text}");
    assert!(
        text.contains(
            "replaying the same PAYMENT-SIGNATURE → 402: unknown quote, or one already used"
        ),
        "{text}"
    );
    assert!(
        text.contains("payment 5 above per-payment limit 1; carol's API answers 402"),
        "{text}"
    );
    assert!(
        text.contains("pays the same API with a plain transfer → 200 OK"),
        "{text}"
    );
    assert!(
        text.contains("this devnet has no standby validator"),
        "the staking demo explains what it needs: {text}"
    );
    devnet.shutdown();
}

#[test]
fn validators_join_earn_get_slashed_and_light_clients_follow() {
    let _serial = serial();
    let devnet = Devnet::start(DevnetConfig {
        rounds_per_pulse: 2,
        prove_chunk: Some(8),
        standby: 1,
        epoch_length: 24,
        unbonding: 8,
        ..DevnetConfig::default()
    })
    .unwrap();
    let text = run(&devnet, "staking");
    for expected in [
        "validator",
        "registered by operator-4 with 50,000 USD at stake",
        "5 validators hold seats, the newcomer among them",
        "light client: followed the committee from genesis through",
        "slashed: 50,000 USD forfeited, 2,500 USD of it to watchdog",
        "the slashed validator lost its seat",
        "light client: still following",
    ] {
        assert!(text.contains(expected), "missing {expected:?}:\n{text}");
    }
    devnet.shutdown();
}

#[test]
fn the_mcp_server_serves_an_agent() {
    let _serial = serial();
    let devnet = devnet();
    {
        let mut session = Session::open(&devnet).unwrap();
        let init = mcp::handle(
            &mut session,
            &json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {} }),
        )
        .unwrap();
        assert_eq!(init["result"]["protocolVersion"], mcp::PROTOCOL_VERSION);
        assert!(mcp::handle(
            &mut session,
            &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })
        )
        .is_none());
        let list = mcp::handle(
            &mut session,
            &json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
        )
        .unwrap();
        assert!(list["result"]["tools"].as_array().unwrap().len() >= 8);
        let status = mcp::handle(
            &mut session,
            &json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": { "name": "fold_status", "arguments": {} } }),
        )
        .unwrap();
        assert_eq!(status["result"]["isError"], false);
        let bad = mcp::handle(
            &mut session,
            &json!({ "jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": { "name": "fold_pay", "arguments": { "from": "nobody", "to": "alice", "amount_usd": 5 } } }),
        )
        .unwrap();
        assert_eq!(
            bad["result"]["isError"], true,
            "an unfunded wallet cannot pay"
        );

        // An agent that signs its own transactions submits an intent, reads
        // it back as a sentence, and fetches a receipt it can verify.
        let bob = fold_exec::Wallet::named("bob").id;
        session
            .fund(
                "alice",
                fold_core::ids::AssetId::usd(),
                fold_core::Amount::units(10),
            )
            .unwrap();
        let tx = {
            let view = fold_cli::session::RemoteView::new(&devnet, session.config.clone());
            session
                .wallet("alice")
                .build(
                    &view,
                    fold_exec::TxKind::Transfer {
                        asset: fold_core::ids::AssetId::usd(),
                        outputs: vec![(bob, fold_core::Amount::units(3))],
                    },
                    Vec::new(),
                )
                .unwrap()
        };
        let submitted = mcp::handle(
            &mut session,
            &json!({ "jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": { "name": "fold_submit_intent", "arguments": { "tx": hex::encode(fold_core::encode::encode(&tx)) } } }),
        )
        .unwrap();
        let result = &submitted["result"]["structuredContent"];
        assert_eq!(submitted["result"]["isError"], false, "{submitted}");
        assert!(
            result["intent"]
                .as_str()
                .unwrap()
                .starts_with("Pay 3 USD to bob."),
            "{result}"
        );
        assert!(result["receipt"]
            .as_str()
            .unwrap()
            .starts_with("Done: paid 3 USD to bob"));
        let digest = result["digest"].clone();
        let chain = devnet.shared.chain_id.to_hex();
        // The receipt is fetched once its Pulse is certified.
        let mut fetched = Value::Null;
        for _ in 0..100 {
            fetched = mcp::handle(
                &mut session,
                &json!({ "jsonrpc": "2.0", "id": 6, "method": "tools/call", "params": { "name": "fold_fetch_receipt", "arguments": { "digest": digest, "chain": chain } } }),
            )
            .unwrap();
            if fetched["result"]["isError"] == false {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let receipt = &fetched["result"]["structuredContent"];
        assert_eq!(receipt["verified"], true, "{fetched}");
        assert_eq!(receipt["chain_pinned"], true);
        assert_eq!(receipt["outcome"], "success");

        // An agent fetches a resource that charges through x402: it pays
        // within its own cap and gets the body; a resource above the cap is
        // not paid for.
        use fold_cli::x402::{Facilitator, PaidRoute, PaidServer};
        let carol = fold_exec::Wallet::named("carol").id;
        let facilitator = Facilitator::new(&devnet, devnet.shared.chain_id).unwrap();
        let server = PaidServer::bind("127.0.0.1:0").unwrap();
        let routes = [PaidRoute {
            path: "/quote".into(),
            price: fold_core::Amount(500_000),
            description: "a quote".into(),
            mime_type: "text/plain".into(),
            body: Box::new(|_| "42".into()),
        }];
        let stop = std::sync::atomic::AtomicBool::new(false);
        let url = format!("http://{}/quote", server.address);
        std::thread::scope(|scope| {
            let serving = scope.spawn(|| {
                server.serve(
                    &facilitator,
                    carol,
                    &routes,
                    std::time::Duration::from_secs(60),
                    &stop,
                )
            });
            let call = |session: &mut Session<'_>, max: f64| {
                mcp::handle(
                    session,
                    &json!({ "jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": { "name": "fold_fetch_paid", "arguments": { "url": url, "wallet": "alice", "max_usd": max, "chain": chain } } }),
                )
                .unwrap()
            };
            let refused = call(&mut session, 0.25);
            assert_eq!(refused["result"]["isError"], true, "{refused}");
            let paid = call(&mut session, 1.0);
            stop.store(true, std::sync::atomic::Ordering::Relaxed);
            serving.join().unwrap();
            let result = &paid["result"]["structuredContent"];
            assert_eq!(paid["result"]["isError"], false, "{paid}");
            assert_eq!(result["status"], 200, "{result}");
            assert_eq!(result["body"], "42");
            assert_eq!(result["paid"], true);
        });
    }
    devnet.shutdown();
}
