//! Task Charters (spec v1.10): posting escrowed work whose acceptance is
//! fixed in advance, attesting and submitting results, settling and
//! cancelling.
//!
//! A worker's result claim must carry exactly the formula the task
//! requires (`sion_exec::task::result_formula`): the worker's task bond
//! (`bond:<worker>:task-worker`, see `sion bond`) or a named verifier's
//! attestation of the same output, and every required claim. These
//! commands build the claims the way the ledger checks them.
//!
//! A task may also fix a **check** when it is posted ([`Check`]): a command
//! and the files it runs among, committed in the task's spec
//! ([`CheckedSpec`]) and kept on the poster's machine for the task's
//! verifiers, not handed to the worker. `sion task verify` runs it on an
//! output in a sandbox, in a fresh directory, and attests the output only
//! if it passes, so that what a paid result is judged by is fixed before
//! the work starts, out of the worker's reach, and applied by someone
//! other than the worker. A task with a check must ask for an attestation.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use borsh::{BorshDeserialize, BorshSerialize};
use clap::Subcommand;
use serde_json::{json, Value};
use sion_assurance::{AtomClass, Policy, Ratio};
use sion_core::encode::{decode, encode};
use sion_core::hash::{hash_domain, Hash32};
use sion_core::ids::{AccountId, AtomId, ClaimId, ObjectId, Pulse};
use sion_exec::claim::{claim_id, formulas, ClaimBody, StoredClaim};
use sion_exec::execute::{bond_atom, Event};
use sion_exec::object::ObjectData;
use sion_exec::task::{
    result_formula, Acceptance, Task, TaskResult, TaskState, ATTESTATION_SCHEMA, RESULT_SCHEMA,
};
use sion_exec::{ExecStatus, TxKind};

use crate::error::CliError;
use crate::ledger::{Ledger, LedgerExt};
use crate::session::Session;

/// The bond label verifiers' attestations rest on by default.
pub const VERIFIER_BOND_LABEL: &str = "verifier";

/// The command a check runs unless its poster gives another.
pub const CHECK_COMMAND: &str = "sh check.sh";

/// The most files a check may hold.
pub const MAX_CHECK_FILES: usize = 1_000;

/// The most bytes a check's files may hold together.
pub const MAX_CHECK_BYTES: usize = 16 << 20;

