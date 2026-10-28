//! A Model Context Protocol server, so AI agents can use the ledger.
//!
//! Speaks JSON-RPC 2.0 over stdio (one message per line) and exposes tools
//! to check balances, pay, issue claims, price a claim's trustworthiness
//! with the Assurance Algebra before relying on it, pay within an Agent
//! Charter (the ledger enforces the limits, whatever the agent was told),
//! submit an intent the agent signed itself, fetch a receipt verified the
//! way a light client would, verify a Pulse, and fetch HTTP resources that
//! charge through x402. Every transaction and receipt also comes back as a
//! sentence.

use std::io::{BufRead, Write};

use serde_json::{json, Value};
use sion_assurance::{Policy, Ratio};
use sion_core::encode::decode;
use sion_core::hash::Hash32;
use sion_core::ids::TxDigest;
use sion_core::ids::{AssetId, AtomId, ClaimId, ObjectId, Pulse};
use sion_core::Amount;
use sion_exec::claim::{claim_id, formulas, ClaimBody, Payload};
use sion_exec::{Event, SignedTransaction, TxKind};

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
            "name": "sion_status",
            "description": "Chain status: latest Pulse, certified Pulse, world commitment, transaction counts.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "sion_balance",
            "description": "Balances of an account.",
            "inputSchema": { "type": "object", "properties": { "account": account }, "required": ["account"] }
        },
        {
            "name": "sion_pay",
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
            "name": "sion_issue_claim",
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
            "name": "sion_assess_claim",
            "description": "Before relying on a claim: compute its cost of corruption under a policy and apply the decision rule min(δ·CoC, CoC(adjudicator)) ≥ κ·V. Returns accept/reject with reasons.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "claim": { "type": "string", "description": "Claim ID (hex)" },
                    "value_usd": { "type": "number", "description": "What you stand to lose if the claim is false" },
                    "kappa": { "type": "number", "description": "Safety factor κ (default 2)" },
                    "distrust_atoms": { "type": "array", "items": { "type": "string" }, "description": "Atom IDs not to trust even if their class is trusted: they count only for what breaking them costs (slashable bonds plus attack cost), not as unbreakable" }
                },
                "required": ["claim", "value_usd"]
            }
        },
        {
            "name": "sion_agent_pay",
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
            "name": "sion_verify_pulse",
            "description": "Verify a Pulse as a light client: chain identity, committee certificate, receipts, state proof, data-availability sampling, the World Proof and lane certificates. Pass the chain identity you trust as `chain` (64 hex digits); without it the node vouches for itself.",
            "inputSchema": { "type": "object", "properties": { "pulse": { "type": "integer" }, "chain": { "type": "string", "description": "Pinned chain identity (hex)" } } }
        },
        {
            "name": "sion_events",
            "description": "Recent events from a Pulse on.",
            "inputSchema": { "type": "object", "properties": { "from": { "type": "integer" }, "limit": { "type": "integer" } } }
        },
        {
            "name": "sion_submit_intent",
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
            "name": "sion_fetch_paid",
            "description": "Fetch an HTTP resource that may charge through x402 (HTTP 402 Payment Required). If the server asks for payment on this chain of at most max_usd, pay it and retry: under an Agent Charter when charter and allotment are given (the ledger enforces the charter's payees and limits, whatever the agent was told), otherwise by transfer from the wallet. Returns the body and the settlement.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "The resource" },
                    "wallet": { "type": "string", "description": "The paying wallet (the agent's, when paying under a charter)" },
                    "charter": { "type": "string", "description": "Charter ID (hex), to pay under a charter" },
                    "allotment": { "type": "string", "description": "The agent's allotment ID (hex)" },
                    "max_usd": { "type": "number", "description": "The most this call may pay" },
                    "chain": { "type": "string", "description": "Pinned chain identity (hex); without it the node vouches for itself" }
                },
                "required": ["url", "wallet", "max_usd"]
            }
        },
        {
            "name": "sion_task_post",
            "description": "Post a task (Task Charters): escrow money for work whose acceptance rule is fixed now. A result is paid only if it rests on every required claim (for example an environment vendor's bonded 'no known exploit' claim) and on the worker's bond or a named verifier's attestation, and passes the poster's decision rule; κ times the payment of its backing is then reserved, so a result later proven false compensates the poster. Without worker, the task is a bounty anyone may settle. With charter and allotment, the escrow comes from that agent's budget under its caveats.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "wallet": { "type": "string", "description": "The poster's wallet (the agent's, with charter)" },
                    "amount_usd": { "type": "number", "description": "Escrow, paid in full for an accepted result" },
                    "worker": { "type": "string", "description": "The worker (wallet or account); omit for an open bounty" },
                    "requires": { "type": "array", "items": { "type": "string" }, "description": "Claim ids every result must rest on" },
                    "verifiers": { "type": "array", "items": { "type": "string" }, "description": "Verifiers whose attestations count (wallets or accounts)" },
                    "adjudicator": { "type": "string", "description": "Adjudicator atom id (hex) results must name" },
                    "deadline_pulses": { "type": "integer", "description": "Pulses until the deadline (default 1000)" },
                    "detection_permille": { "type": "integer", "description": "Largest detection factor results may declare (default 900)" },
                    "reliance_pulses": { "type": "integer", "description": "Pulses the backing of a paid result stays reserved (default 1000)" },
                    "kappa": { "type": "number", "description": "Safety factor κ (default 2)" },
                    "spec": { "type": "string", "description": "The task's description; its bytes are committed" },
                    "charter": { "type": "string", "description": "Post from this agent's budget (charter id, hex)" },
                    "allotment": { "type": "string", "description": "The agent's allotment (hex), with charter" }
                },
                "required": ["wallet", "amount_usd"]
            }
        },
        {
            "name": "sion_task_attest",
            "description": "Attest, as a verifier the task names, that an output passes: issues an attestation claim resting on the verifier's bond bond:<verifier>:verifier (or bond_label).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "wallet": { "type": "string" },
                    "task": { "type": "string", "description": "Task id (hex)" },
                    "output": { "type": "string", "description": "The output; its bytes are committed" },
                    "bond_label": { "type": "string" }
                },
                "required": ["wallet", "task", "output"]
            }
        },
        {
            "name": "sion_task_submit",
            "description": "Submit a result for a task: issues a result claim with exactly the formula the task requires, over the attestations given (claim ids, in order). The worker needs a task bond (bond:<worker>:task-worker), however small.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "wallet": { "type": "string" },
                    "task": { "type": "string", "description": "Task id (hex)" },
                    "output": { "type": "string", "description": "The output; its bytes are committed" },
                    "attestations": { "type": "array", "items": { "type": "string" } }
                },
                "required": ["wallet", "task", "output"]
            }
        },
        {
            "name": "sion_task_settle",
            "description": "Settle a task with your result claim and its attestations (claim ids, in the order the result lists them). The ledger pays only if the result passes the rule fixed when the task was posted, and only once.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "wallet": { "type": "string" },
                    "task": { "type": "string", "description": "Task id (hex)" },
                    "result": { "type": "string", "description": "Result claim id (hex)" },
                    "attestations": { "type": "array", "items": { "type": "string" } }
                },
                "required": ["wallet", "task", "result"]
            }
        },
        {
            "name": "sion_task_show",
            "description": "Show a task: poster, worker, escrow, deadline, the acceptance rule and its state.",
            "inputSchema": { "type": "object", "properties": { "task": { "type": "string" } }, "required": ["task"] }
        },
        {
            "name": "sion_memory_open",
            "description": "Open a memory log on the ledger: an append-only log of memory atoms kept off the ledger, whose head the ledger moves only along proofs that the log grew. With agent, the log belongs to that agent identity (the wallet must be its agent) and cannot grow while the agent is suspended.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "wallet": { "type": "string" },
                    "agent": { "type": "string", "description": "Agent identity id (hex)" }
                },
                "required": ["wallet"]
            }
        },
        {
            "name": "sion_memory_remember",
            "description": "Record a memory atom in a log's local store: a statement (subject, predicate, value) with when it held and, for robots, where (a box in a frame, in millimetres). It reaches the ledger's commitment at the next sion_memory_commit.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "wallet": { "type": "string" },
                    "log": { "type": "string", "description": "Memory log id (hex) or a unique prefix" },
                    "subject": { "type": "string" },
                    "predicate": { "type": "string" },
                    "value": { "type": "string", "description": "true/false, an integer, @name for an entity, or text" },
                    "kind": { "type": "string", "description": "observation (default), coverage, world-state, semantic, episodic, procedural, preference, goal, plan, …" },
                    "frame": { "type": "string", "description": "The frame its place is in, for example warehouse-3" },
                    "at": { "type": "array", "items": { "type": "integer" }, "description": "Its place: x, y, z in millimetres" },
                    "size": { "type": "array", "items": { "type": "integer" }, "description": "Half the size of its box: dx, dy, dz in millimetres" },
                    "from_s": { "type": "integer", "description": "When it began to hold, seconds since the Unix epoch (default now)" },
                    "seconds": { "type": "integer", "description": "For how long it held (default open-ended)" },
                    "confidence_permille": { "type": "integer", "description": "Confidence in thousandths (default 1000)" }
                },
                "required": ["wallet", "log", "subject", "predicate", "value"]
            }
        },
        {
            "name": "sion_memory_commit",
            "description": "Commit a memory log's local head to the ledger with a proof that it extends the committed one. The Pulse it executes in becomes the transaction time of the atoms it adds.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "wallet": { "type": "string" },
                    "log": { "type": "string", "description": "Memory log id (hex) or a unique prefix" },
                    "seal": { "type": "boolean", "description": "Close the log for good" }
                },
                "required": ["wallet", "log"]
            }
        },
        {
            "name": "sion_memory_recall",
            "description": "Everything a memory log had recorded in a box of a frame during an interval, as of its committed head, with a proof of completeness checked against the index root the ledger holds (an empty answer is proved too).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "log": { "type": "string", "description": "Memory log id (hex) or a unique prefix" },
                    "frame": { "type": "string" },
                    "min": { "type": "array", "items": { "type": "integer" }, "description": "Lower corner x, y, z in millimetres" },
                    "max": { "type": "array", "items": { "type": "integer" }, "description": "Upper corner x, y, z in millimetres" },
                    "from_s": { "type": "integer", "description": "Interval start, seconds since the Unix epoch" },
                    "until_s": { "type": "integer", "description": "Interval end (exclusive)" }
                },
                "required": ["log", "frame", "min", "max", "from_s", "until_s"]
            }
        },
        {
            "name": "sion_stake_pools",
            "description": "Every validator with its own stake, its pool of delegated stake (and how much of it earns), its commission and its weight in the active set.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "sion_stake_delegate",
            "description": "Delegate stake to a validator's pool without handing it over: the wallet keeps a delegation object holding pool shares. It counts towards the validator's weight and earns a share of its rewards, less the validator's commission, from the end of this epoch; it shares the validator's slashing risk (a tenth of the pool on equivocation). Refused if it would give the validator more than twice an equal share of the bonded stake.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "wallet": { "type": "string" },
                    "validator": { "type": "string", "description": "Validator id (hex) or a unique prefix" },
                    "amount": { "type": "number", "description": "Stake, in the stake asset (at least 1)" }
                },
                "required": ["wallet", "validator", "amount"]
            }
        },
        {
            "name": "sion_stake_show",
            "description": "An account's delegations: validator, shares, the stake behind them, state (pending, earning or unbonding) and the rewards owed now.",
            "inputSchema": { "type": "object", "properties": { "account": account }, "required": ["account"] }
        },
        {
            "name": "sion_stake_claim",
            "description": "Collect a delegation's rewards (USD).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "wallet": { "type": "string" },
                    "delegation": { "type": "string", "description": "Delegation id (hex) or a unique prefix" }
                },
                "required": ["wallet", "delegation"]
            }
        },
        {
            "name": "sion_stake_undelegate",
            "description": "Stop a delegation earning: collect its rewards and start unbonding. The stake stays slashable until it is withdrawn after the unbonding period.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "wallet": { "type": "string" },
                    "delegation": { "type": "string", "description": "Delegation id (hex) or a unique prefix" }
                },
                "required": ["wallet", "delegation"]
            }
        },
        {
            "name": "sion_stake_withdraw",
            "description": "Withdraw an unbonded delegation's stake.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "wallet": { "type": "string" },
                    "delegation": { "type": "string", "description": "Delegation id (hex) or a unique prefix" }
                },
                "required": ["wallet", "delegation"]
            }
        },
        {
            "name": "sion_storage_post",
            "description": "Have a file kept by a storage provider (spec v1.12): escrows price × rounds for data whose Merkle root the deal fixes. A provider that accepts posts collateral and must prove every round that it holds leaves picked at random; it is paid per proven round, and loses collateral for missed rounds (all of it if it abandons the deal). Without a provider, any provider may accept, and the deal must ask for collateral. Proofs publish the challenged leaves: encrypt private data first. Returns the deal id.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "wallet": { "type": "string" },
                    "path": { "type": "string", "description": "The file to have kept" },
                    "price": { "type": "number", "description": "USD paid per proven round" },
                    "rounds": { "type": "integer", "description": "Rounds (1 to 1,000,000)" },
                    "round_length": { "type": "integer", "description": "Pulses per round: a multiple of 8, from 8 to 100,000 (default 16)" },
                    "collateral": { "type": "number", "description": "USD the provider must post (default: the escrow for an open deal, 0 for a named provider)" },
                    "challenges": { "type": "integer", "description": "Leaves challenged per round (1 to 16; default 4)" },
                    "provider": { "type": "string", "description": "Wallet name or account of the provider; omit for an open deal" },
                    "locator": { "type": "string", "description": "Where providers fetch the data: an https:// URL serving the file" },
                    "local": { "type": "boolean", "description": "Without a locator: point providers at the file itself (file://<path>), for a provider on this machine; the deal publishes the path" }
                },
                "required": ["wallet", "path", "price", "rounds"]
            }
        },
        {
            "name": "sion_storage_deals",
            "description": "Storage deals with their terms, state and, once accepted, the provider's progress (rounds proven, the round under way, the end).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "open": { "type": "boolean", "description": "Only deals still open for acceptance" },
                    "deal": { "type": "string", "description": "One deal (id or unique prefix)" }
                }
            }
        },
        {
            "name": "sion_storage_close",
            "description": "Close a storage deal: the client refunds an open deal, or ends an accepted one once it is over or once its provider has missed three rounds in a row (the provider then forfeits its collateral to the treasury); the provider may close one that is over. Pays the provider for proven rounds and refunds the escrow of unproven ones.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "wallet": { "type": "string" },
                    "deal": { "type": "string", "description": "Deal id (hex) or a unique prefix" }
                },
                "required": ["wallet", "deal"]
            }
        },
        {
            "name": "sion_fetch_receipt",
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

fn text_arg(args: &Value, key: &str) -> Result<String, CliError> {
    args[key]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| CliError::Usage(format!("{key} required")))
}

