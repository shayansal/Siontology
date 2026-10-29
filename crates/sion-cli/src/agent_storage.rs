//! Storage deals as an agent's tools (`sion agent run --storage`): have a
//! file of the workspace kept by storage providers, proven every round
//! ([Storage deals](../../docs/book/src/concepts/storage.md)), follow the
//! deals, and close them.
//!
//! A deal's escrow comes from the agent's account, the money it pays fees
//! with, not from its charter's budget: the charter's caveats do not bound
//! it, only that balance does (`agent:storage-from-account`). Posting a deal
//! is spending, so autonomy L5 asks first.

use std::path::PathBuf;

use serde_json::{json, Value};
use sion_agent::tools::{Effect, Outcome, Tool};
use sion_agent::ToolSpec;
use sion_core::ids::Pulse;
use sion_core::Amount;
use sion_exec::storage::DataTree;
use sion_exec::{Event, ExecStatus, TxKind};

use crate::error::CliError;
use crate::ledger::LedgerExt;
use crate::runtime::Acting;
use crate::session::Session;

/// The most bytes of a file an agent has kept.
pub const MAX_KEEP: u64 = 64 << 20;

/// Pulses a deal stays open for a provider to accept.
pub const ACCEPT_WITHIN: u64 = 200;

fn schema(properties: Value, required: &[&str]) -> Value {
    json!({"type": "object", "properties": properties, "required": required})
}

fn outcome(result: Result<Value, CliError>) -> Outcome {
    match result {
        Ok(value) => Outcome::ok(value.to_string()),
        Err(CliError::Connection(e)) => Outcome::uncertain(format!(
            "the node could not be reached ({e}); the transaction may or may not have been sent: \
             check with storage_deals before trying again"
        )),
        Err(e) => Outcome::error(e.to_string()),
    }
}

/// Posts a deal to have a workspace file kept.
#[derive(Clone, Debug)]
pub struct KeepTool {
    /// The agent acting.
    pub acting: Acting,
    /// Its workspace.
    pub root: PathBuf,
}

impl Tool for KeepTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "storage_keep".into(),
            description: format!(
                "Have a file of your workspace kept by a storage provider, who proves every round \
                 that it still has it and is paid per round proven. Give the file's path, the price \
                 per round and the number of rounds (the escrow, price × rounds, comes from your \
                 account's balance, not your budget), and where providers fetch it: an https URL \
                 serving the same bytes, or nothing for a provider on this machine (the deal then \
                 shows the file's path to everyone). Optionally: Pulses per round (a multiple of 8, \
                 default 16), leaves challenged per round (1 to 16, default 4), the collateral a \
                 provider must post (default: the escrow) and the one provider allowed. A deal \
                 stays open for {ACCEPT_WITHIN} Pulses."
            ),
            input_schema: schema(
                json!({
                    "path": {"type": "string"},
                    "price": {"type": "string", "description": "USD per round, for example \"0.05\""},
                    "rounds": {"type": "integer"},
                    "locator": {"type": "string"},
                    "round_length": {"type": "integer"},
                    "challenges": {"type": "integer"},
                    "collateral": {"type": "string"},
                    "provider": {"type": "string"}
                }),
                &["path", "price", "rounds"],
            ),
        }
    }

    fn effect(&self) -> Effect {
        Effect::Spend
    }

    fn call(&self, input: &Value, _key: &str) -> Outcome {
        outcome(keep(self, input))
    }
}