/// Where the output a check is run on is, inside its sandbox (`$OUTPUT`).
pub const CHECKED_OUTPUT: &str = "/task/output";

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
        /// A check results must pass, fixed now: the files of this
        /// directory and --check-command, committed in the task's spec with
        /// the description --spec gives, and kept on this machine for the
        /// task's verifiers, who run it with `sion task verify`. A task with
        /// a check needs an attestation (--must-attest or
        /// --min-attestations).
        #[arg(long, value_name = "DIR")]
        check: Option<PathBuf>,
        /// The check's command, run with /bin/sh in a directory holding its
        /// files, the output to check at $OUTPUT.
        #[arg(long, default_value = CHECK_COMMAND, value_name = "COMMAND")]
        check_command: String,
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
    /// Run the check a task fixed when it was posted on an output, as one
    /// of its verifiers, and attest the output (on your bond
    /// `bond:<you>:verifier`) only if the check passes. The check runs in a
    /// sandbox with no network, in a fresh directory holding its files,
    /// with the output read-only at $OUTPUT; an exit status of 0 passes.
    Verify {
        /// The verifier's wallet.
        verifier: String,
        /// The task.
        task: String,
        /// The output: a file (its exact bytes), or the commitment of an
        /// output kept on this machine (as an agent's task_submit keeps
        /// it).
        output: String,
        /// The check's files, if the check is not kept on this machine: it
        /// must be the one the task fixed.
        #[arg(long, value_name = "DIR")]
        check: Option<PathBuf>,
        /// The check's command, with --check: the one the task was posted
        /// with.
        #[arg(long, default_value = CHECK_COMMAND, value_name = "COMMAND")]
        check_command: String,
        /// The task's description, with --check, if it is not kept on this
        /// machine.
        #[arg(long, value_name = "FILE")]
        spec: Option<PathBuf>,
        /// The bond label the attestation rests on.
        #[arg(long, default_value = VERIFIER_BOND_LABEL)]
        bond: String,
        /// What runs the check: `bwrap` (bubblewrap, the default) or
        /// `docker:<image>@sha256:<digest>`, an image already on this
        /// machine.
        #[arg(long, default_value = "bwrap", value_name = "SANDBOX")]
        sandbox: String,
        /// A directory the check may read, at its own path (a toolchain,
        /// say). Repeat for more.
        #[arg(long = "sandbox-read", value_name = "DIR")]
        sandbox_read: Vec<PathBuf>,
        /// Seconds the check may run before it is stopped (and fails).
        #[arg(long, default_value_t = 600, value_name = "SECONDS")]
        timeout: u64,
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
            check,
            check_command,
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
            // A check is fixed in the spec, with the description, and both
            // are kept here for the task's verifiers.
            let (spec, check) = match &check {
                Some(dir) => {
                    let description = read_file(spec.as_deref().ok_or_else(|| {
                        CliError::Usage(
                            "--check goes with --spec, the task's description, which the \
                             spec fixes with the check"
                                .into(),
                        )
                    })?)?;
                    let check = Check::from_dir(dir, &check_command)?;
                    let (spec, check) = commit_check(&description, &check, &acceptance)?;
                    (spec, Some(check))
                }
                None => (commitment(spec.as_deref(), "sion/task/spec")?, None),
            };
            let kind = TxKind::PostTask {
                spec,
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
                    if let Some(check) = check {
                        println!(
                            "check {check} (fixed in the task's spec, kept on this machine for \
                             its verifiers)"
                        );
                    }
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
        TaskCommand::Verify {
            verifier,
            task,
            output,
            check,
            check_command,
            spec,
            bond,
            sandbox,
            sandbox_read,
            timeout,
        } => {
            let task = parse_task(&task)?;
            let who = counted_verifier(session, ledger, task, &verifier)?;
            let given = Given {
                spec: spec.as_deref().map(read_file).transpose()?,
                check: check
                    .as_deref()
                    .map(|dir| Check::from_dir(dir, &check_command))
                    .transpose()?,
            };
            let sandbox = crate::runtime::sandbox_from(&sandbox, "none", sandbox_read, timeout)?;
            let output = output_bytes(&output)?;
            let verdict = run_committed_check(ledger, task, &output, given, &sandbox)?;
            if !verdict.passed {
                return Err(CliError::Verification(format!(
                    "the check did not pass, so nothing was attested. It reported:\n{}",
                    verdict.report.trim_end()
                )));
            }
            println!("the check passed:\n{}", verdict.report.trim_end());
            let body = attestation_body(ledger, task, &who, &output, &bond)?;
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

/// A check a task's results must pass, fixed in its spec when it is posted
/// ([`CheckedSpec`]): a command and the files it runs among. A verifier
/// runs it on an output ([`run_task_check`]) before attesting it.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Check {
    /// The command, run with /bin/sh.
    pub command: String,
    /// Its files, by their paths relative to the directory it runs in,
    /// `/`-separated.
    pub files: BTreeMap<String, Vec<u8>>,
}

impl Check {
    /// The check made of the files under `dir` (links are refused), run
    /// with `command`.
    pub fn from_dir(dir: &Path, command: &str) -> Result<Check, CliError> {
        if !dir.is_dir() {
            return Err(CliError::Usage(format!(
                "{} is not a directory of the check's files",
                dir.display()
            )));
        }
        let mut files = BTreeMap::new();
        collect(dir, dir, &mut files, &mut 0)?;
        let check = Check {
            command: command.trim().to_string(),
            files,
        };
        check.validate().map_err(CliError::Usage)?;
        Ok(check)
    }

    /// Whether it may be run: it has a command, at most
    /// [`MAX_CHECK_FILES`] files of [`MAX_CHECK_BYTES`] bytes in all, each
    /// at a plain relative path, and no file where another needs a
    /// directory.
    pub fn validate(&self) -> Result<(), String> {
        if self.command.trim().is_empty() {
            return Err("a check needs a command to run".into());
        }
        let bytes: usize = self.files.values().map(Vec::len).sum();
        if self.files.len() > MAX_CHECK_FILES || bytes > MAX_CHECK_BYTES {
            return Err(format!(
                "a check holds at most {MAX_CHECK_FILES} files of {} MiB in all",
                MAX_CHECK_BYTES >> 20
            ));
        }
        for path in self.files.keys() {
            if !plain_path(path) {
                return Err(format!(
                    "the check's file {path:?} is not at a plain relative path"
                ));
            }
            let mut at = 0;
            while let Some(slash) = path[at..].find('/') {
                let parent = &path[..at + slash];
                if self.files.contains_key(parent) {
                    return Err(format!(
                        "the check's file {parent:?} is also a directory of its files"
                    ));
                }
                at += slash + 1;
            }
        }
        Ok(())
    }

    /// The commitment a task's spec makes to it: to its command and to
    /// every file's path and exact bytes.
    pub fn commitment(&self) -> Hash32 {
        hash_domain("sion/task/check", &encode(self))
    }

    /// Writes its files under `dir`.
    pub fn write_to(&self, dir: &Path) -> Result<(), CliError> {
        self.validate().map_err(CliError::Usage)?;
        for (path, content) in &self.files {
            let file = dir.join(path);
            if let Some(parent) = file.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| CliError::Usage(format!("{}: {e}", parent.display())))?;
            }
            std::fs::write(&file, content)
                .map_err(|e| CliError::Usage(format!("{}: {e}", file.display())))?;
        }
        Ok(())
    }
}

