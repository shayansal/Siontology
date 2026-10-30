//! Task Charters (spec v1.10): posting escrowed work whose acceptance is
//! fixed in advance, attesting and submitting results, settling and
//! cancelling.
//!
//! A worker's result claim must carry exactly the formula the task
//! requires (`sion_exec::task::result_formula`): the worker's task bond
//! (`bond:<worker>:task-worker`, see `sion bond`) or a named verifier's
//! attestation of the same output, and every required claim. These
//! commands build the claims the way the ledger checks them.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use clap::Subcommand;
use serde_json::{json, Value};
use sion_assurance::{AtomClass, Policy, Ratio};
use sion_core::hash::{hash_domain, Hash32};
use sion_core::ids::{AccountId, AtomId, ClaimId, ObjectId, Pulse};
use sion_exec::claim::{claim_id, formulas, ClaimBody, StoredClaim};
use sion_exec::execute::{bond_atom, Event};
use sion_exec::object::ObjectData;
use sion_exec::task::{
    result_formula, Acceptance, Task, TaskResult, ATTESTATION_SCHEMA, RESULT_SCHEMA,
};
use sion_exec::TxKind;

use crate::error::CliError;
use crate::ledger::{Ledger, LedgerExt};
use crate::session::Session;

/// The bond label verifiers' attestations rest on by default.
pub const VERIFIER_BOND_LABEL: &str = "verifier";

/// `sion task …`
#[derive(Debug, Subcommand)]
pub enum TaskCommand {
    /// Post a task: escrow money for work whose acceptance rule is fixed
    /// now. Without --worker, anyone may settle it (a bounty).
    Post {
        /// The poster's wallet (with --agent, the agent's wallet).
        poster: String,
        /// Escrow (USD), paid in full for an accepted result.
        amount: String,
        /// The worker (wallet or account); omit for an open bounty.
        #[arg(long)]
        worker: Option<String>,
        /// A file describing the task (its exact bytes are committed).
        #[arg(long)]
        spec: Option<PathBuf>,
        /// Claims every result must rest on (comma-separated ids).
        #[arg(long, value_delimiter = ',')]
        requires: Vec<String>,
        /// Verifiers whose attestations count (comma-separated wallets or
        /// accounts).
        #[arg(long, value_delimiter = ',')]
        verifiers: Vec<String>,
        /// Verifiers who must each attest every result, among --verifiers
        /// (comma-separated; spec v1.15).
        #[arg(long, value_delimiter = ',')]
        must_attest: Vec<String>,
        /// How many attestations a result needs at least (spec v1.15).
        #[arg(long, value_name = "N", default_value_t = 0)]
        min_attestations: u8,
        /// The adjudicator atom results must name (hex).
        #[arg(long)]
        adjudicator: Option<String>,
        /// The largest detection factor results may declare, in permille.
        #[arg(long, default_value_t = 900)]
        detection: u16,
        /// Pulses of reliance on a paid result (its backing stays reserved).
        #[arg(long, default_value_t = 1_000)]
        reliance: u64,
        /// Safety factor κ of the lab's policy.
        #[arg(long, default_value_t = 2.0)]
        kappa: f64,
        /// Pulses until the deadline.
        #[arg(long, default_value_t = 1_000)]
        deadline: u64,
        /// Post from this agent's budget (its id); needs --allotment.
        #[arg(long)]
        agent: Option<String>,
        /// The agent's allotment.
        #[arg(long)]
        allotment: Option<String>,
    },
    /// Attest a task's output as a verifier, on your bond
    /// `bond:<you>:verifier`.
    Attest {
        /// The verifier's wallet.
        verifier: String,
        /// The task.
        task: String,
        /// The output file (its exact bytes are committed).
        output: PathBuf,
        /// The bond label the attestation rests on.
        #[arg(long, default_value = VERIFIER_BOND_LABEL)]
        bond: String,
    },
    /// Submit a result claim for a task, as its worker, resting on your
    /// task bond, the attestations given, and the task's required claims.
    Submit {
        /// The worker's wallet.
        worker: String,
        /// The task.
        task: String,
        /// The output file (its exact bytes are committed).
        output: PathBuf,
        /// Attestation claim ids (comma-separated), in order.
        #[arg(long, value_delimiter = ',')]
        attestations: Vec<String>,
    },
    /// Settle a task with your result claim and its attestations.
    Settle {
        /// The worker's wallet.
        worker: String,
        /// The task.
        task: String,
        /// The result claim id.
        result: String,
        /// Attestation claim ids (comma-separated), in the order the
        /// result lists them.
        #[arg(long, value_delimiter = ',')]
        attestations: Vec<String>,
    },
    /// Cancel a task: a worker releases it, or the poster withdraws an open
    /// or expired one. The escrow is refunded.
    Cancel {
        /// Wallet.
        wallet: String,
        /// The task.
        task: String,
    },
    /// Show a task: its terms, escrow and state.
    Show {
        /// The task.
        task: String,
    },
}

