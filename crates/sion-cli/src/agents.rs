//! Agent identities (spec v1.8): registering agents, launching subagents
//! with their own identities, cards, suspension, resumption, revocation.
//!
//! An agent's identity is a charter with an identity record: the charter's
//! id is the agent's id, its account signs for the agent, and its card
//! commitment says what the agent is. Subagents get their own identity,
//! account, narrower caveats and a budget carved from their parent's
//! allotment.

use clap::Subcommand;
use serde_json::{json, Value};
use sion_assurance::Policy;
use sion_core::hash::Hash32;
use sion_core::ids::{ObjectId, Pulse};
use sion_core::Amount;
use sion_exec::charter::{card_commitment, Caveats, Charter, Spawn, MAX_SPAWN_BATCH};
use sion_exec::execute::Event;
use sion_exec::object::ObjectData;
use sion_exec::TxKind;

use crate::error::CliError;
use crate::ledger::{Ledger, LedgerExt};
use crate::session::{create_wallet, wallet_dir, Session};

/// `sion agent …`
#[derive(Debug, Subcommand)]
pub enum AgentCommand {
    /// Register an agent identity: a charter with a card, funded by the
    /// principal.
    Register {
        /// The principal's wallet (who answers for the agent).
        principal: String,
        /// The agent's wallet or account.
        agent: String,
        /// The agent card file (its exact bytes are committed).
        #[arg(long)]
        card: Option<std::path::PathBuf>,
        /// Allowed payees (comma-separated; empty: any).
        #[arg(long, value_delimiter = ',')]
        payees: Vec<String>,
        /// Largest payment (USD).
        #[arg(long, default_value = "100")]
        max_payment: String,
        /// Largest total per window (USD).
        #[arg(long, default_value = "500")]
        max_window: String,
        /// Window (Pulses).
        #[arg(long, default_value_t = 1_000)]
        window: u64,
        /// Budget (USD).
        #[arg(long, default_value = "1000")]
        funding: String,
        /// Bond behind the agent's receipts (USD).
        #[arg(long, default_value = "0")]
        bond: String,
        /// Lifetime (Pulses).
        #[arg(long, default_value_t = 100_000)]
        lifetime: u64,
        /// The agent may not launch subagents.
        #[arg(long)]
        no_spawn: bool,
    },
    /// Launch subagents, each with its own identity, account, caveats and
    /// budget carved from the parent's allotment.
    Spawn {
        /// The parent agent's wallet.
        parent: String,
        /// The parent's agent id (its charter).
        agent: String,
        /// The parent's allotment.
        allotment: String,
        /// How many subagents.
        #[arg(long, default_value_t = 1)]
        count: usize,
        /// Budget of each (USD).
        #[arg(long, default_value = "10")]
        budget: String,
        /// Wallet names are `<prefix>-1`, `<prefix>-2`, …; a wallet that
        /// does not exist yet is created with a random seed.
        #[arg(long, default_value = "subagent")]
        prefix: String,
        /// The subagents' card file.
        #[arg(long)]
        card: Option<std::path::PathBuf>,
        /// Largest payment of each (USD); default: the parent's.
        #[arg(long)]
        max_payment: Option<String>,
        /// Largest total per window of each (USD); default: an equal share
        /// of the window allowance the parent has not carved out yet.
        #[arg(long)]
        max_window: Option<String>,
        /// The subagents may launch subagents of their own.
        #[arg(long)]
        may_spawn: bool,
    },
    /// Show an agent: account, principal, lineage, card, status, budget.
    Show {
        /// Agent id.
        agent: String,
    },
    /// Replace an agent's card with a file's commitment.
    Card {
        /// The agent's wallet.
        wallet: String,
        /// Agent id.
        agent: String,
        /// The card file.
        file: std::path::PathBuf,
    },
    /// Print an agent card template (A2A-style JSON with a `sion` section).
    CardTemplate {
        /// The agent's name.
        name: String,
        /// What it does.
        #[arg(long, default_value = "")]
        description: String,
        /// Where it is served.
        #[arg(long, default_value = "")]
        url: String,
    },
    /// Suspend an agent and everything below it (an overseer: the backer,
    /// the principal or an ancestor's agent).
    Suspend {
        /// The overseer's wallet.
        wallet: String,
        /// Agent id.
        agent: String,
    },
    /// Lift a suspension (the suspender or the backer).
    Resume {
        /// Wallet.
        wallet: String,
        /// Agent id.
        agent: String,
    },
    /// Revoke an agent and everything below it, for good (the agent itself
    /// or an overseer).
    Revoke {
        /// Wallet.
        wallet: String,
        /// Agent id.
        agent: String,
    },
    /// Return a revoked or expired agent's unspent budget to its backer
    /// (its principal or its backer).
    Reclaim {
        /// Wallet.
        wallet: String,
        /// Agent id.
        agent: String,
    },
}

