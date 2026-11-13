//! A market for proofs of Pulses.
//!
//! The architecture has provers do the heavy work of proving each Pulse
//! and be paid for it. Here that is a market made of pieces the ledger
//! already has, with no change to consensus:
//!
//! - **A request** is a Task Charter: a payment in escrow for the World
//!   Proof of one Pulse under a named proof system ([`Wanted`]), which
//!   the task's spec commits to. It is open to any prover, or names one.
//!   Its acceptance names the verifiers who must each attest a result.
//! - **A prover** delivers the proof. Its result claim commits to the
//!   proof's bytes.
//! - **Verifiers** check the delivered proof: that it is of the certified
//!   header of that Pulse, and that every leaf verifies under both proof
//!   stacks. Each attests on its own bond. A false attestation is a
//!   bonded claim an adjudicator can rule against.
//! - **Settling** pays the prover once the attestations the request asked
//!   for are there.
//!
//! The only proof system here is [`SYSTEM`]: the two replay stacks, which
//! simulate succinct proofs (a leaf proof is its witness, checked by
//! re-executing it). A request names its system so that a succinct one
//! can be asked for the same way once there is one. A node keeps the
//! World Proofs of its last Pulses only, so what a requester buys today
//! is a proof delivered and checked before it is gone.

use std::path::PathBuf;

use clap::Subcommand;
use serde::{Deserialize, Serialize};
use sion_core::encode::decode;
use sion_core::hash::Hash32;
use sion_core::ids::{AccountId, AtomId, ClaimId, ObjectId, Pulse};
use sion_core::Amount;
use sion_exec::claim::{claim_id, formulas, ClaimBody, Payload};
use sion_exec::execute::Event;
use sion_exec::task::{Acceptance, TaskState};
use sion_exec::{ExecStatus, TxKind};
use sion_node::Query;
use sion_prover::WorldProof;

use crate::error::CliError;
use crate::feeds;
use crate::ledger::{Ledger, LedgerExt};
use crate::session::Session;

/// The proof system this implementation has: World Proofs whose leaves
/// are replay proofs, each verified under the reference executor and
/// under the independent implementation of the specification.
pub const SYSTEM: &str = "replay+spec/v1";

/// The schema of the claim that says what a request's task wants.
pub const REQUEST_SCHEMA: &str = "proving.request/v1";

/// What a request wants: fixed in its task's spec.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Wanted {
    /// The chain: its identity, as a light client pins it.
    pub chain: String,
    /// The Pulse.
    pub pulse: u64,
    /// The proof system.
    pub system: String,
}

impl Wanted {
    /// The spec a task that wants this commits to.
    pub fn spec(&self) -> Hash32 {
        let said = serde_json::json!({
            "wanted": "the World Proof of a Pulse",
            "chain": self.chain,
            "pulse": self.pulse,
            "system": self.system,
        });
        crate::tasks::spec_commitment(said.to_string().as_bytes())
    }
}

/// What a request's claim holds: its task, and what the task wants.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    /// The task.
    pub task: ObjectId,
    /// What it wants.
    pub wanted: Wanted,
}

fn request_key(task: &ObjectId) -> String {
    format!("proving:{}", feeds::short_key(&task.0))
}

/// The terms of a request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Terms {
    /// The Pulse whose World Proof is wanted.
    pub pulse: u64,
    /// What is escrowed for it.
    pub amount: Amount,
    /// The verifiers, every one of whom must attest the proof.
    pub verifiers: Vec<AccountId>,
    /// The one prover who may deliver, or anyone. An attestation is of the
    /// proof's bytes and not of who made them, so an open request pays
    /// whoever settles first with the attested bytes.
    pub prover: Option<AccountId>,
    /// The adjudicator atom the result and its attestations name.
    pub adjudicator: Option<AtomId>,
    /// How many Pulses the request stays open.
    pub deadline: u64,
}

