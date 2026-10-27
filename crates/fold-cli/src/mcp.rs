//! A Model Context Protocol server, so AI agents can use the ledger.
//!
//! Speaks JSON-RPC 2.0 over stdio (one message per line) and exposes tools
//! to check balances, pay, issue claims, price a claim's trustworthiness
//! with the Assurance Algebra before relying on it, pay within an Agent
//! Charter (the ledger enforces the limits, whatever the agent was told),
//! submit an intent the agent signed itself, fetch a receipt verified the
//! way a light client would, and verify a Pulse. Every transaction and
//! receipt also comes back as a sentence.

use std::io::{BufRead, Write};

use fold_assurance::{Policy, Ratio};
use fold_core::encode::decode;
use fold_core::hash::Hash32;
use fold_core::ids::TxDigest;
use fold_core::ids::{AssetId, AtomId, ClaimId, ObjectId, Pulse};
use fold_core::Amount;
use fold_exec::claim::{claim_id, formulas, ClaimBody, Payload};
use fold_exec::{Event, SignedTransaction, TxKind};
use serde_json::{json, Value};

use crate::describe;
use crate::error::CliError;
use crate::ledger::{Ledger, LedgerExt};
use crate::session::Session;

/// The protocol version this server speaks.
pub const PROTOCOL_VERSION: &str = "2025-06-18";

fn tools() -> Value {
    let account = json!({ "type": "string", "description": "A wallet name (devnet) or a 64-digit hex account ID" });
    json!([
        {
            "name": "fold_status",
            "description": "Chain status: latest Pulse, certified Pulse, world commitment, transaction counts.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "fold_balance",
            "description": "Balances of an account.",
            "inputSchema": { "type": "object", "properties": { "account": account }, "required": ["account"] }
        },
        {
            "name": "fold_pay",
            "description": "Pay USD from a wallet this server holds keys for. Final once the lane certificate forms; executes in the next Pulse.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "from": { "type": "string", "description": "Wallet name" },
                    "to": account,
                    "amount_usd": { "type": "number", "description": "Amount in USD" }
                },
                "required": ["from", "to", "amount_usd"]
            }
        },
        {
            "name": "fold_issue_claim",
            "description": "Issue a claim (a statement with a trust formula). With bond_label the claim rests on the issuer's bond of that label; otherwise only on the issuer's dual post-quantum signature.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "issuer": { "type": "string" },
                    "subject": { "type": "string" },
                    "schema": { "type": "string" },
                    "bond_label": { "type": "string" },
                    "valid_for_pulses": { "type": "integer" }
                },
                "required": ["issuer", "subject"]
            }
        },
        {
            "name": "fold_assess_claim",
            "description": "Before relying on a claim: compute its cost of corruption under a policy and apply the decision rule min(δ·CoC, CoC(adjudicator)) ≥ κ·V. Returns accept/reject with reasons.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "claim": { "type": "string", "description": "Claim ID (hex)" },
                    "value_usd": { "type": "number", "description": "What you stand to lose if the claim is false" },
                    "kappa": { "type": "number", "description": "Safety factor κ (default 2)" },
                    "distrust_atoms": { "type": "array", "items": { "type": "string" }, "description": "Atom IDs to treat as worthless" }
                },
                "required": ["claim", "value_usd"]
            }
        },
        {
            "name": "fold_agent_pay",
            "description": "Pay from an Agent Charter allotment. The ledger enforces the charter's payees, per-payment and per-window limits, expiry and revocation; a receipt claim is issued if the charter requires it.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "agent": { "type": "string" },
                    "charter": { "type": "string" },
                    "allotment": { "type": "string" },
                    "payee": account,
                    "amount_usd": { "type": "number" },
                    "purpose": { "type": "string" }
                },
                "required": ["agent", "charter", "allotment", "payee", "amount_usd", "purpose"]
            }
        },
        {
            "name": "fold_verify_pulse",
            "description": "Verify a Pulse as a light client: chain identity, committee certificate, receipts, state proof, data-availability sampling, the World Proof and lane certificates. Pass the chain identity you trust as `chain` (64 hex digits); without it the node vouches for itself.",
            "inputSchema": { "type": "object", "properties": { "pulse": { "type": "integer" }, "chain": { "type": "string", "description": "Pinned chain identity (hex)" } } }
        },
        {
            "name": "fold_events",
            "description": "Recent events from a Pulse on.",
            "inputSchema": { "type": "object", "properties": { "from": { "type": "integer" }, "limit": { "type": "integer" } } }
        },
        {
            "name": "fold_submit_intent",
            "description": "Submit a transaction the agent built and signed itself (hex of its canonical encoding). Returns what it asks for as a sentence, then waits for its receipt unless wait is false.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "tx": { "type": "string", "description": "Hex-encoded signed transaction" },
                    "wait": { "type": "boolean", "description": "Wait for execution (default true)" }
                },
                "required": ["tx"]
            }
        },
        {
            "name": "fold_fetch_receipt",
            "description": "Fetch a transaction's receipt and verify it as a light client: the chain identity, the committee certificate of its Pulse and the receipt's inclusion proof. Pass the chain identity you trust as `chain`.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "digest": { "type": "string", "description": "Transaction digest (hex)" },
                    "chain": { "type": "string", "description": "Pinned chain identity (hex)" }
                },
                "required": ["digest"]
            }
        }
    ])
}