/// Runs a task command.
pub fn run(
    session: &mut Session<'_>,
    ledger: &dyn Ledger,
    command: TaskCommand,
) -> Result<(), CliError> {
    match command {
        TaskCommand::Post {
            poster,
            amount,
            worker,
            spec,
            requires,
            verifiers,
            must_attest,
            min_attestations,
            adjudicator,
            detection,
            reliance,
            kappa,
            deadline,
            agent,
            allotment,
        } => {
            let policy = Policy::standard().with_kappa(Ratio::new(
                (kappa * 1000.0).round().max(1_000.0) as u64,
                1000,
            ));
            let acceptance = Acceptance {
                policy,
                requires: requires
                    .iter()
                    .map(|c| parse_hash(c, "claim").map(ClaimId::from_hash))
                    .collect::<Result<BTreeSet<_>, _>>()?,
                verifiers: verifiers.iter().map(|v| session.id(v)).collect(),
                must_attest: must_attest.iter().map(|v| session.id(v)).collect(),
                min_attestations,
                adjudicator: adjudicator
                    .as_deref()
                    .map(|a| parse_hash(a, "adjudicator atom").map(AtomId::from_hash))
                    .transpose()?,
                detection_permille: detection,
                reliance,
            };
            let charter = agent
                .as_deref()
                .map(|a| parse_hash(a, "agent").map(ObjectId::from_hash))
                .transpose()?;
            let extra = match (&charter, allotment) {
                (Some(_), Some(allotment)) => {
                    vec![session
                        .reference(&ObjectId::from_hash(parse_hash(&allotment, "allotment")?))?]
                }
                (Some(_), None) => {
                    return Err(CliError::Usage(
                        "--agent needs --allotment, the agent's allotment".into(),
                    ))
                }
                (None, _) => Vec::new(),
            };
            let kind = TxKind::PostTask {
                spec: commitment(spec.as_deref(), "sion/task/spec")?,
                acceptance,
                worker: worker.as_deref().map(|w| session.id(w)),
                amount: crate::agents::parse_amount(&amount)?,
                deadline: Pulse(ledger.pulse()?.0 + deadline),
                charter,
            };
            let view = session.send(&poster, kind, extra)?;
            crate::agents::print_receipt(session, &view);
            for event in &view.effects.events {
                if let Event::TaskPosted { task, .. } = event {
                    println!("task {task}");
                }
            }
            Ok(())
        }
        TaskCommand::Attest {
            verifier,
            task,
            output,
            bond,
        } => {
            let task = parse_task(&task)?;
            let who = session.id(&verifier);
            let body = attestation_body(ledger, task, &who, &read_file(&output)?, &bond)?;
            issue(session, &verifier, who, body)
        }
        TaskCommand::Submit {
            worker,
            task,
            output,
            attestations,
        } => {
            let task = parse_task(&task)?;
            let who = session.id(&worker);
            let attested = attestations
                .iter()
                .map(|a| parse_hash(a, "attestation").map(ClaimId::from_hash))
                .collect::<Result<Vec<_>, _>>()?;
            let body = result_body(ledger, task, &who, &read_file(&output)?, &attested)?;
            issue(session, &worker, who, body)
        }
        TaskCommand::Settle {
            worker,
            task,
            result,
            attestations,
        } => {
            let task = parse_task(&task)?;
            let result = stored_claim(ledger, &result)?;
            let attestations = attestations
                .iter()
                .map(|a| stored_claim(ledger, a).map(|c| (c.issuer, c.body)))
                .collect::<Result<Vec<_>, _>>()?;
            let view = session.send(
                &worker,
                TxKind::SettleTask {
                    task,
                    result: result.body,
                    attestations,
                },
                Vec::new(),
            )?;
            crate::agents::print_receipt(session, &view);
            Ok(())
        }
        TaskCommand::Cancel { wallet, task } => {
            let task = parse_task(&task)?;
            let view = session.send(&wallet, TxKind::CancelTask { task }, Vec::new())?;
            crate::agents::print_receipt(session, &view);
            Ok(())
        }
        TaskCommand::Show { task } => {
            let id = parse_task(&task)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&describe(ledger, id)?).unwrap_or_default()
            );
            Ok(())
        }
    }
}