/// Runs an agent command.
pub fn run(
    session: &mut Session<'_>,
    ledger: &dyn Ledger,
    command: AgentCommand,
) -> Result<(), CliError> {
    match command {
        AgentCommand::Register {
            principal,
            agent,
            card,
            payees,
            max_payment,
            max_window,
            window,
            funding,
            bond,
            lifetime,
            no_spawn,
        } => {
            let agent = session.id(&agent);
            let payees = if payees.is_empty() {
                None
            } else {
                Some(payees.iter().map(|p| session.id(p)).collect())
            };
            let caveats = Caveats {
                payees,
                max_per_payment: parse_amount(&max_payment)?,
                max_per_window: parse_amount(&max_window)?,
                window,
                expires_at: Pulse(ledger.pulse()?.0 + lifetime),
                evidence_above: None,
                evidence_policy: Policy::standard(),
                receipts: true,
                may_delegate: !no_spawn,
            };
            let card = read_card(card.as_deref())?;
            let view = session.send(
                &principal,
                TxKind::RegisterAgent {
                    agent,
                    card,
                    caveats,
                    funding: parse_amount(&funding)?,
                    agent_bond: parse_amount(&bond)?,
                },
                Vec::new(),
            )?;
            print_receipt(session, &view);
            for event in &view.effects.events {
                if let Event::AgentRegistered {
                    agent, allotment, ..
                } = event
                {
                    println!(
                        "agent {agent}\nallotment {allotment}\ndid {}",
                        did(ledger, agent)?
                    );
                }
            }
            Ok(())
        }
        AgentCommand::Spawn {
            parent,
            agent,
            allotment,
            count,
            budget,
            prefix,
            card,
            max_payment,
            max_window,
            may_spawn,
        } => {
            if count == 0 {
                return Err(CliError::Usage("--count must be at least 1".into()));
            }
            let charter = parse_id(&agent, "agent")?;
            let allotment = parse_id(&allotment, "allotment")?;
            let own = identity(ledger, charter)?;
            let mut caveats = own.caveats.clone();
            if let Some(max) = max_payment {
                caveats.max_per_payment = parse_amount(&max)?;
            }
            caveats.max_per_window = match max_window {
                Some(max) => parse_amount(&max)?,
                // Window limits are carved from the parent's: by default the
                // children share what the parent has left, equally.
                None => {
                    let left = match ledger.object(allotment)?.map(|o| o.data) {
                        Some(ObjectData::Allotment(a)) => a
                            .caveats
                            .as_ref()
                            .map(|k| k.max_per_window.0.saturating_sub(a.carved_window.0))
                            .unwrap_or(0),
                        _ => 0,
                    };
                    Amount(left / count as u128)
                }
            };
            caveats.may_delegate = may_spawn && own.caveats.may_delegate;
            let card = read_card(card.as_deref())?;
            let budget = parse_amount(&budget)?;
            let mut names = Vec::with_capacity(count);
            for i in 1..=count {
                let name = format!("{prefix}-{i}");
                ensure_wallet(&name)?;
                names.push(name);
            }
            for batch in names.chunks(MAX_SPAWN_BATCH) {
                let children: Vec<Spawn> = batch
                    .iter()
                    .map(|name| Spawn {
                        agent: session.id(name),
                        card,
                        caveats: caveats.clone(),
                        budget,
                    })
                    .collect();
                let input = session.reference(&allotment)?;
                let view = session.send(
                    &parent,
                    TxKind::SpawnAgents { charter, children },
                    vec![input],
                )?;
                print_receipt(session, &view);
                let spawned: Vec<(ObjectId, ObjectId)> = view
                    .effects
                    .events
                    .iter()
                    .filter_map(|event| match event {
                        Event::AgentSpawned {
                            agent, allotment, ..
                        } => Some((*agent, *allotment)),
                        _ => None,
                    })
                    .collect();
                for (name, (id, own_allotment)) in batch.iter().zip(&spawned) {
                    println!("{name}: agent {id} allotment {own_allotment}");
                }
            }
            Ok(())
        }
        AgentCommand::Show { agent } => {
            let id = parse_id(&agent, "agent")?;
            println!(
                "{}",
                serde_json::to_string_pretty(&describe(ledger, id)?).unwrap_or_default()
            );
            Ok(())
        }
        AgentCommand::Card {
            wallet,
            agent,
            file,
        } => {
            let card = read_card(Some(&file))?;
            let view = session.send(
                &wallet,
                TxKind::SetAgentCard {
                    charter: parse_id(&agent, "agent")?,
                    card,
                },
                Vec::new(),
            )?;
            print_receipt(session, &view);
            Ok(())
        }
        AgentCommand::CardTemplate {
            name,
            description,
            url,
        } => {
            println!(
                "{}",
                serde_json::to_string_pretty(&card_template(&name, &description, &url))
                    .unwrap_or_default()
            );
            Ok(())
        }
        AgentCommand::Suspend { wallet, agent } => {
            let charter = parse_id(&agent, "agent")?;
            let view = session.send(&wallet, TxKind::SuspendAgent { charter }, Vec::new())?;
            print_receipt(session, &view);
            Ok(())
        }
        AgentCommand::Resume { wallet, agent } => {
            let charter = parse_id(&agent, "agent")?;
            let view = session.send(&wallet, TxKind::ResumeAgent { charter }, Vec::new())?;
            print_receipt(session, &view);
            Ok(())
        }
        AgentCommand::Revoke { wallet, agent } => {
            let charter = parse_id(&agent, "agent")?;
            let view = session.send(&wallet, TxKind::RevokeCharter { charter }, Vec::new())?;
            print_receipt(session, &view);
            Ok(())
        }
        AgentCommand::Reclaim { wallet, agent } => {
            let charter = parse_id(&agent, "agent")?;
            let view = session.send(&wallet, TxKind::ReclaimCharter { charter }, Vec::new())?;
            print_receipt(session, &view);
            Ok(())
        }
    }
}