/// Posts a request on `terms`, as `wallet`. Returns the task.
pub fn request(
    session: &mut Session<'_>,
    wallet: &str,
    terms: &Terms,
) -> Result<ObjectId, CliError> {
    let Terms {
        pulse,
        amount,
        verifiers,
        prover,
        adjudicator,
        deadline,
    } = terms.clone();
    if verifiers.is_empty() || verifiers.len() > sion_exec::task::MAX_VERIFIERS {
        return Err(CliError::Usage(format!(
            "a request names 1 to {} verifiers, who each check the proof",
            sion_exec::task::MAX_VERIFIERS
        )));
    }
    let ledger = session.ledger;
    let wanted = Wanted {
        chain: crate::verify::served_chain(ledger)?.to_hex(),
        pulse,
        system: SYSTEM.to_string(),
    };
    let view = session.must(
        wallet,
        TxKind::PostTask {
            spec: wanted.spec(),
            acceptance: Acceptance {
                policy: sion_assurance::Policy::standard(),
                requires: Default::default(),
                verifiers: verifiers.iter().copied().collect(),
                must_attest: verifiers.iter().copied().collect(),
                min_attestations: verifiers.len() as u8,
                adjudicator,
                detection_permille: 900,
                reliance: 1_000,
            },
            worker: prover,
            amount,
            deadline: Pulse(ledger.pulse()?.0 + deadline),
            charter: None,
        },
        Vec::new(),
    )?;
    let task = view
        .effects
        .events
        .iter()
        .find_map(|event| match event {
            Event::TaskPosted { task, .. } => Some(*task),
            _ => None,
        })
        .ok_or_else(|| CliError::Transaction("the ledger posted no task".into()))?;
    // What the spec commits to, for provers and verifiers to read.
    let said = Request { task, wanted };
    let payload = serde_json::to_vec(&said)
        .map_err(|e| CliError::Usage(format!("the request could not be written: {e}")))?;
    let body = ClaimBody {
        schema: REQUEST_SCHEMA.into(),
        subject: format!("wants the World Proof of Pulse {pulse}"),
        payload: Payload::Public(payload),
        formula: sion_assurance::Formula::and(vec![formulas::dual_signature()]),
        adjudicator: None,
        detection_permille: 0,
        valid_for: 1_000_000,
        challenge_period: 0,
    };
    crate::vouching::publish(session, wallet, body, Some(request_key(&task)))?;
    Ok(task)
}

/// What the task `task` wants, as its poster said and its spec commits
/// to, with the task as the ledger holds it.
pub fn wanted(
    ledger: &dyn Ledger,
    task: ObjectId,
) -> Result<(Wanted, sion_exec::task::Task), CliError> {
    let state = crate::tasks::load_task(ledger, task)?;
    let not_one = || {
        CliError::Usage(format!(
            "task {} is not a request for a proof",
            task.0.short()
        ))
    };
    let claim = feeds::indexed(ledger, &state.poster, &request_key(&task))?.ok_or_else(not_one)?;
    let read = feeds::read::<Request>(ledger, claim, REQUEST_SCHEMA)?.ok_or_else(not_one)?;
    if read.issuer != state.poster
        || read.says.task != task
        || read.says.wanted.spec() != state.spec
    {
        return Err(not_one());
    }
    Ok((read.says.wanted, state))
}

/// The World Proof of `pulse` as this node serves it, in bytes. A node
/// keeps the proofs of its last Pulses only.
pub fn fetch(ledger: &dyn Ledger, pulse: u64) -> Result<Vec<u8>, CliError> {
    let none = |why: String| {
        CliError::Usage(format!(
            "this node has no World Proof of Pulse {pulse}. A node keeps the proofs of its last \
             {KEPT} Pulses only, and only if it proves. It said: {why}"
        ))
    };
    let entry = ledger
        .query(Query::World(Pulse(pulse)))
        .map_err(|e| none(e.to_string()))?;
    match entry.get("proof").and_then(|proof| proof.as_str()) {
        Some(hex) => hex::decode(hex).map_err(|e| CliError::Protocol(e.to_string())),
        None => Err(none(
            entry["error"].as_str().unwrap_or("nothing").to_string(),
        )),
    }
}

/// Who checks a delivered proof.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Checker {
    /// The prover, before it delivers: the Pulse's header is its node's
    /// word.
    Prover,
    /// A verifier: the Pulse's header is checked as a light client checks
    /// one, against the chain the request names. With `pinned`, that must
    /// be the chain the verifier knows; without, the verifier took the
    /// request and its chain from the node.
    Verifier {
        /// The chain identity the verifier trusts.
        pinned: Option<Hash32>,
    },
}

/// How many shares of a Pulse's data a verifier samples.
const SAMPLES: usize = 16;

/// How many Pulses back a proving node of this build still has a proof.
const KEPT: usize = sion_node::prover::PROOF_HISTORY;