/// A task's terms and state, as JSON.
pub fn describe(ledger: &dyn Ledger, id: ObjectId) -> Result<Value, CliError> {
    let task = load_task(ledger, id)?;
    Ok(json!({
        "task": id,
        "poster": task.poster,
        "funded_by_agent": task.funder,
        "refund_to": task.refund_to,
        "worker": task.worker,
        "escrow": task.amount.to_string(),
        "deadline": task.deadline,
        "spec": task.spec,
        "acceptance": {
            "requires": task.acceptance.requires,
            "verifiers": task.acceptance.verifiers,
            "must_attest": task.acceptance.must_attest,
            "min_attestations": task.acceptance.min_attestations,
            "adjudicator": task.acceptance.adjudicator,
            "detection_permille": task.acceptance.detection_permille,
            "reliance_pulses": task.acceptance.reliance,
            "kappa": task.acceptance.policy.kappa.num as f64 / task.acceptance.policy.kappa.den.max(1) as f64,
        },
        "state": task.state,
    }))
}

/// The body of `verifier`'s attestation of `output` for `task`, resting on
/// its bond `bond:<verifier>:<bond_label>`.
pub fn attestation_body(
    ledger: &dyn Ledger,
    task: ObjectId,
    verifier: &AccountId,
    output: &[u8],
    bond_label: &str,
) -> Result<ClaimBody, CliError> {
    let state = load_task(ledger, task)?;
    Ok(ClaimBody {
        schema: ATTESTATION_SCHEMA.into(),
        subject: format!("the output for task {} passes", task.0.short()),
        payload: result_payload(task, output),
        formula: formulas::bonded_statement(
            bond_atom(AtomClass::Economic, verifier, bond_label).id,
        ),
        adjudicator: state.acceptance.adjudicator,
        detection_permille: 1000,
        valid_for: claim_window(&state, ledger)?,
        challenge_period: state.acceptance.reliance,
    })
}

/// The body of `worker`'s result claim for `task`: the exact formula the
/// task requires (spec §5.43 step 5), over the given attestations.
pub fn result_body(
    ledger: &dyn Ledger,
    task: ObjectId,
    worker: &AccountId,
    output: &[u8],
    attestations: &[ClaimId],
) -> Result<ClaimBody, CliError> {
    let state = load_task(ledger, task)?;
    Ok(ClaimBody {
        schema: RESULT_SCHEMA.into(),
        subject: format!("result for task {}", task.0.short()),
        payload: result_payload(task, output),
        formula: result_formula(worker, attestations, &state.acceptance.requires),
        adjudicator: state.acceptance.adjudicator,
        detection_permille: state.acceptance.detection_permille,
        valid_for: claim_window(&state, ledger)?,
        challenge_period: state.acceptance.reliance,
    })
}

