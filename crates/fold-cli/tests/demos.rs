//! Every demo runs end to end on a real in-process devnet, and the MCP
//! server answers an agent's session.

use fold_cli::demos::{self, Narrator};
use fold_cli::{mcp, Session};
use fold_node::{Devnet, DevnetConfig};
use serde_json::json;

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
    }
    devnet.shutdown();
}