/// Whether `path` is relative, `/`-separated, and goes only down: no
/// empty, `.` or `..` part, no backslash and no NUL.
fn plain_path(path: &str) -> bool {
    !path.contains(['\\', '\0'])
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

/// Adds the files under `dir` to `files`, by their paths below `root`,
/// counting their bytes in `bytes`.
fn collect(
    root: &Path,
    dir: &Path,
    files: &mut BTreeMap<String, Vec<u8>>,
    bytes: &mut u64,
) -> Result<(), CliError> {
    let unreadable =
        |e: std::io::Error| CliError::Usage(format!("cannot read {}: {e}", dir.display()));
    for entry in std::fs::read_dir(dir).map_err(unreadable)? {
        let entry = entry.map_err(unreadable)?;
        let path = entry.path();
        let kind = entry.file_type().map_err(unreadable)?;
        if kind.is_dir() {
            collect(root, &path, files, bytes)?;
            continue;
        }
        if !kind.is_file() {
            return Err(CliError::Usage(format!(
                "{} is not a file: a check holds files and directories, not links",
                path.display()
            )));
        }
        *bytes += entry.metadata().map_err(unreadable)?.len();
        if files.len() >= MAX_CHECK_FILES || *bytes > MAX_CHECK_BYTES as u64 {
            return Err(CliError::Usage(format!(
                "a check holds at most {MAX_CHECK_FILES} files of {} MiB in all",
                MAX_CHECK_BYTES >> 20
            )));
        }
        let parts = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .components()
            .map(|part| part.as_os_str().to_str().map(str::to_string))
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| CliError::Usage(format!("{} is not named in UTF-8", path.display())))?;
        files.insert(parts.join("/"), read_file(&path)?);
    }
    Ok(())
}

/// The spec of a task posted with a check: its description and the
/// commitment to its check. The task holds the commitment to this spec
/// ([`CheckedSpec::commitment`]), so its check cannot change once it is
/// posted, and a verifier given the description and the check can tell
/// that they are the ones the task fixed.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct CheckedSpec {
    /// The task's description, its exact bytes.
    pub description: Vec<u8>,
    /// The commitment to its check ([`Check::commitment`]).
    pub check: Hash32,
}

impl CheckedSpec {
    /// The commitment a task makes to it: what the task's `spec` holds.
    pub fn commitment(&self) -> Hash32 {
        hash_domain("sion/task/spec/checked", &encode(self))
    }
}

/// What is kept on this machine under `commitment` (in
/// `$SION_HOME/agents/tasks`), if anything.
pub fn kept(commitment: &Hash32) -> Option<Vec<u8>> {
    std::fs::read(crate::delegation::tasks_dir().join(commitment.to_hex())).ok()
}

