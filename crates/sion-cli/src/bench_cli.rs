//! `sion agent bench`: the benchmark suites built in, runs of them with
//! the operator's models, and the records of those runs.
//!
//! A run is kept under `$SION_HOME/agents/bench/<id>/`: `record.json` (the
//! suite, its digest, the models, the figures and each case's runs) and
//! `log.jsonl` (each run whole), the log fixed by its digest in the record.
//!
//! Published, a run is a claim on the ledger signed by the publisher's
//! wallet ([`bench::CLAIM_SCHEMA`]): the suite and its digest, the models,
//! the pass rate, pass^k, tokens and cost, and the log's digest. Anyone
//! given the log checks the claim against it ([`bench::check_claim`]):
//! that it is the log the claim names, that every run in it, judged again
//! by the suite's checks, comes out as it says, and that the figures are
//! the claim's.

use std::path::{Path, PathBuf};

use sion_agent::bench::{self, BenchRun, SUITES};
use sion_agent::compare::TaskSet;

use crate::error::CliError;
use crate::ledger::Ledger;
use crate::session::Session;

/// Benchmark suites: what an agent must get right, with stand-in tools.
#[derive(Debug, clap::Subcommand)]
pub enum BenchCommand {
    /// The suites built in, a line each.
    List,
    /// A suite as JSON: one built in by name, to read or start one of your
    /// own from.
    Show {
        /// The suite's name.
        suite: String,
    },
    /// Run a suite with your models: each case several times, each run
    /// with fresh models and the case's stand-in tools (nothing is paid or
    /// sent but the model calls), and report the pass rate, pass^k, the
    /// tokens and the cost. The run is kept, with its log.
    Run {
        /// A suite built in, or a JSON file of one (a set of tasks, as
        /// `sion agent graph eval` takes).
        suite: String,
        /// A model to run with, in order of preference (repeat for more).
        #[arg(
            long = "model",
            value_name = "MODEL",
            required_unless_present = "profile"
        )]
        models: Vec<String>,
        /// Run with a profile's models instead.
        #[arg(long, conflicts_with = "models")]
        profile: Option<String>,
        /// How many times each case is run (pass^k is reported for k this).
        #[arg(long, default_value_t = 3)]
        repeats: u32,
        /// The most turns a run may take.
        #[arg(long)]
        max_turns: Option<u32>,
    },
    /// The runs kept, newest first.
    Runs,
    /// Publish a kept run on the ledger, as a claim signed by your wallet:
    /// the suite and its digest, the models, the pass rate, pass^k, the
    /// tokens and the cost, and the digest of the run's log, which you
    /// give to whoever is to check it.
    Publish {
        /// The run: its directory, or the start of its name
        /// (`sion agent bench runs`).
        run: String,
        /// The wallet that signs the claim.
        #[arg(long)]
        wallet: String,
    },
    /// Check a published run: the claim on the ledger, who signed it and
    /// when, and, given its log, that the log is the one it names, that
    /// every run in it judged again by the suite's checks comes out as it
    /// says, and that the figures are the claim's.
    Check {
        /// The claim's id.
        claim: String,
        /// The run's log (by default, a run kept here with the log the
        /// claim names).
        #[arg(long, value_name = "FILE")]
        log: Option<PathBuf>,
        /// The suite, if it is not one built in.
        #[arg(long, value_name = "FILE")]
        suite: Option<PathBuf>,
    },
}

impl BenchCommand {
    /// Whether it needs the ledger.
    pub fn needs_ledger(&self) -> bool {
        matches!(
            self,
            BenchCommand::Publish { .. } | BenchCommand::Check { .. }
        )
    }
}

/// Where runs are kept.
pub fn dir() -> PathBuf {
    crate::runtime::sion_home().join("agents").join("bench")
}

/// A suite's name, its parsed cases and exact bytes: built in, or a file.
pub fn suite(name: &str) -> Result<(TaskSet, Vec<u8>), CliError> {
    let bytes = match bench::builtin(name) {
        Some(bytes) => bytes.as_bytes().to_vec(),
        None if Path::new(name).is_file() => {
            std::fs::read(name).map_err(|e| CliError::Usage(format!("{name}: {e}")))?
        }
        None => {
            return Err(CliError::Usage(format!(
                "no suite {name}: give one built in ({}) or a file",
                SUITES.map(|(n, _)| n).join(", ")
            )))
        }
    };
    let set = TaskSet::parse(&bytes).map_err(CliError::Usage)?;
    Ok((set, bytes))
}