fn usd(value: &Value) -> Result<Amount, CliError> {
    let number = value
        .as_f64()
        .ok_or_else(|| CliError::Usage("amount must be a number".into()))?;
    if !(number.is_finite() && number > 0.0 && number < 1e12) {
        return Err(CliError::Usage("amount out of range".into()));
    }
    // Reporting boundary: convert to micro-units once, rounding to the micro-unit.
    Ok(Amount((number * 1_000_000.0).round() as u128))
}

fn hex_id(value: &Value, what: &str) -> Result<Hash32, CliError> {
    value
        .as_str()
        .and_then(|s| Hash32::from_hex(s).ok())
        .ok_or_else(|| CliError::Usage(format!("{what} must be a 64-digit hex identifier")))
}

fn call(session: &mut Session<'_>, name: &str, args: &Value) -> Result<Value, CliError> {
    match name {
        "fold_status" => session.ledger.status(),
        "fold_balance" => {
            let account = args["account"]
                .as_str()
                .ok_or_else(|| CliError::Usage("account required".into()))?;
            let id = session.id(account);
            let view = session.ledger.account(id)?;
            Ok(json!({
                "account": id,
                "balances": view.balances.iter().map(|(a, v)| json!({
                    "asset": if *a == AssetId::usd() { "USD".to_string() } else { a.to_string() },
                    "amount": v.to_string(),
                })).collect::<Vec<_>>(),
                "pulse": view.pulse,
            }))
        }
        "fold_pay" => {
            let from = args["from"]
                .as_str()
                .ok_or_else(|| CliError::Usage("from required".into()))?;
            let to = args["to"]
                .as_str()
                .ok_or_else(|| CliError::Usage("to required".into()))?;
            let to = session.id(to);
            let amount = usd(&args["amount_usd"])?;
            let view = session.send(
                from,
                TxKind::Transfer {
                    asset: AssetId::usd(),
                    outputs: vec![(to, amount)],
                },
                Vec::new(),
            )?;
            Ok(json!({
                "pulse": view.pulse,
                "status": view.status,
                "digest": view.effects.digest,
                "fee": view.effects.fee.to_string(),
                "receipt": describe::receipt(&session.names(), &view.effects),
            }))
        }
        "fold_issue_claim" => {
            let issuer = args["issuer"]
                .as_str()
                .ok_or_else(|| CliError::Usage("issuer required".into()))?;
            let subject = args["subject"]
                .as_str()
                .ok_or_else(|| CliError::Usage("subject required".into()))?;
            let id = session.id(issuer);
            let formula = match args["bond_label"].as_str() {
                Some(label) => formulas::bonded_statement(
                    fold_exec::execute::bond_atom(fold_assurance::AtomClass::Economic, &id, label)
                        .id,
                ),
                None => formulas::dual_signature(),
            };
            let body = ClaimBody {
                schema: args["schema"].as_str().unwrap_or("note/v1").to_string(),
                subject: subject.to_string(),
                payload: Payload::Public(subject.as_bytes().to_vec()),
                formula,
                adjudicator: None,
                detection_permille: 1000,
                valid_for: args["valid_for_pulses"].as_u64().unwrap_or(10_000),
                challenge_period: 100,
            };
            let claim = claim_id(&id, &body);
            let view = session.send(issuer, TxKind::IssueClaim { claim: body }, Vec::new())?;
            let issued = view
                .effects
                .events
                .iter()
                .any(|e| matches!(e, Event::ClaimSubmitted { .. }));
            Ok(json!({
                "claim": claim,
                "pulse": view.pulse,
                "status": view.status,
                "submitted": issued,
                "receipt": describe::receipt(&session.names(), &view.effects),
            }))
        }
        "fold_assess_claim" => {
            let claim = ClaimId::from_hash(hex_id(&args["claim"], "claim")?);
            let value = usd(&args["value_usd"])?;
            let kappa = args["kappa"].as_f64().unwrap_or(2.0);
            let mut policy =
                Policy::standard().with_kappa(Ratio::new((kappa * 1000.0).round() as u64, 1000));
            if let Some(list) = args["distrust_atoms"].as_array() {
                let atoms: Result<Vec<AtomId>, _> = list
                    .iter()
                    .map(|a| hex_id(a, "atom").map(AtomId::from_hash))
                    .collect();
                policy = policy.distrusting(atoms?);
            }
            let decision = session.ledger.decide(claim, value, policy)?;
            let record = session.ledger.claim(claim)?;
            Ok(
                json!({ "decision": decision, "claim": record["claim"]["body"]["subject"], "status": record["status"], "atoms": record["atoms"] }),
            )
        }
        "fold_agent_pay" => {
            let agent = args["agent"]
                .as_str()
                .ok_or_else(|| CliError::Usage("agent required".into()))?;
            let charter = ObjectId::from_hash(hex_id(&args["charter"], "charter")?);
            let allotment = ObjectId::from_hash(hex_id(&args["allotment"], "allotment")?);
            let payee = args["payee"]
                .as_str()
                .ok_or_else(|| CliError::Usage("payee required".into()))?;
            let payee = session.id(payee);
            let amount = usd(&args["amount_usd"])?;
            let purpose = args["purpose"].as_str().unwrap_or("").to_string();
            let input = session.reference(&allotment)?;
            let view = session.send(
                agent,
                TxKind::AgentPay {
                    charter,
                    payee,
                    amount,
                    purpose,
                    evidence: None,
                },
                vec![input],
            )?;
            Ok(json!({
                "pulse": view.pulse,
                "status": view.status,
                "digest": view.effects.digest,
                "events": view.effects.events,
                "receipt": describe::receipt(&session.names(), &view.effects),
            }))
        }
        "fold_verify_pulse" => {
            let pulse = args["pulse"].as_u64().map(Pulse);
            let pinned = match args["chain"].as_str() {
                Some(hex) => Some(
                    Hash32::from_hex(hex)
                        .map_err(|_| CliError::Usage("chain must be 64 hex digits".into()))?,
                ),
                None => None,
            };
            let report = crate::verify::verify_pulse(session.ledger, pinned, pulse, 16, true)?;
            Ok(json!({ "verified": true, "pinned": pinned.is_some(), "report": report }))
        }
        "fold_events" => {
            let from = Pulse(args["from"].as_u64().unwrap_or(0));
            let limit = args["limit"].as_u64().unwrap_or(50) as usize;
            Ok(
                serde_json::to_value(session.ledger.events(from, limit.min(500))?)
                    .unwrap_or(Value::Null),
            )
        }
        "fold_submit_intent" => {
            let bytes = args["tx"]
                .as_str()
                .and_then(|text| hex::decode(text.trim()).ok())
                .ok_or_else(|| CliError::Usage("tx must be hex".into()))?;
            let tx: SignedTransaction = decode(&bytes)
                .map_err(|_| CliError::Usage("tx is not a signed transaction".into()))?;
            let names = session.names();
            let sentence = describe::sentence(&names, &tx.tx);
            let submitted = session.ledger.submit(&tx)?;
            if !args["wait"].as_bool().unwrap_or(true) {
                return Ok(json!({
                    "digest": submitted.digest,
                    "route": submitted.route,
                    "intent": sentence,
                }));
            }
            let view = session.wait(submitted.digest)?;
            Ok(json!({
                "digest": submitted.digest,
                "intent": sentence,
                "pulse": view.pulse,
                "status": view.status,
                "receipt": describe::receipt(&names, &view.effects),
            }))
        }
        "fold_fetch_receipt" => {
            let digest = TxDigest::from_hash(hex_id(&args["digest"], "digest")?);
            let pinned = match args["chain"].as_str() {
                Some(hex) => Some(
                    Hash32::from_hex(hex)
                        .map_err(|_| CliError::Usage("chain must be 64 hex digits".into()))?,
                ),
                None => None,
            };
            let verified = crate::verify::verify_receipt_of(session.ledger, pinned, digest)?;
            Ok(json!({
                "digest": digest,
                "pulse": verified.pulse,
                "receipt": describe::receipt(&session.names(), &verified.effects),
                "outcome": match verified.effects.status {
                    fold_exec::ExecStatus::Success => "success",
                    fold_exec::ExecStatus::Failed(_) => "failed",
                    fold_exec::ExecStatus::Rejected(_) => "rejected",
                },
                "events": verified.effects.events,
                "certificate": verified.signed.map(|(stake, quorum)| json!({ "signed": stake, "quorum": quorum })),
                "chain": verified.chain.0,
                "chain_pinned": verified.chain.1,
                "verified": true,
            }))
        }
        other => Err(CliError::Usage(format!("unknown tool {other}"))),
    }
}