/// The spec with a check that `task` fixed, if it is kept on this machine.
pub fn kept_spec(task: &Task) -> Option<CheckedSpec> {
    let spec: CheckedSpec = decode(&kept(&task.spec)?).ok()?;
    (spec.commitment() == task.spec).then_some(spec)
}

/// The check kept on this machine under `commitment`, if any.
pub fn kept_check(commitment: &Hash32) -> Option<Check> {
    let check: Check = decode(&kept(commitment)?).ok()?;
    (check.commitment() == *commitment).then_some(check)
}

/// Fixes `check` with `description` in a task's spec: keeps both on this
/// machine for the task's verifiers, and returns the commitments to the
/// spec (what the task holds) and to the check. A check is run by those
/// who attest results, so `acceptance` must ask for an attestation.
pub fn commit_check(
    description: &[u8],
    check: &Check,
    acceptance: &Acceptance,
) -> Result<(Hash32, Hash32), CliError> {
    check.validate().map_err(CliError::Usage)?;
    if acceptance.must_attest.is_empty() && acceptance.min_attestations == 0 {
        return Err(CliError::Usage(
            "a check is run by the verifiers who attest results, so a task with one must ask \
             for an attestation: name a verifier who must attest every result, or how many \
             attestations a result needs"
                .into(),
        ));
    }
    let check_commitment = check.commitment();
    let spec = CheckedSpec {
        description: description.to_vec(),
        check: check_commitment,
    };
    let spec_commitment = spec.commitment();
    for (bytes, commitment) in [
        (encode(check), check_commitment),
        (encode(&spec), spec_commitment),
    ] {
        crate::delegation::keep(&bytes, &commitment).map_err(CliError::Usage)?;
    }
    Ok((spec_commitment, check_commitment))
}

/// A task's description and check as a verifier was given them, when they
/// are not kept on its machine.
#[derive(Clone, Debug, Default)]
pub struct Given {
    /// The task's description, its exact bytes (with the check).
    pub spec: Option<Vec<u8>>,
    /// The check.
    pub check: Option<Check>,
}

/// The check `task` fixed when it was posted, from what is kept on this
/// machine or `given`: refused unless the task's spec commits to it.
pub fn committed_check(
    ledger: &dyn Ledger,
    task: ObjectId,
    given: Given,
) -> Result<Check, CliError> {
    let state = load_task(ledger, task)?;
    let short = task.short();
    let spec = match (given.spec, &given.check) {
        (Some(description), Some(check)) => {
            let spec = CheckedSpec {
                description,
                check: check.commitment(),
            };
            if spec.commitment() != state.spec {
                return Err(CliError::Verification(format!(
                    "the description and the check given are not the ones task {short} fixed \
                     when it was posted"
                )));
            }
            spec
        }
        (Some(_), None) => {
            return Err(CliError::Usage(
                "--spec goes with --check, the check's files".into(),
            ))
        }
        (None, _) => kept_spec(&state).ok_or_else(|| {
            CliError::Usage(format!(
                "task {short}'s spec is not kept on this machine as one with a check: was it \
                 posted with one? Give its description and its check (--spec, --check)"
            ))
        })?,
    };
    let check = match given.check {
        Some(check) => check,
        None => kept_check(&spec.check).ok_or_else(|| {
            CliError::Usage(format!(
                "task {short}'s check {} is not kept on this machine: give its files (--check)",
                spec.check.short()
            ))
        })?,
    };
    if check.commitment() != spec.check {
        return Err(CliError::Verification(format!(
            "the check given is not the one task {short} fixed when it was posted: its spec \
             commits to check {}, and the one given is {}",
            spec.check.short(),
            check.commitment().short()
        )));
    }
    Ok(check)
}