/// An agent card template: the fields of an A2A agent card, plus a `sion`
/// section that ties the card to its identity once registered. The card's
/// commitment is [`card_commitment`] of the file's exact bytes.
pub fn card_template(name: &str, description: &str, url: &str) -> Value {
    json!({
        "name": name,
        "description": description,
        "url": url,
        "version": "1.0.0",
        "provider": { "organization": "", "url": "" },
        "capabilities": { "streaming": false, "pushNotifications": false },
        "defaultInputModes": ["text/plain"],
        "defaultOutputModes": ["text/plain"],
        "skills": [],
        "sion": {
            "agent": "did:sion:<chain identity>:<agent id, filled in after registration>",
            "account": "<the account that signs for the agent>",
            "principal": "<the account that answers for it>",
            "parent": null
        }
    })
}

/// An agent's identity record, lineage and status, as JSON.
pub fn describe(ledger: &dyn Ledger, id: ObjectId) -> Result<Value, CliError> {
    let charter = identity(ledger, id)?;
    let mut lineage = Vec::new();
    let mut next = charter.parent;
    while let Some(parent) = next {
        if lineage.len() == sion_exec::charter::MAX_CHARTER_CHAIN {
            break;
        }
        let above = identity(ledger, parent)?;
        lineage.push(json!({
            "agent": parent,
            "account": above.agent,
            "suspended_by": above.suspended_by,
            "revoked_at": above.revoked_at,
        }));
        next = above.parent;
    }
    let mut remaining = 0u128;
    for allotment in &charter.allotments {
        if let Some(object) = ledger.object(*allotment)? {
            if let ObjectData::Allotment(a) = object.data {
                remaining += a.remaining.0;
            }
        }
    }
    let status = if let Some(at) = charter.revoked_at {
        format!("revoked at Pulse {}", at.0)
    } else if let Some(by) = charter.suspended_by {
        format!("suspended by {by}")
    } else if lineage
        .iter()
        .any(|a| !a["revoked_at"].is_null() || !a["suspended_by"].is_null())
    {
        "stopped by an ancestor".into()
    } else {
        "active".into()
    };
    Ok(json!({
        "agent": id,
        "did": did(ledger, &id)?,
        "account": charter.agent,
        "principal": charter.principal,
        "backer": charter.backer,
        "card": charter.identity.as_ref().map(|i| i.card),
        "depth": charter.identity.as_ref().map(|i| i.depth),
        "status": status,
        "created_at": charter.created_at,
        "expires_at": charter.caveats.expires_at,
        "may_spawn": charter.caveats.may_delegate,
        "max_per_payment": charter.caveats.max_per_payment,
        "max_per_window": charter.caveats.max_per_window,
        "budget_remaining": Amount(remaining),
        "allotments": charter.allotments,
        "lineage": lineage,
    }))
}