/// Checks that `bytes` are what `wanted` asks for: a World Proof of that
/// Pulse of that chain, of its header, verified under both stacks. A
/// verifier first checks the Pulse's header as a light client checks one,
/// which takes a node that still serves the Pulse's data, and the lane
/// certificates after. Returns the checks, one line each.
pub fn checked(
    ledger: &dyn Ledger,
    wanted: &Wanted,
    bytes: &[u8],
    by: Checker,
) -> Result<Vec<String>, CliError> {
    if wanted.system != SYSTEM {
        return Err(CliError::Usage(format!(
            "the request asks for a proof of system {}, and this build checks only {SYSTEM}",
            wanted.system
        )));
    }
    let chain = Hash32::from_hex(&wanted.chain)
        .map_err(|e| CliError::Usage(format!("the request's chain: {e}")))?;
    let samples = match by {
        Checker::Prover => {
            if crate::verify::served_chain(ledger)? != chain {
                return Err(CliError::Verification(
                    "the request is for another chain than this node's".into(),
                ));
            }
            None
        }
        Checker::Verifier { pinned } => {
            if pinned.is_some_and(|mine| mine != chain) {
                return Err(CliError::Verification(format!(
                    "the request is for chain {}, not the chain you pinned",
                    chain.short()
                )));
            }
            Some(SAMPLES)
        }
    };
    let proof: WorldProof = decode(bytes)
        .map_err(|e| CliError::Verification(format!("these bytes are not a World Proof: {e}")))?;
    if proof.pulse.0 != wanted.pulse {
        return Err(CliError::Verification(format!(
            "this is the World Proof of Pulse {}, and the request is for Pulse {}",
            proof.pulse.0, wanted.pulse
        )));
    }
    let mut lines = crate::verify::verify_delivered(ledger, Some(chain), &proof, samples)?;
    if by == (Checker::Verifier { pinned: None }) {
        // The light client's first line says the chain matches the pinned
        // one. Here that is the request's, which came from the same node.
        if let Some(line) = lines.iter_mut().find(|line| line.starts_with("chain: ")) {
            *line = format!(
                "chain: {} is the chain the request names (NOT PINNED: the request came from \
                 the node itself; pass --chain {} to pin it)",
                chain.short(),
                chain.to_hex()
            );
        }
    }
    Ok(lines)
}

/// Attests, as `wallet`, one of the request's verifiers, that `bytes` are
/// the proof the task `task` wants: only after checking them
/// ([`checked`]). The attestation rests on the verifier's bond
/// (`bond:<verifier>:verifier`). With `pinned`, the chain the request
/// names must be that one. Returns the attestation and what was checked.
pub fn attest(
    session: &mut Session<'_>,
    wallet: &str,
    task: ObjectId,
    bytes: &[u8],
    pinned: Option<Hash32>,
) -> Result<(ClaimId, Vec<String>), CliError> {
    let ledger = session.ledger;
    let (wanted, state) = wanted(ledger, task)?;
    let me = session.id(wallet);
    if !state.acceptance.verifiers.contains(&me) {
        return Err(CliError::Usage(format!(
            "{wallet} is not a verifier of this request"
        )));
    }
    let lines = checked(ledger, &wanted, bytes, Checker::Verifier { pinned })?;
    let body = crate::tasks::attestation_body(
        ledger,
        task,
        &me,
        bytes,
        crate::tasks::VERIFIER_BOND_LABEL,
    )?;
    session.must(
        wallet,
        TxKind::IssueClaim {
            claim: body.clone(),
        },
        Vec::new(),
    )?;
    Ok((claim_id(&me, &body), lines))
}