/// Runs `check` on `output` for `task` in `sandbox`: in a fresh directory
/// holding its files, read-write at the sandbox's workspace, with the
/// output read-only at [`CHECKED_OUTPUT`] (`$OUTPUT`) and the task's id in
/// `$TASK`. It passes if it exits with status 0. The directory is removed
/// after.
pub fn run_task_check(
    sandbox: &sion_agent::sandbox::Sandbox,
    check: &Check,
    task: ObjectId,
    output: &[u8],
) -> Result<sion_agent::verify::Verdict, CliError> {
    use sion_agent::sandbox::{Mount, WORKSPACE};
    use sion_agent::verify::{score_of, Verdict};
    let base = std::env::temp_dir().join(format!("sion-check-{}", crate::runtime::new_id()));
    let (work, given) = (base.join("work"), base.join("task"));
    let ran = (|| -> Result<Verdict, CliError> {
        for dir in [&work, &given] {
            std::fs::create_dir_all(dir)
                .map_err(|e| CliError::Usage(format!("{}: {e}", dir.display())))?;
        }
        check.write_to(&work)?;
        std::fs::write(given.join("output"), output)
            .map_err(|e| CliError::Usage(format!("{}: {e}", given.display())))?;
        let script = format!(
            "OUTPUT={CHECKED_OUTPUT}; TASK={}; export OUTPUT TASK\n{}",
            task.0.to_hex(),
            check.command
        );
        let mounts = [
            Mount::workspace(&work),
            Mount {
                host: given.clone(),
                inside: "/task".into(),
                writable: false,
            },
        ];
        Ok(match sandbox.run(&mounts, WORKSPACE, &script) {
            Ok(run) => Verdict {
                passed: run.succeeded(),
                score: score_of(&run.stdout),
                report: run.render(),
            },
            Err(e) => Verdict {
                passed: false,
                score: None,
                report: format!("the check could not run: {e}"),
            },
        })
    })();
    let _ = std::fs::remove_dir_all(&base);
    ran
}

/// The check `task` fixed ([`committed_check`]), run on `output`
/// ([`run_task_check`]).
pub fn run_committed_check(
    ledger: &dyn Ledger,
    task: ObjectId,
    output: &[u8],
    given: Given,
    sandbox: &sion_agent::sandbox::Sandbox,
) -> Result<sion_agent::verify::Verdict, CliError> {
    let check = committed_check(ledger, task, given)?;
    run_task_check(sandbox, &check, task, output)
}

/// `verifier`'s account, if its attestations count for `task`: the task is
/// open and names it among its verifiers.
pub fn counted_verifier(
    session: &mut Session<'_>,
    ledger: &dyn Ledger,
    task: ObjectId,
    verifier: &str,
) -> Result<AccountId, CliError> {
    let state = load_task(ledger, task)?;
    if state.state != TaskState::Open {
        return Err(CliError::Usage(format!(
            "task {} is not open: there is nothing left to attest",
            task.short()
        )));
    }
    let who = session.id(verifier);
    if !state.acceptance.verifiers.contains(&who) {
        return Err(CliError::Usage(format!(
            "{verifier} is not one of task {}'s verifiers: its attestation would not count",
            task.short()
        )));
    }
    Ok(who)
}

/// What a verifier's check of an output came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verified {
    /// How the check ended, and what it printed.
    pub report: String,
    /// The attestation issued, if the check passed.
    pub attestation: Option<ClaimId>,
}

/// `verifier`'s check of `output` for `task`: the check the task fixed is
/// run in `sandbox` ([`run_committed_check`]), and only if it passes is the
/// output attested, on the verifier's bond `bond:<verifier>:<bond_label>`.
#[allow(clippy::too_many_arguments)]
pub fn verify_output(
    session: &mut Session<'_>,
    ledger: &dyn Ledger,
    verifier: &str,
    task: ObjectId,
    output: &[u8],
    given: Given,
    bond_label: &str,
    sandbox: &sion_agent::sandbox::Sandbox,
) -> Result<Verified, CliError> {
    let who = counted_verifier(session, ledger, task, verifier)?;
    let verdict = run_committed_check(ledger, task, output, given, sandbox)?;
    if !verdict.passed {
        return Ok(Verified {
            report: verdict.report,
            attestation: None,
        });
    }
    let body = attestation_body(ledger, task, &who, output, bond_label)?;
    let id = claim_id(&who, &body);
    let view = session.send(verifier, TxKind::IssueClaim { claim: body }, Vec::new())?;
    if view.status != ExecStatus::Success {
        return Err(CliError::Transaction(format!(
            "the check passed, but the attestation was refused: {}",
            crate::describe::receipt(&session.names(), &view.effects)
        )));
    }
    Ok(Verified {
        report: verdict.report,
        attestation: Some(id),
    })
}