/// An agent's DID: `did:sion:<chain identity>:<agent id>`.
pub fn did(ledger: &dyn Ledger, agent: &ObjectId) -> Result<String, CliError> {
    let status = ledger.status()?;
    let chain = status["chain"].as_str().unwrap_or("unknown");
    Ok(format!("did:sion:{chain}:{agent}"))
}

/// Loads a charter.
pub fn identity(ledger: &dyn Ledger, id: ObjectId) -> Result<Charter, CliError> {
    match ledger.object(id)?.map(|o| o.data) {
        Some(ObjectData::Charter(charter)) => Ok(charter),
        Some(_) => Err(CliError::Usage(format!("{id} is not an agent or charter"))),
        None => Err(CliError::Usage(format!("no object {id}"))),
    }
}

/// The commitment of a card file, or zero for none.
fn read_card(path: Option<&std::path::Path>) -> Result<Hash32, CliError> {
    match path {
        None => Ok(Hash32::ZERO),
        Some(path) => {
            let bytes = std::fs::read(path)
                .map_err(|e| CliError::Usage(format!("{}: {e}", path.display())))?;
            Ok(card_commitment(&bytes))
        }
    }
}

/// Makes sure a wallet named `name` has a seed file, creating one with a
/// random seed if it has none.
fn ensure_wallet(name: &str) -> Result<(), CliError> {
    if wallet_dir().join(format!("{name}.seed")).exists() {
        return Ok(());
    }
    create_wallet(name).map(|_| ())
}

pub(crate) fn print_receipt(session: &Session<'_>, view: &sion_node::TxView) {
    println!(
        "Pulse {}: {}",
        view.pulse,
        crate::describe::receipt(&session.names(), &view.effects)
    );
}

fn parse_id(text: &str, what: &str) -> Result<ObjectId, CliError> {
    Hash32::from_hex(text.trim())
        .map(ObjectId::from_hash)
        .map_err(|_| CliError::Usage(format!("{what} must be a 64-digit hex ID")))
}

/// A decimal amount with at most six places: `12.5` → 12,500,000 micro-units.
pub fn parse_amount(text: &str) -> Result<Amount, CliError> {
    let text = text.trim();
    let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
    if whole.is_empty() && fraction.is_empty() || fraction.len() > 6 {
        return Err(CliError::Usage(format!(
            "{text:?} is not an amount with at most 6 decimals"
        )));
    }
    let whole: u128 = if whole.is_empty() {
        0
    } else {
        whole
            .parse()
            .map_err(|_| CliError::Usage(format!("{text:?} is not an amount")))?
    };
    let mut padded = fraction.to_string();
    while padded.len() < 6 {
        padded.push('0');
    }
    let fraction: u128 = padded
        .parse()
        .map_err(|_| CliError::Usage(format!("{text:?} is not an amount")))?;
    Ok(Amount(whole * 1_000_000 + fraction))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_card_template_has_the_a2a_fields_and_a_sion_section() {
        let card = card_template("researcher", "finds papers", "https://example.org/a2a");
        for field in [
            "name",
            "description",
            "url",
            "version",
            "capabilities",
            "skills",
        ] {
            assert!(card.get(field).is_some(), "{field}");
        }
        assert!(card["sion"]["agent"]
            .as_str()
            .unwrap()
            .starts_with("did:sion:"));
    }

    #[test]
    fn amounts_parse_to_micro_units() {
        assert_eq!(parse_amount("12.5").unwrap(), Amount(12_500_000));
        assert_eq!(parse_amount(".000001").unwrap(), Amount(1));
        assert!(parse_amount("1.0000001").is_err());
        assert!(parse_amount("").is_err());
    }
}
