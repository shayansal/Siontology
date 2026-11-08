//! `sion agent bench`: the benchmark suites built in, runs of them with
//! the operator's models, and the records of those runs.
//!
//! A run is kept under `$SION_HOME/agents/bench/<id>/`: `record.json` (the
//! suite, its digest, the models, the figures and each case's runs) and
//! `log.jsonl` (each run whole), the log fixed by its digest in the record.

use std::path::{Path, PathBuf};

use sion_agent::bench::{self, BenchRun, SUITES};
use sion_agent::compare::TaskSet;

use crate::error::CliError;

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
}

impl BenchCommand {
    /// Whether it needs the ledger.
    pub fn needs_ledger(&self) -> bool {
        false
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