/// A task as the ledger holds it.
pub fn load_task(ledger: &dyn Ledger, id: ObjectId) -> Result<Task, CliError> {
    match ledger.object(id)?.map(|o| o.data) {
        Some(ObjectData::Task(task)) => Ok(*task),
        Some(_) => Err(CliError::Usage(format!("{id} is not a task"))),
        None => Err(CliError::Usage(format!("no task {id}"))),
    }
}

fn parse_hash(text: &str, what: &str) -> Result<Hash32, CliError> {
    Hash32::from_hex(text.trim()).map_err(|_| CliError::Usage(format!("bad {what} id {text:?}")))
}

fn parse_task(text: &str) -> Result<ObjectId, CliError> {
    parse_hash(text, "task").map(ObjectId::from_hash)
}

/// A commitment to a file's exact bytes, or to nothing.
fn commitment(file: Option<&Path>, domain: &str) -> Result<Hash32, CliError> {
    match file {
        Some(path) => {
            let bytes = std::fs::read(path)
                .map_err(|e| CliError::Usage(format!("cannot read {}: {e}", path.display())))?;
            Ok(hash_domain(domain, &bytes))
        }
        None => Ok(Hash32::ZERO),
    }
}

/// The commitment a task makes to its spec's exact bytes.
pub fn spec_commitment(spec: &[u8]) -> Hash32 {
    hash_domain("sion/task/spec", spec)
}

/// The commitment a result and its attestations make to an output's
/// exact bytes.
pub fn output_commitment(output: &[u8]) -> Hash32 {
    hash_domain("sion/task/output", output)
}

/// The payload of a result or an attestation for `task`: the task and a
/// commitment to the output.
fn result_payload(task: ObjectId, output: &[u8]) -> sion_exec::claim::Payload {
    TaskResult {
        task,
        output: output_commitment(output),
    }
    .payload()
}

fn read_file(path: &Path) -> Result<Vec<u8>, CliError> {
    std::fs::read(path).map_err(|e| CliError::Usage(format!("cannot read {}: {e}", path.display())))
}

/// A claim window long enough to be relied on until the task's deadline
/// plus its reliance period.
fn claim_window(task: &Task, ledger: &dyn Ledger) -> Result<u64, CliError> {
    let now = ledger.pulse()?.0;
    Ok(task
        .deadline
        .0
        .saturating_sub(now)
        .saturating_add(task.acceptance.reliance)
        .max(1))
}

/// Issues `body` as `wallet` (whose account is `issuer`) and prints the
/// claim's id.
pub fn issue(
    session: &mut Session<'_>,
    wallet: &str,
    issuer: AccountId,
    body: ClaimBody,
) -> Result<(), CliError> {
    let id = claim_id(&issuer, &body);
    let view = session.send(wallet, TxKind::IssueClaim { claim: body }, Vec::new())?;
    crate::agents::print_receipt(session, &view);
    println!("claim {id}");
    Ok(())
}

/// A claim as the ledger stores it, body and issuer included.
pub fn stored_claim(ledger: &dyn Ledger, id: &str) -> Result<StoredClaim, CliError> {
    let id = ClaimId::from_hash(parse_hash(id, "claim")?);
    let value = ledger.claim(id)?;
    serde_json::from_value(value["claim"].clone())
        .map_err(|e| CliError::Protocol(format!("claim {id}: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_commitment_binds_the_exact_bytes() {
        let dir = std::env::temp_dir().join(format!("sion-task-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("out.txt");
        std::fs::write(&file, b"patch").unwrap();
        let a = commitment(Some(&file), "sion/task/output").unwrap();
        std::fs::write(&file, b"patch ").unwrap();
        let b = commitment(Some(&file), "sion/task/output").unwrap();
        assert_ne!(a, b);
        assert_eq!(commitment(None, "x").unwrap(), Hash32::ZERO);
        std::fs::remove_dir_all(&dir).ok();
    }
}