/// Runs `suite` with `models` `repeats` times a case; the record, and
/// where it was kept.
pub fn run(
    suite_name: &str,
    models: &[String],
    repeats: u32,
    max_turns: Option<u32>,
) -> Result<(BenchRun, PathBuf), CliError> {
    let (set, bytes) = suite(suite_name)?;
    if models.is_empty() {
        return Err(CliError::Usage("give a --model to run with".into()));
    }
    let specs = models.to_vec();
    let fresh = move || -> Result<Vec<Box<dyn sion_agent::model::Model>>, String> {
        specs
            .iter()
            .map(|spec| crate::runtime::model(spec, 0).map_err(|e| e.to_string()))
            .collect()
    };
    let prices = crate::runtime::prices()?;
    let mut limits = sion_agent::Limits::default();
    if let Some(turns) = max_turns {
        limits.max_turns = turns;
    }
    let (record, log) = bench::run(
        &set,
        &bytes,
        &fresh,
        models,
        repeats,
        Some(&prices),
        &limits,
    )
    .map_err(CliError::Usage)?;
    let kept = keep(&record, &log)?;
    Ok((record, kept))
}

/// Keeps `record` and `log` in a directory of their own; returns it.
pub fn keep(record: &BenchRun, log: &[u8]) -> Result<PathBuf, CliError> {
    let at = dir().join(format!("{}-{}", record.suite, &record.log.to_hex()[..16]));
    std::fs::create_dir_all(&at).map_err(|e| CliError::Usage(format!("{}: {e}", at.display())))?;
    let text = serde_json::to_string_pretty(record).map_err(|e| CliError::Usage(e.to_string()))?;
    for (name, bytes) in [("log.jsonl", log), ("record.json", text.as_bytes())] {
        let path = at.join(name);
        std::fs::write(&path, bytes)
            .map_err(|e| CliError::Usage(format!("{}: {e}", path.display())))?;
    }
    Ok(at)
}

/// A kept run: its record and log, from its directory, or the start of
/// its name under [`dir`].
pub fn kept(run: &str) -> Result<(BenchRun, Vec<u8>, PathBuf), CliError> {
    let at = if Path::new(run).is_dir() {
        PathBuf::from(run)
    } else {
        let mut found: Vec<PathBuf> = std::fs::read_dir(dir())
            .map(|entries| {
                entries
                    .flatten()
                    .filter(|e| e.file_name().to_string_lossy().starts_with(run))
                    .map(|e| e.path())
                    .collect()
            })
            .unwrap_or_default();
        match found.len() {
            1 => found.remove(0),
            0 => return Err(CliError::Usage(format!("no benchmark run {run}"))),
            _ => return Err(CliError::Usage(format!("{run} names more than one run"))),
        }
    };
    let read = |name: &str| {
        let path = at.join(name);
        std::fs::read(&path).map_err(|e| CliError::Usage(format!("{}: {e}", path.display())))
    };
    let record: BenchRun = serde_json::from_slice(&read("record.json")?)
        .map_err(|e| CliError::Usage(format!("{}: {e}", at.display())))?;
    Ok((record, read("log.jsonl")?, at))
}

/// The index key the latest published run of `suite` is named under.
pub fn claim_key(suite: &str) -> String {
    format!("agent-bench:{suite}")
}

/// Publishes the kept run `run` as `wallet`'s claim; returns its id, and
/// keeps it beside the run.
pub fn publish(session: &mut Session<'_>, wallet: &str, run: &str) -> Result<String, CliError> {
    let (record, log, at) = kept(run)?;
    bench::verify(&record, &log, None).map_err(CliError::Usage)?;
    let claim = bench::BenchClaim::of(&record);
    let payload = serde_json::to_vec(&claim).map_err(|e| CliError::Usage(e.to_string()))?;
    let body = sion_exec::claim::ClaimBody {
        schema: bench::CLAIM_SCHEMA.into(),
        subject: format!(
            "a run of the benchmark suite {}: {} of {} runs passed",
            record.suite,
            record.passed(),
            record.total()
        ),
        payload: sion_exec::claim::Payload::Public(payload),
        // A commitment rests on its issuer's signature alone: it says what
        // the issuer ran and saw, which the log, not the ledger, shows.
        formula: sion_assurance::Formula::and(vec![sion_exec::claim::formulas::dual_signature()]),
        adjudicator: None,
        detection_permille: 0,
        valid_for: crate::sealed::VALID_FOR,
        challenge_period: 0,
    };
    let id = crate::vouching::publish(session, wallet, body, Some(claim_key(&record.suite)))?;
    let id = id.as_hash().to_hex();
    let published = serde_json::json!({"claim": id, "wallet": wallet});
    let path = at.join("published.json");
    std::fs::write(&path, published.to_string())
        .map_err(|e| CliError::Usage(format!("{}: {e}", path.display())))?;
    Ok(id)
}