/// An output given as a file, or as the commitment of one kept on this
/// machine.
fn output_bytes(given: &str) -> Result<Vec<u8>, CliError> {
    let path = Path::new(given);
    if path.is_file() {
        return read_file(path);
    }
    match Hash32::from_hex(given.trim()) {
        Ok(commitment) => kept(&commitment)
            .filter(|bytes| output_commitment(bytes) == commitment)
            .ok_or_else(|| {
                CliError::Usage(format!(
                    "no output {} is kept on this machine",
                    commitment.short()
                ))
            }),
        Err(_) => Err(CliError::Usage(format!(
            "cannot read {given}: give a file, or the commitment of an output kept on this \
             machine"
        ))),
    }
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

    fn check(files: &[(&str, &str)]) -> Check {
        Check {
            command: CHECK_COMMAND.into(),
            files: files
                .iter()
                .map(|(path, content)| (path.to_string(), content.as_bytes().to_vec()))
                .collect(),
        }
    }

    #[test]
    fn a_check_commits_to_its_command_and_every_file() {
        let base = check(&[("check.sh", "grep -q fixed \"$OUTPUT\"\n")]);
        let mut other_command = base.clone();
        other_command.command = "sh other.sh".into();
        let mut other_bytes = base.clone();
        other_bytes
            .files
            .insert("check.sh".into(), b"exit 0\n".to_vec());
        let mut more = base.clone();
        more.files
            .insert("tests/case.txt".into(), b"1,2,\n".to_vec());
        let commitments: BTreeSet<Hash32> = [&base, &other_command, &other_bytes, &more]
            .iter()
            .map(|c| c.commitment())
            .collect();
        assert_eq!(commitments.len(), 4);
        // A spec with a check commits to it, apart from a plain spec of the
        // same description.
        let spec = CheckedSpec {
            description: b"Fix the parser.".to_vec(),
            check: base.commitment(),
        };
        let other = CheckedSpec {
            check: more.commitment(),
            ..spec.clone()
        };
        assert_ne!(spec.commitment(), other.commitment());
        assert_ne!(spec.commitment(), spec_commitment(b"Fix the parser."));
    }

    #[test]
    fn a_check_is_read_from_files_and_written_back_as_it_was() {
        let dir = std::env::temp_dir().join(format!("sion-check-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let given = check(&[
            ("check.sh", "sh tests/run.sh \"$OUTPUT\"\n"),
            ("tests/run.sh", "grep -q ok \"$1\"\n"),
            ("tests/data/cases.txt", "a,b,\n"),
        ]);
        given.write_to(&dir.join("from")).unwrap();
        let read = Check::from_dir(&dir.join("from"), CHECK_COMMAND).unwrap();
        assert_eq!(read, given);
        assert_eq!(read.commitment(), given.commitment());
        // A link is refused: what it points to could change.
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/etc/hostname", dir.join("from/link")).unwrap();
            let refused = Check::from_dir(&dir.join("from"), CHECK_COMMAND);
            assert!(
                refused.is_err_and(|e| e.to_string().contains("not links")),
                "a link was taken into a check"
            );
        }
        assert!(Check::from_dir(&dir.join("none"), CHECK_COMMAND).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_check_writes_only_below_its_directory() {
        for path in [
            "../escape.sh",
            "/etc/passwd",
            "a//b",
            "./a",
            "a/../b",
            "a\\b",
            "",
        ] {
            assert!(
                check(&[(path, "x")]).validate().is_err(),
                "{path:?} was taken as a check's file"
            );
        }
        assert!(check(&[("a", "x"), ("a/b", "y")]).validate().is_err());
        assert!(check(&[("a/b", "x"), ("a/c", "y")]).validate().is_ok());
        let mut no_command = check(&[("check.sh", "exit 0")]);
        no_command.command = " ".into();
        assert!(no_command.validate().is_err());
    }
}