fn hex_id(value: &Value, what: &str) -> Result<Hash32, CliError> {
    value
        .as_str()
        .and_then(|s| Hash32::from_hex(s).ok())
        .ok_or_else(|| CliError::Usage(format!("{what} must be a 64-digit hex identifier")))
}

fn call(session: &mut Session<'_>, name: &str, args: &Value) -> Result<Value, CliError> {
    match name {
        "sion_status" => session.ledger.status(),
        "sion_balance" => {
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
        "sion_pay" => {
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
        "sion_issue_claim" => {
            let issuer = args["issuer"]
                .as_str()
                .ok_or_else(|| CliError::Usage("issuer required".into()))?;
            let subject = args["subject"]
                .as_str()
                .ok_or_else(|| CliError::Usage("subject required".into()))?;
            let id = session.id(issuer);
            let formula = match args["bond_label"].as_str() {
                Some(label) => formulas::bonded_statement(
                    sion_exec::execute::bond_atom(sion_assurance::AtomClass::Economic, &id, label)
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
        "sion_assess_claim" => {
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
        "sion_agent_pay" => {
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
        "sion_verify_pulse" => {
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
        "sion_events" => {
            let from = Pulse(args["from"].as_u64().unwrap_or(0));
            let limit = args["limit"].as_u64().unwrap_or(50) as usize;
            Ok(
                serde_json::to_value(session.ledger.events(from, limit.min(500))?)
                    .unwrap_or(Value::Null),
            )
        }
        "sion_submit_intent" => {
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
        "sion_fetch_paid" => {
            let url = args["url"]
                .as_str()
                .ok_or_else(|| CliError::Usage("url required".into()))?;
            let wallet = args["wallet"]
                .as_str()
                .ok_or_else(|| CliError::Usage("wallet required".into()))?
                .to_string();
            let max = usd(&args["max_usd"])?;
            let payer = match (args.get("charter"), args.get("allotment")) {
                (Some(charter), Some(allotment)) if !charter.is_null() => {
                    crate::x402::Payer::Charter {
                        agent: wallet,
                        charter: ObjectId::from_hash(hex_id(charter, "charter")?),
                        allotment: ObjectId::from_hash(hex_id(allotment, "allotment")?),
                    }
                }
                _ => crate::x402::Payer::Wallet(wallet),
            };
            let chain = match args["chain"].as_str() {
                Some(hex) => Hash32::from_hex(hex)
                    .map_err(|_| CliError::Usage("chain must be 64 hex digits".into()))?,
                None => crate::verify::served_chain(session.ledger)?,
            };
            let fetched = crate::x402::fetch(session, &payer, url, &chain, max)?;
            Ok(json!({
                "status": fetched.status,
                "body": fetched.body,
                "paid": fetched.settlement.as_ref().map(|s| s.success).unwrap_or(false),
                "quoted": fetched.quoted,
                "settlement": fetched.settlement,
            }))
        }
        "sion_fetch_receipt" => {
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
                    sion_exec::ExecStatus::Success => "success",
                    sion_exec::ExecStatus::Failed(_) => "failed",
                    sion_exec::ExecStatus::Rejected(_) => "rejected",
                },
                "events": verified.effects.events,
                "certificate": verified.signed.map(|(stake, quorum)| json!({ "signed": stake, "quorum": quorum })),
                "chain": verified.chain.0,
                "chain_pinned": verified.chain.1,
                "verified": true,
            }))
        }
        "sion_task_post" => {
            let wallet = text_arg(args, "wallet")?;
            let ids = |key: &str| -> Result<Vec<Hash32>, CliError> {
                args[key]
                    .as_array()
                    .map(|items| items.iter().map(|v| hex_id(v, key)).collect())
                    .unwrap_or_else(|| Ok(Vec::new()))
            };
            let kappa = args["kappa"].as_f64().unwrap_or(2.0).max(1.0);
            let acceptance = sion_exec::task::Acceptance {
                policy: Policy::standard()
                    .with_kappa(Ratio::new((kappa * 1000.0).round() as u64, 1000)),
                requires: ids("requires")?
                    .into_iter()
                    .map(ClaimId::from_hash)
                    .collect(),
                verifiers: args["verifiers"]
                    .as_array()
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(|v| v.as_str())
                            .map(|v| session.id(v))
                            .collect()
                    })
                    .unwrap_or_default(),
                adjudicator: match args["adjudicator"].as_str() {
                    Some(_) => Some(AtomId::from_hash(hex_id(
                        &args["adjudicator"],
                        "adjudicator",
                    )?)),
                    None => None,
                },
                detection_permille: args["detection_permille"].as_u64().unwrap_or(900).min(1000)
                    as u16,
                reliance: args["reliance_pulses"].as_u64().unwrap_or(1_000),
            };
            let charter = match args["charter"].as_str() {
                Some(_) => Some(ObjectId::from_hash(hex_id(&args["charter"], "charter")?)),
                None => None,
            };
            let extra = match charter {
                Some(_) => vec![session.reference(&ObjectId::from_hash(hex_id(
                    &args["allotment"],
                    "allotment",
                )?))?],
                None => Vec::new(),
            };
            let deadline = Pulse(
                session.ledger.pulse()?.0 + args["deadline_pulses"].as_u64().unwrap_or(1_000),
            );
            let spec = sion_core::hash::hash_domain(
                "sion/task/spec",
                args["spec"].as_str().unwrap_or("").as_bytes(),
            );
            let worker = args["worker"].as_str().map(|w| session.id(w));
            let view = session.send(
                &wallet,
                TxKind::PostTask {
                    spec,
                    acceptance,
                    worker,
                    amount: usd(&args["amount_usd"])?,
                    deadline,
                    charter,
                },
                extra,
            )?;
            let task = view.effects.events.iter().find_map(|e| match e {
                Event::TaskPosted { task, .. } => Some(*task),
                _ => None,
            });
            Ok(json!({
                "task": task,
                "pulse": view.pulse,
                "status": view.status,
                "receipt": describe::receipt(&session.names(), &view.effects),
            }))
        }
        "sion_task_attest" | "sion_task_submit" => {
            let wallet = text_arg(args, "wallet")?;
            let task = ObjectId::from_hash(hex_id(&args["task"], "task")?);
            let output = args["output"].as_str().unwrap_or("").as_bytes().to_vec();
            let who = session.id(&wallet);
            let body = if name == "sion_task_attest" {
                crate::tasks::attestation_body(
                    session.ledger,
                    task,
                    &who,
                    &output,
                    args["bond_label"]
                        .as_str()
                        .unwrap_or(crate::tasks::VERIFIER_BOND_LABEL),
                )?
            } else {
                let attestations = args["attestations"]
                    .as_array()
                    .map(|items| {
                        items
                            .iter()
                            .map(|v| hex_id(v, "attestation").map(ClaimId::from_hash))
                            .collect::<Result<Vec<_>, _>>()
                    })
                    .unwrap_or_else(|| Ok(Vec::new()))?;
                crate::tasks::result_body(session.ledger, task, &who, &output, &attestations)?
            };
            let claim = claim_id(&who, &body);
            let view = session.send(&wallet, TxKind::IssueClaim { claim: body }, Vec::new())?;
            Ok(json!({
                "claim": claim,
                "pulse": view.pulse,
                "status": view.status,
                "receipt": describe::receipt(&session.names(), &view.effects),
                "note": "the claim joins the knowledge log at the end of this Pulse; use it from the next one",
            }))
        }
        "sion_task_settle" => {
            let wallet = text_arg(args, "wallet")?;
            let task = ObjectId::from_hash(hex_id(&args["task"], "task")?);
            let result =
                crate::tasks::stored_claim(session.ledger, args["result"].as_str().unwrap_or(""))?;
            let attestations = args["attestations"]
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .map(|v| {
                            crate::tasks::stored_claim(session.ledger, v.as_str().unwrap_or(""))
                                .map(|c| (c.issuer, c.body))
                        })
                        .collect::<Result<Vec<_>, _>>()
                })
                .unwrap_or_else(|| Ok(Vec::new()))?;
            let view = session.send(
                &wallet,
                TxKind::SettleTask {
                    task,
                    result: result.body,
                    attestations,
                },
                Vec::new(),
            )?;
            Ok(json!({
                "pulse": view.pulse,
                "status": view.status,
                "events": view.effects.events,
                "receipt": describe::receipt(&session.names(), &view.effects),
            }))
        }
        "sion_task_show" => {
            let task = ObjectId::from_hash(hex_id(&args["task"], "task")?);
            crate::tasks::describe(session.ledger, task)
        }
        "sion_memory_open" => {
            let wallet = text_arg(args, "wallet")?;
            let agent = match args["agent"].as_str() {
                Some(_) => Some(ObjectId::from_hash(hex_id(&args["agent"], "agent")?)),
                None => None,
            };
            let (view, log) = crate::memory::open(session, &wallet, agent)?;
            Ok(json!({
                "pulse": view.pulse,
                "status": view.status,
                "log": log,
                "receipt": describe::receipt(&session.names(), &view.effects),
            }))
        }
        "sion_memory_remember" => {
            let wallet = text_arg(args, "wallet")?;
            let log = crate::memory::find_log(session.ledger, &text_arg(args, "log")?)?;
            let point = |key: &str| -> Result<Option<[i64; 3]>, CliError> {
                match args[key].as_array() {
                    None => Ok(None),
                    Some(items) => {
                        let values: Vec<i64> = items.iter().filter_map(|v| v.as_i64()).collect();
                        <[i64; 3]>::try_from(values)
                            .map(Some)
                            .map_err(|_| CliError::Usage(format!("{key} must be three integers")))
                    }
                }
            };
            let place = match (args["frame"].as_str(), point("at")?) {
                (Some(frame), Some(at)) => {
                    Some((frame.to_string(), at, point("size")?.unwrap_or([0; 3])))
                }
                (None, None) => None,
                _ => return Err(CliError::Usage("frame and at go together".into())),
            };
            let atom = crate::memory::Remember {
                subject: text_arg(args, "subject")?,
                predicate: text_arg(args, "predicate")?,
                value: crate::memory::parse_value(&text_arg(args, "value")?),
                kind: crate::memory::parse_kind(args["kind"].as_str().unwrap_or("observation"))?,
                place,
                from_s: args["from_s"].as_u64(),
                seconds: args["seconds"].as_u64(),
                confidence: args["confidence_permille"]
                    .as_u64()
                    .map(|c| u16::try_from(c.min(1000)).unwrap_or(1000))
                    .unwrap_or(1000),
            };
            let ledger = session.ledger;
            crate::memory::remember(session, ledger, &wallet, &log, atom)
        }
        "sion_memory_commit" => {
            let wallet = text_arg(args, "wallet")?;
            let log = crate::memory::find_log(session.ledger, &text_arg(args, "log")?)?;
            let seal = args["seal"].as_bool().unwrap_or(false);
            let ledger = session.ledger;
            let view = crate::memory::commit(session, ledger, &wallet, &log, seal)?;
            Ok(json!({
                "pulse": view.pulse,
                "status": view.status,
                "receipt": describe::receipt(&session.names(), &view.effects),
            }))
        }
        "sion_memory_recall" => {
            let log = crate::memory::find_log(session.ledger, &text_arg(args, "log")?)?;
            let corner = |key: &str| -> Result<[i64; 3], CliError> {
                let values: Vec<i64> = args[key]
                    .as_array()
                    .map(|items| items.iter().filter_map(|v| v.as_i64()).collect())
                    .unwrap_or_default();
                <[i64; 3]>::try_from(values)
                    .map_err(|_| CliError::Usage(format!("{key} must be three integers")))
            };
            let seconds = |key: &str| -> Result<u64, CliError> {
                args[key]
                    .as_u64()
                    .ok_or_else(|| CliError::Usage(format!("{key} required")))
            };
            crate::memory::recall(
                session.ledger,
                &log,
                &text_arg(args, "frame")?,
                sion_memory::Box3::new(corner("min")?, corner("max")?),
                sion_memory::TimeRange::new(
                    seconds("from_s")?.saturating_mul(1_000_000_000),
                    seconds("until_s")?.saturating_mul(1_000_000_000),
                ),
            )
        }
        "sion_stake_pools" => Ok(json!({
            "pools": crate::stake::pool_lines(session.ledger, &crate::stake::operator_names(session))?
        })),
        "sion_stake_delegate" => {
            let wallet = text_arg(args, "wallet")?;
            let set = crate::stake::validator_set(session.ledger)?;
            let validator = crate::stake::find_validator(&set, &text_arg(args, "validator")?)?;
            let amount = usd(&args["amount"])?;
            let view = session.send(
                &wallet,
                TxKind::DelegateStake { validator, amount },
                Vec::new(),
            )?;
            Ok(json!({
                "pulse": view.pulse,
                "status": view.status,
                "events": view.effects.events,
                "receipt": describe::receipt(&session.names(), &view.effects),
            }))
        }
        "sion_stake_show" => {
            let account = text_arg(args, "account")?;
            let id = session.id(&account);
            crate::stake::delegations_of(session.ledger, id)
        }
        "sion_stake_claim" | "sion_stake_undelegate" | "sion_stake_withdraw" => {
            let wallet = text_arg(args, "wallet")?;
            let owner = session.id(&wallet);
            let delegation = crate::stake::find_delegation(
                session.ledger,
                owner,
                &text_arg(args, "delegation")?,
            )?;
            let kind = match name {
                "sion_stake_claim" => TxKind::ClaimDelegatorRewards,
                "sion_stake_undelegate" => TxKind::Undelegate,
                _ => TxKind::WithdrawDelegation,
            };
            let reference = session.reference(&delegation)?;
            let view = session.send(&wallet, kind, vec![reference])?;
            Ok(json!({
                "pulse": view.pulse,
                "status": view.status,
                "events": view.effects.events,
                "receipt": describe::receipt(&session.names(), &view.effects),
            }))
        }
        "sion_storage_post" => {
            let wallet = text_arg(args, "wallet")?;
            let path = text_arg(args, "path")?;
            let data = std::fs::read(&path).map_err(|e| CliError::Usage(format!("{path}: {e}")))?;
            let tree = sion_exec::storage::DataTree::of_data(&data);
            let locator = match (args["locator"].as_str(), args["local"].as_bool()) {
                (Some(locator), _) => locator.to_string(),
                (None, Some(true)) => format!(
                    "file://{}",
                    std::fs::canonicalize(&path)
                        .map_err(|e| CliError::Usage(e.to_string()))?
                        .display()
                ),
                _ => {
                    return Err(CliError::Usage(
                        "give locator (an https URL serving the file), or local: true for a provider on this machine".into(),
                    ))
                }
            };
            let provider = args["provider"].as_str().map(|name| session.id(name));
            let rounds = args["rounds"]
                .as_u64()
                .and_then(|r| u32::try_from(r).ok())
                .ok_or_else(|| CliError::Usage("rounds required".into()))?;
            let price = usd(&args["price"])?;
            let collateral = match (&args["collateral"], &provider) {
                (Value::Null, Some(_)) => Amount::ZERO,
                (Value::Null, None) => Amount(price.0.saturating_mul(u128::from(rounds))),
                (value, _) => usd(value)?,
            };
            let accept_by = sion_core::ids::Pulse(session.ledger.pulse()?.0 + 200);
            let view = session.send(
                &wallet,
                TxKind::PostDeal {
                    root: tree.root(),
                    size: tree.size(),
                    challenges: args["challenges"]
                        .as_u64()
                        .and_then(|c| u8::try_from(c).ok())
                        .unwrap_or(4),
                    price,
                    rounds,
                    round_length: args["round_length"].as_u64().unwrap_or(16),
                    collateral,
                    provider,
                    accept_by,
                    locator,
                },
                Vec::new(),
            )?;
            let deal = view.effects.events.iter().find_map(|e| match e {
                sion_exec::Event::DealPosted { deal, .. } => Some(*deal),
                _ => None,
            });
            Ok(json!({
                "pulse": view.pulse,
                "status": view.status,
                "deal": deal,
                "root": tree.root(),
                "leaves": tree.size(),
                "receipt": describe::receipt(&session.names(), &view.effects),
            }))
        }
        "sion_storage_deals" => {
            if let Some(text) = args["deal"].as_str() {
                let id = crate::storage::find_deal(session.ledger, text)?;
                let deal = crate::storage::deal(session.ledger, id)?;
                return crate::storage::deal_json(session.ledger, id, &deal);
            }
            let open = args["open"].as_bool().unwrap_or(false);
            let mut list = Vec::new();
            for (id, deal) in crate::storage::deals(session.ledger, open)? {
                list.push(crate::storage::deal_json(session.ledger, id, &deal)?);
            }
            Ok(json!({ "deals": list }))
        }
        "sion_storage_close" => {
            let wallet = text_arg(args, "wallet")?;
            let deal = crate::storage::find_deal(session.ledger, &text_arg(args, "deal")?)?;
            let view = session.send(&wallet, TxKind::CloseDeal { deal }, Vec::new())?;
            Ok(json!({
                "pulse": view.pulse,
                "status": view.status,
                "events": view.effects.events,
                "receipt": describe::receipt(&session.names(), &view.effects),
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
            "serverInfo": { "name": "sion", "version": env!("CARGO_PKG_VERSION") },
            "instructions": "Tools for the Siontology neo-blockchain devnet. Assess a claim with sion_assess_claim before relying on it."
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