/// Handles one JSON-RPC message; returns the response, if any.
pub fn handle(session: &mut Session<'_>, message: &Value) -> Option<Value> {
    let id = message.get("id").cloned();
    let method = message["method"].as_str().unwrap_or("");
    let result = match method {
        "initialize" => Ok(json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": { "tools": { "listChanged": false } },
            "serverInfo": { "name": "fold", "version": env!("CARGO_PKG_VERSION") },
            "instructions": "Tools for the Fold neo-blockchain devnet. Assess a claim with fold_assess_claim before relying on it."
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tools() })),
        "tools/call" => {
            let name = message["params"]["name"].as_str().unwrap_or("");
            let args = message["params"]["arguments"].clone();
            Ok(match call(session, name, &args) {
                Ok(value) => json!({
                    "content": [{ "type": "text", "text": serde_json::to_string_pretty(&value).unwrap_or_default() }],
                    "structuredContent": value,
                    "isError": false
                }),
                Err(error) => json!({
                    "content": [{ "type": "text", "text": error.to_string() }],
                    "isError": true
                }),
            })
        }
        _ if method.starts_with("notifications/") => return None,
        _ => Err(json!({ "code": -32601, "message": format!("method not found: {method}") })),
    };
    let id = id?;
    Some(match result {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err(error) => json!({ "jsonrpc": "2.0", "id": id, "error": error }),
    })
}

/// Serves MCP on stdin/stdout until stdin closes.
pub fn serve(ledger: &dyn Ledger) -> Result<(), CliError> {
    let mut session = Session::open(ledger)?;
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Value>(&line) {
            Ok(message) => handle(&mut session, &message),
            Err(e) => Some(
                json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": e.to_string() } }),
            ),
        };
        if let Some(response) = response {
            writeln!(stdout, "{response}").map_err(|e| CliError::Usage(e.to_string()))?;
            stdout.flush().map_err(|e| CliError::Usage(e.to_string()))?;
        }
    }
    Ok(())
}