/// Delivers `bytes` as the result of the task `task`, as `wallet`, with
/// the verifiers' `attestations`, and settles it: the prover is paid if
/// every attestation the request asked for is there. Returns what was
/// paid.
pub fn deliver(
    session: &mut Session<'_>,
    wallet: &str,
    task: ObjectId,
    bytes: &[u8],
    attestations: &[ClaimId],
) -> Result<Amount, CliError> {
    let ledger = session.ledger;
    let (wanted, _) = wanted(ledger, task)?;
    // A prover that delivers a proof no verifier would pass wastes its fee.
    checked(ledger, &wanted, bytes, Checker::Prover)?;
    let me = session.id(wallet);
    let result = crate::tasks::result_body(ledger, task, &me, bytes, attestations)?;
    session.must(
        wallet,
        TxKind::IssueClaim {
            claim: result.clone(),
        },
        Vec::new(),
    )?;
    let attested = attestations
        .iter()
        .map(|id| {
            crate::tasks::stored_claim(ledger, &id.as_hash().to_hex())
                .map(|claim| (claim.issuer, claim.body))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let view = session.send(
        wallet,
        TxKind::SettleTask {
            task,
            result,
            attestations: attested,
        },
        Vec::new(),
    )?;
    match &view.effects.status {
        ExecStatus::Success => {}
        ExecStatus::Failed(why) => {
            return Err(CliError::Transaction(format!(
                "the ledger did not settle the request: {why}"
            )))
        }
        ExecStatus::Rejected(why) => {
            return Err(CliError::Transaction(format!(
                "the ledger refused to settle the request: {why}"
            )))
        }
    }
    Ok(view
        .effects
        .events
        .iter()
        .find_map(|event| match event {
            Event::TaskSettled { amount, .. } => Some(*amount),
            _ => None,
        })
        .unwrap_or_default())
}

/// Where a request stands, in lines.
pub fn lines(ledger: &dyn Ledger, task: ObjectId) -> Result<Vec<String>, CliError> {
    let (wanted, state) = wanted(ledger, task)?;
    let mut lines = vec![format!(
        "request {} by {}: the World Proof of Pulse {} ({}), {} USD escrowed",
        task.0.short(),
        state.poster.short(),
        wanted.pulse,
        wanted.system,
        state.amount
    )];
    lines.push(format!(
        "  to be delivered by {} and attested by: {}",
        match &state.worker {
            Some(prover) => prover.short(),
            None => "anyone".to_string(),
        },
        state
            .acceptance
            .verifiers
            .iter()
            .map(|v| v.short())
            .collect::<Vec<_>>()
            .join(", ")
    ));
    lines.push(match state.state {
        TaskState::Open => format!("  open until Pulse {}", state.deadline.0),
        TaskState::Settled {
            worker, result, at, ..
        } => format!(
            "  delivered by {} and paid at Pulse {} (result {})",
            worker.short(),
            at.0,
            result.short()
        ),
        TaskState::Cancelled { by, at } => {
            format!(
                "  withdrawn by {} at Pulse {}: its escrow went back",
                by.short(),
                at.0
            )
        }
    });
    Ok(lines)
}

/// `sion proving`.
#[derive(Clone, Debug, Subcommand)]
pub enum ProvingCommand {
    /// Ask for the World Proof of a Pulse: escrow a payment for whoever
    /// delivers it, and name the verifiers who must each check and attest
    /// it before it is paid.
    Request {
        /// The Pulse (one to come, or a recent one: a node keeps the
        /// proofs of its last Pulses only).
        pulse: u64,
        /// What to pay for it, in USD.
        amount: String,
        /// A verifier (a wallet or an account; repeat for more). Each must
        /// attest the proof.
        #[arg(long = "verifier", value_name = "ACCOUNT", required = true)]
        verifiers: Vec<String>,
        /// The one prover who may deliver it (a wallet or an account).
        /// Without it anyone may, and whoever settles first with the
        /// attested proof is paid.
        #[arg(long, value_name = "ACCOUNT")]
        prover: Option<String>,
        /// The adjudicator atom the result and its attestations name
        /// (hex).
        #[arg(long)]
        adjudicator: Option<String>,
        /// How many Pulses the request stays open.
        #[arg(long, default_value_t = 500)]
        deadline: u64,
        /// Your wallet.
        #[arg(long)]
        wallet: String,
    },
    /// Fetch the World Proof a request wants from your node and check it,
    /// as a prover: the file is what you deliver.
    Fetch {
        /// The request (its task).
        task: String,
        /// The file to write the proof to.
        #[arg(long, value_name = "FILE")]
        out: PathBuf,
    },
    /// Check a delivered proof against a request, and attest it, as one
    /// of the request's verifiers: that it is of the certified header of
    /// that Pulse and verifies under both proof stacks. The attestation
    /// rests on your `verifier` bond.
    Attest {
        /// The request (its task).
        task: String,
        /// The proof.
        proof: PathBuf,
        /// The chain identity you trust. Without it the request's chain is
        /// taken from the node, which vouches for itself.
        #[arg(long, env = "SION_CHAIN")]
        chain: Option<String>,
        /// Your wallet.
        #[arg(long)]
        wallet: String,
    },
    /// Deliver a proof for a request with its verifiers' attestations, as
    /// its prover, and be paid.
    Deliver {
        /// The request (its task).
        task: String,
        /// The proof.
        proof: PathBuf,
        /// The verifiers' attestations (comma-separated claims).
        #[arg(long, value_delimiter = ',')]
        attestations: Vec<String>,
        /// Your wallet.
        #[arg(long)]
        wallet: String,
    },
    /// Where a request stands; with a proof, also whether it is what the
    /// request wants.
    Show {
        /// The request (its task).
        task: String,
        /// A proof to check against it, as a verifier checks one.
        #[arg(long, value_name = "FILE")]
        proof: Option<PathBuf>,
        /// The chain identity you trust, for checking a proof.
        #[arg(long, env = "SION_CHAIN")]
        chain: Option<String>,
    },
}

fn task_of(text: &str) -> Result<ObjectId, CliError> {
    Hash32::from_hex(text.trim())
        .map(ObjectId::from_hash)
        .map_err(|e| CliError::Usage(format!("{text:?} is not a task's id: {e}")))
}

fn chain_of(text: Option<String>) -> Result<Option<Hash32>, CliError> {
    text.map(|hex| {
        Hash32::from_hex(hex.trim())
            .map_err(|_| CliError::Usage("--chain takes 64 hex digits".into()))
    })
    .transpose()
}

fn read(path: &std::path::Path) -> Result<Vec<u8>, CliError> {
    std::fs::read(path).map_err(|e| CliError::Usage(format!("{}: {e}", path.display())))
}

/// Runs a proving command.
pub fn run(
    session: &mut Session<'_>,
    ledger: &dyn Ledger,
    command: ProvingCommand,
) -> Result<(), CliError> {
    match command {
        ProvingCommand::Request {
            pulse,
            amount,
            verifiers,
            prover,
            adjudicator,
            deadline,
            wallet,
        } => {
            let verifiers: Vec<AccountId> = verifiers.iter().map(|v| session.id(v)).collect();
            let prover = prover.map(|prover| session.id(&prover));
            let adjudicator = adjudicator
                .as_deref()
                .map(|hex| {
                    Hash32::from_hex(hex)
                        .map(AtomId::from_hash)
                        .map_err(|e| CliError::Usage(format!("the adjudicator's atom: {e}")))
                })
                .transpose()?;
            let amount = crate::agents::parse_amount(&amount)?;
            let count = verifiers.len();
            let task = request(
                session,
                &wallet,
                &Terms {
                    pulse,
                    amount,
                    verifiers,
                    prover,
                    adjudicator,
                    deadline,
                },
            )?;
            println!(
                "asked for the World Proof of Pulse {pulse} ({SYSTEM}): {amount} USD escrowed in \
                 task {task}"
            );
            println!(
                "  paid to {} once {count} verifier(s) attest it",
                match prover {
                    Some(prover) => format!("{} for delivering it", prover.short()),
                    None => "whoever delivers it".to_string(),
                }
            );
            let now = ledger.pulse()?.0;
            if pulse + KEPT as u64 <= now {
                println!(
                    "  note: Pulse {pulse} is {} Pulses back, and a node keeps the proofs of its \
                     last {KEPT} only: if no prover kept this one, cancel the request to take \
                     the escrow back (sion task cancel)",
                    now - pulse
                );
            }
            Ok(())
        }
        ProvingCommand::Fetch { task, out } => {
            let task = task_of(&task)?;
            let (wanted, _) = wanted(ledger, task)?;
            let bytes = fetch(ledger, wanted.pulse)?;
            for line in checked(ledger, &wanted, &bytes, Checker::Prover)? {
                println!("{line}");
            }
            std::fs::write(&out, &bytes)
                .map_err(|e| CliError::Usage(format!("{}: {e}", out.display())))?;
            println!(
                "the World Proof of Pulse {}, {} bytes: {}",
                wanted.pulse,
                bytes.len(),
                out.display()
            );
            Ok(())
        }
        ProvingCommand::Attest {
            task,
            proof,
            chain,
            wallet,
        } => {
            let (claim, lines) = attest(
                session,
                &wallet,
                task_of(&task)?,
                &read(&proof)?,
                chain_of(chain)?,
            )?;
            for line in lines {
                println!("{line}");
            }
            println!("attested: claim {}", claim.as_hash().to_hex());
            Ok(())
        }
        ProvingCommand::Deliver {
            task,
            proof,
            attestations,
            wallet,
        } => {
            let attestations = attestations
                .iter()
                .map(|hex| {
                    Hash32::from_hex(hex.trim())
                        .map(ClaimId::from_hash)
                        .map_err(|e| CliError::Usage(format!("an attestation: {e}")))
                })
                .collect::<Result<Vec<_>, _>>()?;
            let paid = deliver(
                session,
                &wallet,
                task_of(&task)?,
                &read(&proof)?,
                &attestations,
            )?;
            println!("delivered and settled: {paid} USD paid to {wallet}");
            Ok(())
        }
        ProvingCommand::Show { task, proof, chain } => {
            let task = task_of(&task)?;
            for line in lines(ledger, task)? {
                println!("{line}");
            }
            if let Some(path) = proof {
                let (wanted, _) = wanted(ledger, task)?;
                let pinned = chain_of(chain)?;
                for line in checked(ledger, &wanted, &read(&path)?, Checker::Verifier { pinned })? {
                    println!("  {line}");
                }
                println!("  the proof is what the request wants");
            }
            Ok(())
        }
    }
}