fn keep(tool: &KeepTool, input: &Value) -> Result<Value, CliError> {
    let relative = input["path"].as_str().unwrap_or_default();
    let path = sion_agent::builtin::inside(&tool.root, relative).map_err(CliError::Usage)?;
    let meta = std::fs::metadata(&path).map_err(|e| CliError::Usage(format!("{relative}: {e}")))?;
    if !meta.is_file() {
        return Err(CliError::Usage(format!("{relative} is not a file")));
    }
    if meta.len() == 0 || meta.len() > MAX_KEEP {
        return Err(CliError::Usage(format!(
            "a file kept is 1 byte to {} MiB",
            MAX_KEEP >> 20
        )));
    }
    let data = std::fs::read(&path).map_err(|e| CliError::Usage(format!("{relative}: {e}")))?;
    let tree = DataTree::of_data(&data);
    let locator = match input["locator"].as_str().filter(|l| !l.is_empty()) {
        Some(locator) if locator.starts_with("https://") => locator.to_string(),
        Some(other) => {
            return Err(CliError::Usage(format!(
                "{other}: give an https:// URL, or no locator for a provider on this machine"
            )))
        }
        None => format!(
            "file://{}",
            path.canonicalize()
                .map_err(|e| CliError::Usage(e.to_string()))?
                .display()
        ),
    };
    let price = crate::agents::parse_amount(input["price"].as_str().unwrap_or_default())?;
    let rounds = input["rounds"]
        .as_u64()
        .and_then(|r| u32::try_from(r).ok())
        .filter(|r| *r > 0)
        .ok_or_else(|| CliError::Usage("give the number of rounds".into()))?;
    let ledger = tool.acting.ledger.as_ref();
    let mut session = Session::open(ledger)?;
    let provider = input["provider"]
        .as_str()
        .filter(|p| !p.is_empty())
        .map(|p| session.id(p));
    let collateral = match (input["collateral"].as_str(), &provider) {
        (Some(text), _) => crate::agents::parse_amount(text)?,
        (None, Some(_)) => Amount::ZERO,
        (None, None) => Amount(price.0.saturating_mul(u128::from(rounds))),
    };
    let accept_by = Pulse(ledger.pulse()?.0 + ACCEPT_WITHIN);
    let view = session.send(
        &tool.acting.wallet,
        TxKind::PostDeal {
            root: tree.root(),
            size: tree.size(),
            challenges: input["challenges"]
                .as_u64()
                .and_then(|c| u8::try_from(c).ok())
                .unwrap_or(4),
            price,
            rounds,
            round_length: input["round_length"].as_u64().unwrap_or(16),
            collateral,
            provider,
            accept_by,
            locator,
        },
        Vec::new(),
    )?;
    let names = session.names();
    if view.status != ExecStatus::Success {
        return Err(CliError::Transaction(format!(
            "refused by the ledger at Pulse {}: {}",
            view.pulse.0,
            crate::describe::receipt(&names, &view.effects)
        )));
    }
    let deal = view.effects.events.iter().find_map(|e| match e {
        Event::DealPosted { deal, .. } => Some(*deal),
        _ => None,
    });
    Ok(json!({
        "deal": deal,
        "root": tree.root(),
        "leaves": tree.size(),
        "escrow_usd": Amount(price.0.saturating_mul(u128::from(rounds))).to_string(),
        "open_until_pulse": accept_by.0,
        "receipt": format!(
            "Pulse {}: {}",
            view.pulse.0,
            crate::describe::receipt(&names, &view.effects)
        ),
    }))
}

/// The deals the agent posted.
#[derive(Clone, Debug)]
pub struct DealsTool(pub Acting);

impl Tool for DealsTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "storage_deals".into(),
            description: "Your storage deals: for each, whether a provider took it, how many \
                          rounds it proved, what it was paid and what is left in escrow. Give a \
                          deal's id for that one only."
                .into(),
            input_schema: schema(json!({"deal": {"type": "string"}}), &[]),
        }
    }

    fn effect(&self) -> Effect {
        Effect::Read
    }

    fn call(&self, input: &Value, _key: &str) -> Outcome {
        outcome((|| {
            let ledger = self.0.ledger.as_ref();
            let mut session = Session::open(ledger)?;
            let me = session.id(&self.0.wallet);
            if let Some(text) = input["deal"].as_str().filter(|t| !t.is_empty()) {
                let id = crate::storage::find_deal(ledger, text)?;
                let deal = crate::storage::deal(ledger, id)?;
                return crate::storage::deal_json(ledger, id, &deal);
            }
            let mut mine = Vec::new();
            for (id, deal) in crate::storage::deals(ledger, false)? {
                if deal.client == me {
                    mine.push(crate::storage::deal_json(ledger, id, &deal)?);
                }
            }
            Ok(json!({"deals": mine}))
        })())
    }
}

/// Closes one of the agent's deals.
#[derive(Clone, Debug)]
pub struct CloseTool(pub Acting);

impl Tool for CloseTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "storage_close".into(),
            description: "Close one of your storage deals: one no provider took is refunded; one \
                          taken can be closed at its end, or once its provider stopped proving, \
                          and what is left in escrow comes back."
                .into(),
            input_schema: schema(json!({"deal": {"type": "string"}}), &["deal"]),
        }
    }

    fn effect(&self) -> Effect {
        Effect::Write
    }

    fn call(&self, input: &Value, _key: &str) -> Outcome {
        outcome((|| {
            let ledger = self.0.ledger.as_ref();
            let deal =
                crate::storage::find_deal(ledger, input["deal"].as_str().unwrap_or_default())?;
            let mut session = Session::open(ledger)?;
            let view = session.send(&self.0.wallet, TxKind::CloseDeal { deal }, Vec::new())?;
            let names = session.names();
            let receipt = format!(
                "Pulse {}: {}",
                view.pulse.0,
                crate::describe::receipt(&names, &view.effects)
            );
            if view.status != ExecStatus::Success {
                return Err(CliError::Transaction(format!(
                    "refused by the ledger: {receipt}"
                )));
            }
            Ok(json!({"deal": deal, "receipt": receipt}))
        })())
    }
}

/// The storage tools for `acting`; keeping a file needs a workspace.
pub fn tools(acting: &Acting, workspace: Option<&std::path::Path>) -> Vec<Box<dyn Tool>> {
    let mut tools: Vec<Box<dyn Tool>> = vec![
        Box::new(DealsTool(acting.clone())),
        Box::new(CloseTool(acting.clone())),
    ];
    if let Some(root) = workspace {
        tools.push(Box::new(KeepTool {
            acting: acting.clone(),
            root: root.to_path_buf(),
        }));
    }
    tools
}