/// The lines of a check of the claim `id`, against `log` (or the run kept
/// here whose log it names) and its suite.
pub fn check(
    ledger: &dyn Ledger,
    id: &str,
    log: Option<&Path>,
    suite_file: Option<&Path>,
) -> Result<Vec<String>, CliError> {
    let stored = crate::tasks::stored_claim(ledger, id)?;
    if stored.body.schema != bench::CLAIM_SCHEMA {
        return Err(CliError::Usage(format!(
            "claim {id} is a {}, not a benchmark run",
            stored.body.schema
        )));
    }
    let sion_exec::claim::Payload::Public(bytes) = &stored.body.payload else {
        return Err(CliError::Usage(format!(
            "claim {id}'s payload is not public"
        )));
    };
    let claim: bench::BenchClaim = serde_json::from_slice(bytes)
        .map_err(|e| CliError::Protocol(format!("claim {id}: {e}")))?;
    let mut lines = vec![
        format!(
            "claim {} by account {}, issued at Pulse {}",
            &id[..16.min(id.len())],
            stored.issuer,
            stored.issued_at.0
        ),
        claim.line(),
        format!(
            "suite {} ({}), log {}",
            claim.suite,
            &claim.suite_digest.to_hex()[..16],
            claim.log.to_hex()
        ),
    ];
    let log_bytes = match log {
        Some(path) => Some(
            std::fs::read(path).map_err(|e| CliError::Usage(format!("{}: {e}", path.display())))?,
        ),
        None => kept(&format!("{}-{}", claim.suite, &claim.log.to_hex()[..16]))
            .ok()
            .map(|(_, log, _)| log),
    };
    let Some(log_bytes) = log_bytes else {
        lines.push(
            "no log given, nor kept here: the claim says what its issuer saw, unchecked (give --log)"
                .into(),
        );
        return Ok(lines);
    };
    let suite_bytes = match suite_file {
        Some(path) => Some(
            std::fs::read(path).map_err(|e| CliError::Usage(format!("{}: {e}", path.display())))?,
        ),
        None => bench::builtin(&claim.suite)
            .map(|s| s.as_bytes().to_vec())
            .filter(|bytes| sion_agent::evals::digest(bytes) == claim.suite_digest),
    };
    match bench::check_claim(&claim, &log_bytes, suite_bytes.as_deref()) {
        Ok(run) => {
            lines.push(if suite_bytes.is_some() {
                format!(
                    "checked: the log is the one the claim names, its {} runs judged again by                      the suite's checks come out as it says, and its figures are the claim's",
                    run.total()
                )
            } else {
                format!(
                    "checked against the log: it is the one the claim names and its {} runs                      come to the claim's figures; the suite was not at hand to judge them again                      (give --suite)",
                    run.total()
                )
            });
        }
        Err(why) => lines.push(format!("does not check: {why}")),
    }
    Ok(lines)
}

/// Runs a bench command that needs the ledger.
pub fn run_ledger(
    session: &mut Session<'_>,
    ledger: &dyn Ledger,
    command: BenchCommand,
) -> Result<(), CliError> {
    match command {
        BenchCommand::Publish { run, wallet } => {
            let id = publish(session, &wallet, &run)?;
            println!("published as claim {id}");
        }
        BenchCommand::Check { claim, log, suite } => {
            let lines = check(ledger, &claim, log.as_deref(), suite.as_deref())?;
            let failed = lines.iter().any(|l| l.starts_with("does not check"));
            for line in lines {
                println!("{line}");
            }
            if failed {
                return Err(CliError::Usage("the claim does not check".into()));
            }
        }
        other => run_local(other)?,
    }
    Ok(())
}

/// Runs a bench command that needs no ledger.
pub fn run_local(command: BenchCommand) -> Result<(), CliError> {
    match command {
        BenchCommand::List => {
            for (name, bytes) in SUITES {
                let set = TaskSet::parse(bytes.as_bytes()).map_err(CliError::Usage)?;
                println!("{name:<24}{} cases: {}", set.cases.len(), set.about);
            }
        }
        BenchCommand::Show { suite: name } => {
            let (_, bytes) = suite(&name)?;
            print!("{}", String::from_utf8_lossy(&bytes));
        }
        BenchCommand::Run {
            suite: name,
            models,
            profile,
            repeats,
            max_turns,
        } => {
            let models = match profile {
                Some(profile) => crate::profiles::get(&profile)?.models,
                None => models,
            };
            let (record, at) = run(&name, &models, repeats, max_turns)?;
            for line in record.lines() {
                println!("{line}");
            }
            println!("kept in {}", at.display());
        }
        BenchCommand::Publish { .. } | BenchCommand::Check { .. } => {
            return Err(CliError::Usage(
                "publishing and checking a run need the ledger".into(),
            ))
        }
        BenchCommand::Runs => {
            let mut runs: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(dir())
                .map(|entries| {
                    entries
                        .flatten()
                        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
                        .collect()
                })
                .unwrap_or_default();
            runs.sort_by_key(|run| std::cmp::Reverse(run.0));
            for (_, path) in runs {
                let name = path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                match kept(&path.to_string_lossy()) {
                    Ok((record, _, _)) => println!(
                        "{name}  {} of {} passed  {}",
                        record.passed(),
                        record.total(),
                        record.models.join(", ")
                    ),
                    Err(e) => println!("{name}  unreadable: {e}"),
                }
            }
        }
    }
    Ok(())
}
