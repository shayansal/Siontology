//! Every demo runs end to end on a real in-process devnet, and the MCP
//! server answers an agent's session.

use fold_cli::demos::{self, Narrator};
use fold_cli::{mcp, Session};
use fold_node::{Devnet, DevnetConfig};
use serde_json::{json, Value};

fn devnet() -> Devnet {
    Devnet::start(DevnetConfig {
        rounds_per_pulse: 2,
        prove_chunk: Some(8),
        ..DevnetConfig::default()
    })
    .unwrap()
}

#[test]
fn all_demos_run_and_tell_the_truth() {
    let devnet = devnet();
    let mut out = Narrator::default();
    {
        let mut session = Session::open(&devnet).unwrap();
        demos::run("all", &mut session, &mut out).unwrap();
    }
    let text = out.lines.join("\n");
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
    devnet.shutdown();
}

#[test]
fn the_mcp_server_serves_an_agent() {
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
    }
    devnet.shutdown();
}
