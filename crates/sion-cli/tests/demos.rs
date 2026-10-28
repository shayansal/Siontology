//! Every demo runs end to end on a real in-process devnet, and the MCP
//! server answers an agent's session.
//!
//! The demo runs take minutes, so they are `#[ignore]`d: CI runs them every
//! night (`.github/workflows/nightly.yml`), and locally
//! `cargo test --workspace -- --include-ignored` runs them with the rest.

use serde_json::{json, Value};
use sion_cli::demos::{self, Narrator};
use sion_cli::{mcp, Session};
use sion_node::{Devnet, DevnetConfig};

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
#[ignore = "slow (minutes): runs nightly, or with --include-ignored"]
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
    for expected in [
        "2,000 calls paid from the stream in",
        "and sent no transaction per call",
        "carol settled 2,000 payments in",
        "proven by a jump to a checkpoint), not one per payment",
        "call 1,234's receipt: genuine and settled, in 766 hashes from the settled head",
        "a forged receipt for the same call → not an element of the stream's chain",
        "the agent gets 18 USD back",
    ] {
        assert!(
            text.contains(expected),
            "streams: missing {expected:?}:\n{text}"
        );
    }
    for expected in [
        "the orchestrator launches 12 researchers in one transaction",
        "researcher-1 launches 3 sub-researchers of its own (depth 2)",
        "15 subagents each pay gpu-cloud for a tool call, all at once",
        "payment 6 above per-payment limit 5",
        "two levels down, tries to pay 3 USD → refused by the ledger: charter caveat violated: charter suspended",
        "acme reclaims researcher-2's unspent budget: 32 USD",
        "16 identities created in 3 transactions",
    ] {
        assert!(
            text.contains(expected),
            "swarm: missing {expected:?}:\n{text}"
        );
    }
    for expected in [
        "coder claims the 150 USD on its own 100 USD bond → refused by the ledger",
        "is below κ·V = 300",
        "→ paid 150 USD; 300 USD of that backing reserved for the lab",
        "a task is paid once",
        "bounty-hunter settles the open bounty with reviewer-2's attestation → paid 40 USD",
        "the orchestrator withdraws an unclaimed bounty: 30 USD back to the lab",
        "the lab is compensated 190 USD from the vendor's bond",
    ] {
        assert!(
            text.contains(expected),
            "tasks: missing {expected:?}:\n{text}"
        );
    }
    for expected in [
        "fleet gives robot-alpha and robot-beta agent identities of their own",
        "everything it recorded in aisle 4 during that minute: 1 atom, with a",
        "and for aisle 9: 0 atoms, with a proof that it recorded nothing there",
        "robot-alpha tries to replace its observation with one in aisle 5: refused by the ledger",
        "equal sizes require equal roots",
        "a reader trusting the curator believes the observation refuted; one trusting only robot-alpha believes it accepted",
        "anyone holding that scan proves the omission from the head on the ledger alone",
        "(readable: true), then forgets it by destroying the key (readable: false)",
        "its next commit is refused by the ledger: charter caveat violated: charter suspended",
    ] {
        assert!(
            text.contains(expected),
            "robots: missing {expected:?}:\n{text}"
        );
    }
    for expected in [
        "the head's manifest is the root of their archive",
        "robot-gamma posts an open deal for the archive",
        "laptop accepted storage deal",
        "the phone is asked for leaves",
        "robot-gamma closes the footage deal",
        "forfeited to the treasury",
        "the laptop proved",
        "the memory the robot committed is the data that was kept",
    ] {
        assert!(
            text.contains(expected),
            "storage: missing {expected:?}:
{text}"
        );
    }
    assert!(
        text.contains("this devnet has no standby validator"),
        "the staking demo explains what it needs: {text}"
    );
    devnet.shutdown();
}

#[test]
#[ignore = "slow (about a minute): runs nightly, or with --include-ignored"]
fn validators_join_earn_get_slashed_and_light_clients_follow() {
    let _serial = serial();
    // No prover: the staking demo checks certificates and epoch proofs,
    // not World Proofs, and small machines have few cores.
    let devnet = Devnet::start(DevnetConfig {
        rounds_per_pulse: 2,
        prove_chunk: None,
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
            &json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": { "name": "sion_status", "arguments": {} } }),
        )
        .unwrap();
        assert_eq!(status["result"]["isError"], false);
        let bad = mcp::handle(
            &mut session,
            &json!({ "jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": { "name": "sion_pay", "arguments": { "from": "nobody", "to": "alice", "amount_usd": 5 } } }),
        )
        .unwrap();
        assert_eq!(
            bad["result"]["isError"], true,
            "an unfunded wallet cannot pay"
        );

        // An agent that signs its own transactions submits an intent, reads
        // it back as a sentence, and fetches a receipt it can verify.
        let bob = sion_exec::Wallet::named("bob").id;
        session
            .fund(
                "alice",
                sion_core::ids::AssetId::usd(),
                sion_core::Amount::units(10),
            )
            .unwrap();
        let tx = {
            let view = sion_cli::session::RemoteView::new(&devnet, session.config.clone());
            session
                .wallet("alice")
                .build(
                    &view,
                    sion_exec::TxKind::Transfer {
                        asset: sion_core::ids::AssetId::usd(),
                        outputs: vec![(bob, sion_core::Amount::units(3))],
                    },
                    Vec::new(),
                )
                .unwrap()
        };
        let submitted = mcp::handle(
            &mut session,
            &json!({ "jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": { "name": "sion_submit_intent", "arguments": { "tx": hex::encode(sion_core::encode::encode(&tx)) } } }),
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
                &json!({ "jsonrpc": "2.0", "id": 6, "method": "tools/call", "params": { "name": "sion_fetch_receipt", "arguments": { "digest": digest, "chain": chain } } }),
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
        use sion_cli::x402::{Facilitator, PaidRoute, PaidServer};
        let carol = sion_exec::Wallet::named("carol").id;
        let facilitator = Facilitator::new(&devnet, devnet.shared.chain_id).unwrap();
        let server = PaidServer::bind("127.0.0.1:0").unwrap();
        let routes = [PaidRoute {
            path: "/quote".into(),
            price: sion_core::Amount(500_000),
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
                    &json!({ "jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": { "name": "sion_fetch_paid", "arguments": { "url": url, "wallet": "alice", "max_usd": max, "chain": chain } } }),
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
