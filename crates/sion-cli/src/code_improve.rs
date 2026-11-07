//! The agent changing its own code: a coding agent edits a copy of the
//! source, the change is built, tested and scored in a sandbox, and one
//! that is confirmed better on sealed tasks becomes a release — a patch,
//! signed by its publisher on the ledger, that others can rebuild, score
//! and attest before anyone installs it.
//!
//! A round:
//!
//! 1. **The coder** — a model with file and code tools — works in a copy
//!    of the source without its `.git`, its commands run in a sandbox
//!    with no network ([`sion_agent::sandbox`]). It is shown how the agent
//!    built from the source did on the tasks changes are chosen on, and
//!    which it failed; never the sealed tasks.
//! 2. **What it changed** is read by the harness, not by anything the
//!    coder ran: its files are copied into a fresh clone the harness made,
//!    and the diff taken there with git's hooks, filters, external diffs
//!    and the user's own git settings turned off. A copy holding a
//!    symbolic link, or a `.gitattributes` or `.gitmodules` of its own,
//!    is refused before git sees it.
//! 3. **Gates that hold whatever it says** ([`sion_agent::guard`]): only
//!    the paths it may change (by default the agent's runtime), never what
//!    scores or guards the agent, the build's configuration or the money
//!    and keys; at most so many files and lines; no `unsafe`, no network,
//!    no new processes in the lines it adds.
//! 4. **Build and tests**, in the sandbox, must pass.
//! 5. **Scoring.** The candidate runs each case in the sandbox, with the
//!    source mounted read-only and no network, and speaks to the harness
//!    over its standard input and output ([`sion_agent::remote`]): every
//!    model answer and tool answer it gets, the harness gives it, and the
//!    checks are the harness's over what it answered. The candidate has
//!    no say in its own score.
//! 6. **Screening** on the tasks changes are chosen on, against the
//!    agent built from the code as it stands, and **confirmation** on the
//!    sealed tasks at the seal's corrected level, as for any change
//!    ([`crate::improve_loop`]).
//!
//! A confirmed change is a [`Release`]: the commit it applies to, the
//! patch's digest, the commands, the agent it was scored with and the
//! evidence. With a wallet it is published as a claim ([`RELEASE_SCHEMA`]),
//! which the ledger's transaction signs. Anyone holding the source, the
//! patch and the sealed tasks can rebuild and score it ([`attest`]) and
//! publish what they found ([`ATTESTATION_SCHEMA`]). With an adjudicator,
//! the attestation is a claim on the attester's bond ([`ATTESTER_BOND`]):
//! anyone who rebuilds the release and finds otherwise can challenge it,
//! and an attestation the adjudicator rules against is shown false and
//! the bond slashed. [`install`] applies a release to a checkout only when
//! its claim, its patch and the attestations of verifiers its owner names
//! agree, each still standing, and, when told to, each on its attester's
//! bond; nothing installs itself.

use std::collections::BTreeSet;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sion_agent::compare::{newcombe, newcombe_with, Arm, ArmResult, TaskSet, Trial};
use sion_agent::guard::{self, Changed, PatchRules};
use sion_agent::model::Model;
use sion_agent::policy::{Autonomy, Policy};
use sion_agent::remote::{serve, ServeLimits, MAX_LINE};
use sion_agent::sandbox::{Mount, Network, Sandbox, WORKSPACE};
use sion_agent::session::{Limits, Runtime, State};
use sion_agent::tools::Toolbox;
use sion_core::hash::Hash32;
use sion_core::ids::{ClaimId, ObjectId};
use sion_exec::claim::Payload;

use crate::error::CliError;
use crate::improve_loop::{self, Problem, Scored, Variant};
use crate::profiles::Profile;
use crate::self_improve::Outcome;
use crate::session::Session;

/// The schema of a claim publishing a release of an agent's own code.
pub const RELEASE_SCHEMA: &str = "agent.release/v1";

/// The schema of a claim that a release was rebuilt, scored and found as
/// its publisher said.
pub const ATTESTATION_SCHEMA: &str = "agent.release-attestation/v1";

/// The bond an attester stands behind a bonded attestation with.
pub const ATTESTER_BOND: &str = "agent-attester";

/// The formula of an attestation on `attester`'s bond: its signature, and
/// the bond an adjudicator can rule against.
pub fn bonded_attestation(attester: &sion_core::ids::AccountId) -> sion_assurance::Formula {
    sion_exec::claim::formulas::bonded_statement(
        sion_exec::execute::bond_atom(sion_assurance::AtomClass::Economic, attester, ATTESTER_BOND)
            .id,
    )
}

/// The coder's instructions.
pub const CODER_SYSTEM: &str = "You improve the code of an AI agent's harness: the program that \
     runs the agent on a task — how it prompts its model, calls its tools and decides it is done. \
     You work in a copy of its source in your workspace, with file and code tools; commands run \
     in a sandbox with no network. You are shown how the agent did on its tasks and the tasks it \
     failed. Make one small change to the harness that would have prevented such failures in \
     general, not for particular tasks. Keep to the paths you may change, and never change what \
     scores or guards the agent. The build and the tests must pass. When you are done, say in a \
     line what you changed and why.";

fn dir() -> PathBuf {
    crate::model_routes::dir()
}

fn io(path: &Path) -> impl Fn(std::io::Error) -> CliError + '_ {
    move |e| CliError::Usage(format!("{}: {e}", path.display()))
}

/// Where releases are kept: each as `<id>.json` and `<id>.patch`.
pub fn releases_dir() -> PathBuf {
    dir().join("releases")
}

/// A change to the agent's own code that sealed tasks confirmed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Release {
    /// Its id: hex digits of the digest of what it records.
    pub id: String,
    /// The release it builds on, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// The agent.
    pub agent: ObjectId,
    /// The commit of the source it applies to.
    pub base: String,
    /// The digest of the patch's exact bytes.
    pub patch: String,
    /// The files it changes.
    pub files: Vec<Changed>,
    /// How the source is built, in the sandbox.
    pub build: String,
    /// How its tests are run.
    pub test: String,
    /// How a case is run with it.
    pub candidate: Vec<String>,
    /// What the agent's operator told it when it was scored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    /// The agent it was scored with: what it learned and its models.
    pub variant: Variant,
    /// What the coder said of it.
    pub why: String,
    /// The digest of the tasks it was chosen on.
    pub train: String,
    /// The digest of the sealed tasks it was confirmed on.
    pub sealed: String,
    /// The seal's claim, if the seal was published.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seal_claim: Option<String>,
    /// How the code as it stood did on the tasks it was chosen on.
    pub train_before: Outcome,
    /// How the release did.
    pub train_after: Outcome,
    /// Newcombe's 95% interval for the difference.
    pub screen: (f64, f64),
    /// How the code as it stood did on the sealed tasks, canaries apart.
    pub sealed_before: Outcome,
    /// How the release did.
    pub sealed_after: Outcome,
    /// The interval there, at the seal's level.
    pub confirm: (f64, f64),
    /// The z of that level.
    pub z: f64,
    /// Which of the seal's confirmations it was.
    pub confirmation: u32,
    /// Of how many.
    pub budget: u32,
    /// When it was kept (seconds since the epoch).
    pub at: u64,
    /// The claim that published it, if one did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim: Option<String>,
}

impl Release {
    /// What its claim holds: all it records but the claim itself.
    pub fn claimed(&self) -> Result<Vec<u8>, CliError> {
        let mut bare = self.clone();
        bare.claim = None;
        serde_json::to_vec(&bare)
            .map_err(|e| CliError::Usage(format!("the release could not be written: {e}")))
    }

    fn identify(&mut self) {
        let mut bare = self.clone();
        bare.id = String::new();
        bare.claim = None;
        let bytes = serde_json::to_vec(&bare).unwrap_or_default();
        self.id = sion_core::hash::hash_domain("sion-cli/improve/release/v1", &bytes).to_hex()
            [..16]
            .to_string();
    }

    /// Where its patch is kept.
    pub fn patch_path(&self) -> PathBuf {
        releases_dir().join(format!("{}.patch", self.id))
    }
}

/// The digest of a patch's exact bytes.
pub fn patch_digest(patch: &[u8]) -> String {
    sion_core::hash::hash_domain("sion-cli/improve/patch/v1", patch).to_hex()
}

/// How a code change is made, built and scored.
#[derive(Clone, Debug)]
pub struct CodeSettings {
    /// The source: a git checkout, whose last commit is the code as it
    /// stands.
    pub source: PathBuf,
    /// The build, run with `/bin/sh` at the source's root in the sandbox.
    pub build: String,
    /// The tests, likewise.
    pub test: String,
    /// The program that runs a case, and its arguments, at the root.
    pub candidate: Vec<String>,
    /// What a patch may touch.
    pub rules: PatchRules,
    /// The sandbox commands, builds and candidates run in (its network is
    /// always none).
    pub sandbox: Sandbox,
    /// Variables the build and the tests are given.
    pub env: Vec<(String, String)>,
    /// Rounds, at most.
    pub rounds: usize,
    /// Times each task is run.
    pub runs: u32,
    /// Tokens each run of a task may use.
    pub budget_tokens: u64,
    /// Seconds a candidate may take to answer the harness.
    pub answer_secs: u64,
    /// Where the copies are made.
    pub work: PathBuf,
}

/// Runs git in `dir` with nothing of the user's or the repository's own
/// configuration that could run a program: no hooks, no filesystem
/// monitor, no external diff or text conversion, no pager, no system or
/// global settings.
fn git(dir: &Path, args: &[&str]) -> Result<String, CliError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "core.pager=cat",
            "-c",
            "core.attributesFile=/dev/null",
            "-c",
            "diff.external=",
            "-c",
            "protocol.file.allow=always",
        ])
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_AUTHOR_NAME", "sion")
        .env("GIT_AUTHOR_EMAIL", "sion@localhost")
        .env("GIT_COMMITTER_NAME", "sion")
        .env("GIT_COMMITTER_EMAIL", "sion@localhost")
        .stdin(Stdio::null())
        .output()
        .map_err(|e| CliError::Usage(format!("git could not run: {e}")))?;
    if !output.status.success() {
        return Err(CliError::Usage(format!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Runs git in `dir` with `input` on its standard input, hardened as
/// [`git`] is.
fn git_with(dir: &Path, args: &[&str], input: &[u8]) -> Result<(), CliError> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| CliError::Usage(format!("git could not run: {e}")))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(input)
            .map_err(|e| CliError::Usage(format!("git: {e}")))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|e| CliError::Usage(format!("git: {e}")))?;
    if !output.status.success() {
        return Err(CliError::Usage(format!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

/// What git prints when run in `dir` with `input` on its standard input,
/// hardened as [`git`] is.
fn git_reading(dir: &Path, args: &[&str], input: &[u8]) -> Result<String, CliError> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| CliError::Usage(format!("git could not run: {e}")))?;
    let mut stdin = child.stdin.take();
    let input = input.to_vec();
    let writer = std::thread::spawn(move || {
        if let Some(stdin) = stdin.as_mut() {
            let _ = stdin.write_all(&input);
        }
    });
    let output = child
        .wait_with_output()
        .map_err(|e| CliError::Usage(format!("git: {e}")))?;
    let _ = writer.join();
    if !output.status.success() {
        return Err(CliError::Usage(format!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Whether `patch` keeps to `rules` as read from the patch itself, not
/// from what its publisher says it changes: the files `git apply
/// --numstat` finds in it, and the lines it adds.
pub fn check_patch_bytes(dir: &Path, patch: &[u8], rules: &PatchRules) -> Result<(), CliError> {
    let numstat = git_reading(dir, &["apply", "--numstat"], patch)?;
    guard::check_patch(&guard::numstat(&numstat), rules).map_err(CliError::Usage)?;
    if let Some(line) = guard::forbidden_line(&String::from_utf8_lossy(patch)) {
        return Err(CliError::Usage(format!(
            "the patch adds what a change may not: {}",
            improve_loop::clip(&line, 120)
        )));
    }
    Ok(())
}

/// The commit `source` stands at.
pub fn head(source: &Path) -> Result<String, CliError> {
    Ok(git(source, &["rev-parse", "HEAD"])?.trim().to_string())
}

/// A fresh clone of `source` at `commit`, in `into`.
fn checkout(source: &Path, commit: &str, into: &Path) -> Result<(), CliError> {
    if into.exists() {
        std::fs::remove_dir_all(into).map_err(io(into))?;
    }
    if let Some(parent) = into.parent() {
        std::fs::create_dir_all(parent).map_err(io(parent))?;
    }
    let source = std::fs::canonicalize(source).map_err(io(source))?;
    let parent = into.parent().unwrap_or(Path::new("."));
    let name = into
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    git(
        parent,
        &[
            "clone",
            "--quiet",
            "--no-hardlinks",
            "--no-checkout",
            &source.to_string_lossy(),
            &name,
        ],
    )?;
    git(into, &["checkout", "--quiet", commit])?;
    Ok(())
}

/// Applies `patch` in `dir` (taken back, with `reverse`).
fn apply(dir: &Path, patch: &[u8], reverse: bool) -> Result<(), CliError> {
    let mut args = vec!["apply", "--whitespace=nowarn"];
    if reverse {
        args.push("--reverse");
    }
    git_with(dir, &args, patch)
}

/// What a copy's tree holds that the harness does not take: names it skips
/// when it copies.
const SKIPPED: &[&str] = &[".git", "target", "node_modules", "__pycache__"];

/// The regular files under `root`, relative, the skipped names left out.
/// A symbolic link is an error: a copy holding one is not taken.
fn files_of(root: &Path) -> Result<BTreeSet<PathBuf>, CliError> {
    let mut found = BTreeSet::new();
    let mut stack = vec![PathBuf::new()];
    while let Some(relative) = stack.pop() {
        let here = root.join(&relative);
        for entry in std::fs::read_dir(&here).map_err(io(&here))? {
            let entry = entry.map_err(io(&here))?;
            let name = entry.file_name();
            if SKIPPED.iter().any(|s| name == *s) {
                continue;
            }
            let kind = entry.file_type().map_err(io(&here))?;
            let path = relative.join(&name);
            if kind.is_symlink() {
                return Err(CliError::Usage(format!(
                    "{}: a symbolic link, which a change may not hold",
                    path.display()
                )));
            }
            if kind.is_dir() {
                stack.push(path);
            } else if kind.is_file() {
                found.insert(path);
            }
        }
    }
    Ok(found)
}

/// Copies the files of `from` (skipped names apart) into `to`, removing
/// from `to` those `from` does not hold.
fn mirror(from: &Path, to: &Path) -> Result<(), CliError> {
    let wanted = files_of(from)?;
    for relative in &wanted {
        let target = to.join(relative);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(io(parent))?;
        }
        std::fs::copy(from.join(relative), &target).map_err(io(&target))?;
    }
    for relative in files_of(to)? {
        if !wanted.contains(&relative) {
            let path = to.join(&relative);
            std::fs::remove_file(&path).map_err(io(&path))?;
        }
    }
    Ok(())
}

/// What the copy `edited` changes of the clone `clean` made from the same
/// commit: its files mirrored into `clean`, and the diff taken there.
/// Returns the patch and the files it changes.
fn changes(edited: &Path, clean: &Path) -> Result<(String, Vec<Changed>), CliError> {
    let edited_files = files_of(edited)?;
    for relative in &edited_files {
        let name = relative
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if (name == ".gitattributes" || name == ".gitmodules") && !clean.join(relative).exists() {
            return Err(CliError::Usage(format!(
                "{}: a change may not bring git settings of its own",
                relative.display()
            )));
        }
    }
    mirror(edited, clean)?;
    diff_of(clean)
}

/// The patch from the commit `dir` stands at to its files as they are,
/// new files included, and the files it changes.
fn diff_of(dir: &Path) -> Result<(String, Vec<Changed>), CliError> {
    git(dir, &["add", "--intent-to-add", "."])?;
    let flags = [
        "diff",
        "HEAD",
        "--no-renames",
        "--no-ext-diff",
        "--no-textconv",
    ];
    let numstat = git(dir, &[&flags[..], &["--numstat"]].concat())?;
    let patch = git(dir, &[&flags[..], &["--binary"]].concat())?;
    Ok((patch, guard::numstat(&numstat)))
}

/// The sandbox `settings` runs in, with no network whatever it was given.
fn sandbox_of(settings: &CodeSettings) -> Sandbox {
    let mut sandbox = settings.sandbox.clone();
    sandbox.network = Network::None;
    sandbox
}

/// Builds and tests the source in `dir`, in the sandbox: the end of what
/// failed, if anything did.
fn build_and_test(settings: &CodeSettings, dir: &Path) -> Result<(), String> {
    let sandbox = sandbox_of(settings);
    for (what, script) in [("build", &settings.build), ("tests", &settings.test)] {
        if script.trim().is_empty() {
            continue;
        }
        let run = sandbox.run_with(
            &[Mount::workspace(dir)],
            WORKSPACE,
            script,
            &settings.env,
            None,
        )?;
        if !run.succeeded() {
            let rendered = run.render();
            let mut tail: Vec<&str> = rendered.lines().rev().take(20).collect();
            tail.reverse();
            return Err(format!("the {what} failed:\n{}", tail.join("\n")));
        }
    }
    Ok(())
}

/// Runs one case with the candidate built in `dir`: started in the
/// sandbox, the source read-only, and served by the harness.
fn run_case(
    settings: &CodeSettings,
    dir: &Path,
    case: &sion_agent::evals::Case,
    system: &str,
    models: &[Box<dyn Model>],
) -> Result<sion_agent::remote::Served, CliError> {
    let sandbox = sandbox_of(settings);
    let mounts = [Mount {
        host: dir.to_path_buf(),
        inside: WORKSPACE.into(),
        writable: false,
    }];
    let env = vec![
        ("HOME".to_string(), "/tmp".to_string()),
        ("SION_HOME".to_string(), "/tmp/sion".to_string()),
    ];
    let (mut child, name) = sandbox
        .spawn(&mounts, WORKSPACE, &settings.candidate, &env)
        .map_err(CliError::Usage)?;
    let mut stdin = child.stdin.take();
    let stdout = child.stdout.take();
    let (lines, received) = mpsc::channel::<String>();
    let reader = std::thread::spawn(move || {
        let Some(stdout) = stdout else {
            return;
        };
        let mut reader = BufReader::new(stdout);
        loop {
            let mut line = Vec::new();
            let read = (&mut reader)
                .take(MAX_LINE as u64 + 1)
                .read_until(b'\n', &mut line);
            match read {
                Ok(0) | Err(_) => return,
                Ok(_) => {
                    let text = String::from_utf8_lossy(&line).trim_end().to_string();
                    if lines.send(text).is_err() {
                        return;
                    }
                }
            }
        }
    });
    let models: Vec<&dyn Model> = models.iter().map(|m| m.as_ref()).collect();
    let limits = ServeLimits {
        budget_tokens: settings.budget_tokens,
        ..ServeLimits::default()
    };
    let wait = Duration::from_secs(settings.answer_secs.max(1));
    // A case as a whole has the time of a few answers, however slowly
    // and steadily its candidate goes.
    let deadline = std::time::Instant::now() + wait * 8;
    let mut send = |text: &str| -> Result<(), String> {
        let Some(stdin) = stdin.as_mut() else {
            return Err("its input is closed".into());
        };
        stdin
            .write_all(format!("{text}\n").as_bytes())
            .and_then(|()| stdin.flush())
            .map_err(|e| e.to_string())
    };
    let mut receive = || {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            return None;
        }
        received.recv_timeout(wait.min(left)).ok()
    };
    let served = serve(case, system, &models, &limits, &mut send, &mut receive);
    drop(stdin.take());
    sandbox.stop(&mut child, &name);
    let _ = reader.join();
    Ok(served)
}

/// How the agent does on every case of `set`, `runs` times each, with the
/// candidate built in `dir`: told what `operator` tells it and what
/// `variant` learned, on `variant`'s models.
pub fn evaluate(
    settings: &CodeSettings,
    dir: &Path,
    operator: Option<&str>,
    variant: &Variant,
    set: &TaskSet,
    runs: u32,
) -> Result<Scored, CliError> {
    let instructions = improve_loop::with_learned(operator, &variant.learned).unwrap_or_default();
    let system = sion_agent::evolve::with_instructions(set.system(), &instructions);
    let make = crate::skill_improve::models_of(&variant.models);
    let mut trials = Vec::new();
    for case in &set.cases {
        for repeat in 1..=runs.max(1) {
            let models = make().map_err(CliError::Usage)?;
            let served = run_case(settings, dir, case, &system, &models)?;
            trials.push(Trial {
                case: case.id.clone(),
                repeat,
                passed: served.passed,
                failures: served.failures,
                tokens: served.tokens,
                answer: match &served.ended {
                    sion_agent::Ended::Done { text } => text.clone(),
                    _ => String::new(),
                },
                judged: None,
            });
        }
    }
    Ok(improve_loop::scored(
        set,
        ArmResult {
            arm: Arm::Single,
            trials,
        },
    ))
}

/// What the coder is told.
fn coder_prompt(
    settings: &CodeSettings,
    set: &TaskSet,
    result: &ArmResult,
    history: &[String],
) -> String {
    let (failures, failed) = improve_loop::failures_shown(set, result, 6);
    let mut text = format!(
        "The source is in your workspace. You may change files under: {}; never: {}; at most {} \
         files and {} lines.\nIt is built with: {}\nIts tests: {}\nA case is run with: {}\n\n\
         How the agent did: {} of {} runs of the tasks passed.\n\nTasks it failed ({} of {failed} shown):\n{failures}",
        settings.rules.allowed.join(", "),
        guard::PROTECTED
            .iter()
            .copied()
            .chain(settings.rules.protected.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(", "),
        settings.rules.max_files,
        settings.rules.max_lines,
        settings.build,
        settings.test,
        settings.candidate.join(" "),
        result.passed(),
        result.total(),
        failed.min(6),
    );
    if !history.is_empty() {
        text.push_str("\nWhat was tried before, and how it went:\n");
        for line in history.iter().rev().take(12).rev() {
            text.push_str(&format!("- {line}\n"));
        }
    }
    text
}

/// Lets `coder` work in `dir`: its file and code tools there, its
/// commands in the sandbox. Returns what it said last and the tokens it
/// used.
fn code(
    settings: &CodeSettings,
    coder: &dyn Model,
    dir: &Path,
    prompt: &str,
    round: usize,
) -> (String, u64) {
    let sandbox = sandbox_of(settings);
    let mut tools = Toolbox::new();
    for tool in sion_agent::builtin::workspace(dir) {
        tools.add_boxed(tool);
    }
    for tool in sion_agent::code::editing(dir, &[]) {
        tools.add_boxed(tool);
    }
    for tool in sion_agent::code::checking(dir, &sandbox) {
        tools.add_boxed(tool);
    }
    tools.add(sion_agent::builtin::Shell {
        root: dir.to_path_buf(),
        sandbox,
    });
    let limits = Limits {
        turns: 40,
        turns_per_progress: 0,
        max_turns: 40,
        ..Limits::default()
    };
    let runtime = Runtime {
        models: vec![coder],
        tools: &tools,
        policy: Policy {
            autonomy: Autonomy::L3,
            ..Policy::default()
        },
        limits: limits.clone(),
        system: CODER_SYSTEM.into(),
        now: &|| 0,
        stopped: &|| crate::runtime::kill_switch().exists(),
        watch: None,
        steer: None,
        checkpoint: None,
        verify: None,
        code: None,
        hooks: None,
        extras: Default::default(),
    };
    let mut state = State::start(&format!("improve-code-{round}"), prompt, &limits);
    let ended = runtime.run(&mut state);
    let said = match ended {
        sion_agent::Ended::Done { text } => text,
        _ => String::new(),
    };
    (said, state.usage.input + state.usage.output)
}

/// Records a release kept, its patch beside it.
pub type KeepRelease<'a> = &'a mut dyn FnMut(&mut Release, &str) -> Result<Vec<String>, CliError>;

/// What a code improvement does with its confirmations and its releases.
pub struct CodeSinks<'a> {
    /// Spends a confirmation: which it is, of how many.
    pub spend: &'a mut dyn FnMut() -> Result<(u32, u32), CliError>,
    /// Records a release kept, its patch beside it (publishing it).
    pub kept: KeepRelease<'a>,
    /// The time, in seconds since the epoch.
    pub now: &'a dyn Fn() -> u64,
}

impl std::fmt::Debug for CodeSinks<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CodeSinks")
    }
}

/// What a code improvement came to.
#[derive(Clone, Debug, Default)]
pub struct CodeReport {
    /// What it did, a line each.
    pub lines: Vec<String>,
    /// The releases it kept.
    pub kept: Vec<Release>,
    /// The tokens it used.
    pub tokens: u64,
}

/// Improves `profile`'s agent's own code: rounds of the coder changing a
/// copy of `settings.source`, each change gated, built, tested and scored
/// in the sandbox on `problem.train`, and confirmed on `problem.sealed`.
/// A confirmed change is a release the next round builds on.
pub fn improve_code(
    profile: &Profile,
    problem: &Problem<'_>,
    coder: &dyn Model,
    settings: &CodeSettings,
    sinks: &mut CodeSinks<'_>,
) -> Result<CodeReport, CliError> {
    settings.sandbox.check().map_err(CliError::Usage)?;
    if settings.candidate.is_empty() {
        return Err(CliError::Usage(
            "give the program that runs a case (--candidate)".into(),
        ));
    }
    let mut report = CodeReport::default();
    let base = head(&settings.source)?;
    let variant = Variant::of(profile);
    let operator = profile.instructions.as_deref();
    let work = &settings.work;
    let stable = work.join("base");
    checkout(&settings.source, &base, &stable)?;
    build_and_test(settings, &stable).map_err(|why| {
        CliError::Usage(format!(
            "the source as it stands at {} does not pass in the sandbox: {why}",
            &base[..12.min(base.len())]
        ))
    })?;
    let first = evaluate(
        settings,
        &stable,
        operator,
        &variant,
        problem.train,
        settings.runs,
    )?;
    report.tokens += first.outcome.tokens;
    report.lines.push(format!(
        "the code as it stands ({}): {}/{} runs of the tasks to choose on passed",
        &base[..12.min(base.len())],
        first.outcome.passed,
        first.outcome.total
    ));
    let mut incumbent_dir = stable.clone();
    let mut incumbent_patch = String::new();
    let mut incumbent_train = first.clone();
    let mut incumbent_sealed: Option<Scored> = None;
    let mut parent: Option<String> = None;
    let mut history: Vec<String> = Vec::new();
    for round in 0..settings.rounds {
        if crate::runtime::kill_switch().exists() {
            report.lines.push("stopped: the kill switch is on".into());
            break;
        }
        // The coder's copy holds no `.git`; the clean clone is the
        // harness's own.
        let at = work.join(format!("round-{}", round + 1));
        let clean = at.join("clean");
        let edit = at.join("edit");
        checkout(&settings.source, &base, &clean)?;
        if !incumbent_patch.is_empty() {
            apply(&clean, incumbent_patch.as_bytes(), false)?;
        }
        if edit.exists() {
            std::fs::remove_dir_all(&edit).map_err(io(&edit))?;
        }
        std::fs::create_dir_all(&edit).map_err(io(&edit))?;
        mirror(&clean, &edit)?;
        let prompt = coder_prompt(settings, problem.train, &incumbent_train.result, &history);
        let (said, used) = code(settings, coder, &edit, &prompt, round);
        report.tokens += used;
        let said = improve_loop::clip(&said, 300);
        let (patch, changed) = match changes(&edit, &clean) {
            Ok(found) => found,
            Err(why) => {
                report
                    .lines
                    .push(format!("round {}: turned down: {why}", round + 1));
                history.push(format!("round {}: turned down: {why}", round + 1));
                continue;
            }
        };
        let summary = format!(
            "round {}: {} ({})",
            round + 1,
            if said.is_empty() { "a change" } else { &said },
            changed
                .iter()
                .map(|c| format!("{} +{} -{}", c.path, c.added, c.removed))
                .collect::<Vec<_>>()
                .join(", ")
        );
        let refuse = |report: &mut CodeReport, history: &mut Vec<String>, why: &str| {
            report.lines.push(format!("{summary}: turned down: {why}"));
            history.push(format!("{summary}: turned down: {why}"));
        };
        if patch.trim().is_empty() {
            refuse(&mut report, &mut history, "it changed nothing");
            continue;
        }
        // The cumulative patch is checked whole: what this round adds to
        // what was kept.
        if let Err(why) = guard::check_patch(&changed, &settings.rules) {
            refuse(&mut report, &mut history, &why);
            continue;
        }
        if let Some(line) = guard::forbidden_line(&patch) {
            refuse(
                &mut report,
                &mut history,
                &format!(
                    "it adds what a change may not: {}",
                    improve_loop::clip(&line, 120)
                ),
            );
            continue;
        }
        if let Err(why) = build_and_test(settings, &clean) {
            refuse(&mut report, &mut history, &improve_loop::clip(&why, 1_200));
            continue;
        }
        let trained = evaluate(
            settings,
            &clean,
            operator,
            &variant,
            problem.train,
            settings.runs,
        )?;
        report.tokens += trained.outcome.tokens;
        let screen = newcombe(
            (trained.outcome.passed, trained.outcome.total),
            (
                incumbent_train.outcome.passed,
                incumbent_train.outcome.total,
            ),
        );
        if screen.0 <= 0.0 {
            refuse(
                &mut report,
                &mut history,
                &format!(
                    "{}/{} against {}/{} on the tasks to choose on: not better beyond chance",
                    trained.outcome.passed,
                    trained.outcome.total,
                    incumbent_train.outcome.passed,
                    incumbent_train.outcome.total
                ),
            );
            continue;
        }
        let before = match &incumbent_sealed {
            Some(scored) => scored.clone(),
            None => {
                let scored = evaluate(
                    settings,
                    &incumbent_dir,
                    operator,
                    &variant,
                    problem.sealed,
                    settings.runs,
                )?;
                report.tokens += scored.outcome.tokens;
                if let Some(canary) = scored.canaries.first() {
                    report.lines.push(format!(
                        "stopped: the code as it stands passed canary {canary}: the sealed tasks' answers have reached it; seal fresh tasks"
                    ));
                    break;
                }
                incumbent_sealed = Some(scored.clone());
                scored
            }
        };
        let (confirmation, budget) = match (sinks.spend)() {
            Ok(spent) => spent,
            Err(why) => {
                report.lines.push(format!("stopped: {why}"));
                break;
            }
        };
        let after = evaluate(
            settings,
            &clean,
            operator,
            &variant,
            problem.sealed,
            settings.runs,
        )?;
        report.tokens += after.outcome.tokens;
        if let Some(canary) = after.canaries.first() {
            report.lines.push(format!(
                "stopped: a change passed canary {canary}: the sealed tasks' answers have reached it, or its scoring is gamed; seal fresh tasks"
            ));
            break;
        }
        let confirm = newcombe_with(
            (after.outcome.passed, after.outcome.total),
            (before.outcome.passed, before.outcome.total),
            problem.z,
        );
        let regressed = before
            .cases
            .iter()
            .zip(&after.cases)
            .position(|(b, a)| *b && !a);
        if confirm.0 <= 0.0 || regressed.is_some() {
            let why = match regressed {
                Some(at) => format!("it fails sealed task {} that the code as it stands passes", at + 1),
                None => format!(
                    "{}/{} against {}/{} on the sealed tasks: not better beyond chance at the seal's level",
                    after.outcome.passed, after.outcome.total, before.outcome.passed, before.outcome.total
                ),
            };
            report.lines.push(format!(
                "{summary}: better on the tasks to choose on ({}/{} against {}/{}), but not confirmed: {why} (confirmation {confirmation} of {budget})",
                trained.outcome.passed, trained.outcome.total, incumbent_train.outcome.passed, incumbent_train.outcome.total
            ));
            history.push(format!("{summary}: better on the tasks shown, but not confirmed on the tasks you are not shown"));
            continue;
        }
        let mut release = Release {
            id: String::new(),
            parent: parent.clone(),
            agent: profile.agent,
            base: base.clone(),
            patch: patch_digest(patch.as_bytes()),
            files: changed.clone(),
            build: settings.build.clone(),
            test: settings.test.clone(),
            candidate: settings.candidate.clone(),
            instructions: profile.instructions.clone(),
            variant: variant.clone(),
            why: said.clone(),
            train: problem.train_digest.to_hex(),
            sealed: problem.sealed_digest.to_hex(),
            seal_claim: None,
            train_before: incumbent_train.outcome,
            train_after: trained.outcome,
            screen,
            sealed_before: before.outcome,
            sealed_after: after.outcome,
            confirm,
            z: problem.z,
            confirmation,
            budget,
            at: (sinks.now)(),
            claim: None,
        };
        release.identify();
        let lines = (sinks.kept)(&mut release, &patch)?;
        report.lines.push(format!(
            "{summary}: kept as release {}: {}/{} against {}/{} on the tasks to choose on, and {}/{} against {}/{} on the sealed tasks (confirmation {confirmation} of {budget})",
            release.id,
            trained.outcome.passed, trained.outcome.total, incumbent_train.outcome.passed, incumbent_train.outcome.total,
            after.outcome.passed, after.outcome.total, before.outcome.passed, before.outcome.total
        ));
        report
            .lines
            .extend(lines.into_iter().map(|l| format!("  {l}")));
        history.push(format!("{summary}: kept"));
        parent = Some(release.id.clone());
        incumbent_dir = clean;
        incumbent_patch = patch;
        incumbent_train = trained;
        incumbent_sealed = Some(after);
        report.kept.push(release);
    }
    if report.kept.is_empty() {
        report.lines.push(
            "no change to the code was confirmed better: the code is left as it stands".into(),
        );
    }
    Ok(report)
}

/// Keeps `release` and its `patch` beside `profile`'s lineage.
pub fn record(profile: &str, release: &Release, patch: &str) -> Result<(), CliError> {
    let dir = releases_dir();
    std::fs::create_dir_all(&dir).map_err(io(&dir))?;
    let path = release.patch_path();
    std::fs::write(&path, patch).map_err(io(&path))?;
    crate::profiles::write_json(&dir.join(format!("{}.json", release.id)), release)?;
    improve_loop::append(profile, &improve_loop::Record::Released(release.clone()))
}

/// The index key an agent's latest release is named under.
pub fn release_key(agent: &ObjectId) -> String {
    format!("agent-release:{}", hex::encode(&agent.as_hash().0[..16]))
}

/// Publishes `release` as `wallet`'s claim: the transaction that issues it
/// is the release's signature.
pub fn publish(
    session: &mut Session<'_>,
    wallet: &str,
    release: &Release,
) -> Result<ClaimId, CliError> {
    let body = sion_exec::claim::ClaimBody {
        schema: RELEASE_SCHEMA.into(),
        subject: format!(
            "agent {} releases a change to its own code: patch {} on {}",
            release.agent,
            crate::sealed::short(&release.patch, 16),
            &release.base[..12.min(release.base.len())]
        ),
        payload: Payload::Public(release.claimed()?),
        formula: sion_assurance::Formula::and(vec![sion_exec::claim::formulas::dual_signature()]),
        adjudicator: None,
        detection_permille: 0,
        valid_for: crate::sealed::VALID_FOR,
        challenge_period: 0,
    };
    crate::vouching::publish(session, wallet, body, Some(release_key(&release.agent)))
}

/// What an attestation says.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Attestation {
    /// The release's claim.
    pub release: String,
    /// Its patch's digest.
    pub patch: String,
    /// The commit it applies to.
    pub base: String,
    /// The digest of the sealed tasks it was scored on.
    pub sealed: String,
    /// How the code as it stood did there, rebuilt by the attester.
    pub before: Outcome,
    /// How the release did.
    pub after: Outcome,
    /// The interval for the difference, at the release's level.
    pub interval: (f64, f64),
    /// The z of that level.
    pub z: f64,
    /// Whether a sealed task the code as it stood passes, the release
    /// failed.
    pub regressed: bool,
    /// When (seconds since the epoch).
    pub at: u64,
}

/// The index key attestations of a release are named under.
pub fn attested_key(release: &str) -> String {
    format!(
        "agent-release-attested:{}",
        &release[..32.min(release.len())]
    )
}

/// A release, as its claim holds it, and the claim's issuer.
pub fn release_of(
    ledger: &dyn crate::ledger::Ledger,
    claim: &str,
) -> Result<(Release, sion_core::ids::AccountId), CliError> {
    let stored = crate::tasks::stored_claim(ledger, claim)?;
    if stored.body.schema != RELEASE_SCHEMA {
        return Err(CliError::Usage(format!(
            "claim {} is a {}, not a release",
            stored.id.short(),
            stored.body.schema
        )));
    }
    let Payload::Public(bytes) = &stored.body.payload else {
        return Err(CliError::Usage("a release's payload is public".into()));
    };
    let release: Release = serde_json::from_slice(bytes)
        .map_err(|e| CliError::Protocol(format!("release {claim}: {e}")))?;
    Ok((release, stored.issuer))
}

/// Rebuilds and scores the release `claim` as its attester: the source at
/// its commit and with its `patch`, each built and tested in the sandbox,
/// both scored on the `sealed` tasks (their exact bytes) the release was
/// confirmed on. Publishes, as `wallet`'s claim, what was found if the
/// release holds up — better beyond chance at its level, no sealed task
/// lost — on the attester's bond [`ATTESTER_BOND`] when `adjudicator` is
/// given, who can rule against it. Returns the attestation's claim, if one
/// was published, and what was found.
#[allow(clippy::too_many_arguments)]
pub fn attest(
    session: &mut Session<'_>,
    ledger: &dyn crate::ledger::Ledger,
    wallet: &str,
    claim: &str,
    patch: &[u8],
    sealed: &[u8],
    adjudicator: Option<sion_core::ids::AtomId>,
    settings: &CodeSettings,
    now: u64,
) -> Result<(Option<ClaimId>, Vec<String>), CliError> {
    let (release, issuer) = release_of(ledger, claim)?;
    if session.id(wallet) == issuer {
        return Err(CliError::Usage(
            "a release's publisher cannot attest its own release".into(),
        ));
    }
    if patch_digest(patch) != release.patch {
        return Err(CliError::Usage(
            "the patch given is not the release's".into(),
        ));
    }
    if sion_agent::evals::digest(sealed).to_hex() != release.sealed {
        return Err(CliError::Usage(
            "the tasks given are not the sealed tasks the release was confirmed on".into(),
        ));
    }
    let set = TaskSet::parse(sealed).map_err(CliError::Usage)?;
    let mut settings = settings.clone();
    settings.build = release.build.clone();
    settings.test = release.test.clone();
    settings.candidate = release.candidate.clone();
    let stable = settings.work.join("attest-base");
    let changed = settings.work.join("attest-release");
    checkout(&settings.source, &release.base, &stable)?;
    checkout(&settings.source, &release.base, &changed)?;
    check_patch_bytes(&changed, patch, &settings.rules)?;
    apply(&changed, patch, false)?;
    let (_, files) = diff_of(&changed)?;
    guard::check_patch(&files, &settings.rules).map_err(CliError::Usage)?;
    for (what, dir) in [("the code as it stood", &stable), ("the release", &changed)] {
        build_and_test(&settings, dir).map_err(|why| CliError::Usage(format!("{what}: {why}")))?;
    }
    let operator = release.instructions.as_deref();
    let before = evaluate(
        &settings,
        &stable,
        operator,
        &release.variant,
        &set,
        settings.runs,
    )?;
    let after = evaluate(
        &settings,
        &changed,
        operator,
        &release.variant,
        &set,
        settings.runs,
    )?;
    let interval = newcombe_with(
        (after.outcome.passed, after.outcome.total),
        (before.outcome.passed, before.outcome.total),
        release.z,
    );
    let regressed = before.cases.iter().zip(&after.cases).any(|(b, a)| *b && !a);
    let mut lines = vec![format!(
        "rebuilt and scored: the release {}/{} against {}/{} for the code as it stood (interval at the release's level {:+.0}% to {:+.0}%){}",
        after.outcome.passed,
        after.outcome.total,
        before.outcome.passed,
        before.outcome.total,
        interval.0 * 100.0,
        interval.1 * 100.0,
        if regressed { "; it fails a sealed task the code as it stood passes" } else { "" }
    )];
    if !after.canaries.is_empty() || !before.canaries.is_empty() {
        lines.push("a canary passed: not attested".into());
        return Ok((None, lines));
    }
    if interval.0 <= 0.0 || regressed {
        lines.push("not attested: it does not hold up".into());
        return Ok((None, lines));
    }
    let found = Attestation {
        release: claim.to_string(),
        patch: release.patch.clone(),
        base: release.base.clone(),
        sealed: release.sealed.clone(),
        before: before.outcome,
        after: after.outcome,
        interval,
        z: release.z,
        regressed,
        at: now,
    };
    let body = sion_exec::claim::ClaimBody {
        schema: ATTESTATION_SCHEMA.into(),
        subject: format!(
            "release {} rebuilt and scored as its publisher said",
            &claim[..16.min(claim.len())]
        ),
        payload: Payload::Public(
            serde_json::to_vec(&found).map_err(|e| CliError::Usage(e.to_string()))?,
        ),
        formula: match adjudicator {
            Some(_) => bonded_attestation(&session.id(wallet)),
            None => {
                sion_assurance::Formula::and(vec![sion_exec::claim::formulas::dual_signature()])
            }
        },
        adjudicator,
        detection_permille: if adjudicator.is_some() { 1_000 } else { 0 },
        valid_for: crate::sealed::VALID_FOR,
        challenge_period: if adjudicator.is_some() { 100 } else { 0 },
    };
    let id = crate::vouching::publish(session, wallet, body, Some(attested_key(claim)))?;
    lines.push(match adjudicator {
        Some(_) => format!(
            "attested, as claim {}, on your bond {ATTESTER_BOND}",
            id.as_hash().to_hex()
        ),
        None => format!("attested, as claim {}", id.as_hash().to_hex()),
    });
    Ok((Some(id), lines))
}

/// Applies the release `claim` to the checkout `source` (or, with `undo`,
/// takes it back out): only if its patch is the one given and keeps to
/// `rules` as read from the patch itself, the checkout stands at its
/// commit, and at least `need` of the `attestations` hold —
/// each by one of `verifiers`, none by the release's publisher, each about
/// this release and its patch and finding it better, each still standing
/// (not expired, nor shown false), and, when `bonded` holds the adjudicators
/// the installer accepts and the least bond, each on its attester's bond
/// [`ATTESTER_BOND`], unbroken and holding at least that much, under one of
/// those adjudicators. Returns a line for what was done.
#[allow(clippy::too_many_arguments)]
pub fn install(
    ledger: &dyn crate::ledger::Ledger,
    profile: &str,
    claim: &str,
    source: &Path,
    patch: &[u8],
    attestations: &[String],
    verifiers: &[sion_core::ids::AccountId],
    need: usize,
    bonded: Option<(&[sion_core::ids::AtomId], sion_core::Amount)>,
    rules: &PatchRules,
    undo: bool,
    now: u64,
) -> Result<Vec<String>, CliError> {
    if let Some((adjudicators, _)) = bonded {
        if adjudicators.is_empty() {
            return Err(CliError::Usage(
                "bonded attestations count only under adjudicators you name (--adjudicator)".into(),
            ));
        }
    }
    let (release, issuer) = release_of(ledger, claim)?;
    if patch_digest(patch) != release.patch {
        return Err(CliError::Usage(
            "the patch given is not the release's".into(),
        ));
    }
    // Held to the installer's own rules, whatever its publisher and its
    // attesters allowed: this patch is about to reach a tree built
    // outside any sandbox.
    check_patch_bytes(source, patch, rules)?;
    if undo {
        apply(source, patch, true)?;
        improve_loop::append(
            profile,
            &improve_loop::Record::Uninstalled {
                release: claim.to_string(),
                source: source.to_path_buf(),
                at: now,
            },
        )?;
        return Ok(vec![format!(
            "took release {} back out of {}",
            &claim[..16.min(claim.len())],
            source.display()
        )]);
    }
    if head(source)? != release.base {
        return Err(CliError::Usage(format!(
            "{} is not at the commit the release applies to ({})",
            source.display(),
            release.base
        )));
    }
    let mut by: BTreeSet<sion_core::ids::AccountId> = BTreeSet::new();
    let mut lines = Vec::new();
    for id in attestations {
        let stored = crate::tasks::stored_claim(ledger, id)?;
        let short = &id[..16.min(id.len())];
        if stored.body.schema != ATTESTATION_SCHEMA {
            lines.push(format!("claim {short} is not an attestation: not counted"));
            continue;
        }
        let Payload::Public(bytes) = &stored.body.payload else {
            continue;
        };
        let Ok(found) = serde_json::from_slice::<Attestation>(bytes) else {
            lines.push(format!("attestation {short} cannot be read: not counted"));
            continue;
        };
        if stored.issuer == issuer {
            lines.push(format!(
                "attestation {short} is by the release's publisher: not counted"
            ));
            continue;
        }
        if !verifiers.contains(&stored.issuer) {
            lines.push(format!(
                "attestation {short} is by an account not named a verifier: not counted"
            ));
            continue;
        }
        if found.release != claim || found.patch != release.patch || found.base != release.base {
            lines.push(format!(
                "attestation {short} is about another release: not counted"
            ));
            continue;
        }
        if found.interval.0 <= 0.0 || found.regressed {
            lines.push(format!(
                "attestation {short} did not find it better: not counted"
            ));
            continue;
        }
        let on_bond = stored.body.adjudicator.is_some()
            && stored.body.formula == bonded_attestation(&stored.issuer);
        // An attestation that expired, or that its adjudicator ruled
        // against, attests nothing.
        let standing = crate::ledger::LedgerExt::claim(ledger, stored.id)?;
        if standing["status"].as_str() != Some("Active") {
            lines.push(format!(
                "attestation {short} no longer stands ({}): not counted",
                standing["status"].as_str().unwrap_or("unknown")
            ));
            continue;
        }
        if let Some((adjudicators, least)) = bonded {
            if !on_bond {
                lines.push(format!(
                    "attestation {short} rests on its attester's signature, not on a bond: not counted"
                ));
                continue;
            }
            if !stored
                .body
                .adjudicator
                .is_some_and(|a| adjudicators.contains(&a))
            {
                lines.push(format!(
                    "attestation {short} is under an adjudicator you did not name: not counted"
                ));
                continue;
            }
            // The bond as the ledger has it now: one broken by an upheld
            // challenge, or emptied, stands behind nothing.
            let label = format!("bond:{}:{ATTESTER_BOND}", stored.issuer);
            let bond = standing["atoms"].as_array().and_then(|atoms| {
                atoms
                    .iter()
                    .find(|a| a["atom"]["label"].as_str() == Some(label.as_str()))
            });
            let amount = |v: &serde_json::Value| -> u128 {
                v.as_u64()
                    .map(u128::from)
                    .or_else(|| v.as_str().and_then(|t| t.parse().ok()))
                    .unwrap_or(0)
            };
            let held = bond.map_or(0, |b| {
                amount(&b["bond"]["amount"]).saturating_sub(amount(&b["bond"]["slashed"]))
            });
            if bond.map_or(true, |b| b["broken"].as_bool() != Some(false)) {
                lines.push(format!(
                    "attestation {short}'s bond is broken, or the ledger names none: not counted"
                ));
                continue;
            }
            if held < least.0 {
                lines.push(format!(
                    "attestation {short}'s bond holds {} USD, less than the {} USD asked: not counted",
                    sion_core::Amount(held),
                    least
                ));
                continue;
            }
        }
        if by.insert(stored.issuer) {
            lines.push(format!(
                "attestation {short}: rebuilt and scored {}/{} against {}/{}{}",
                found.after.passed,
                found.after.total,
                found.before.passed,
                found.before.total,
                if on_bond {
                    format!(", on its attester's bond {ATTESTER_BOND}")
                } else {
                    String::new()
                }
            ));
        }
    }
    if by.len() < need.max(1) {
        return Err(CliError::Usage(format!(
            "{} attestations by verifiers named hold, of {} needed: not installed{}",
            by.len(),
            need.max(1),
            if lines.is_empty() {
                String::new()
            } else {
                format!("\n{}", lines.join("\n"))
            }
        )));
    }
    apply(source, patch, false)?;
    improve_loop::append(
        profile,
        &improve_loop::Record::Installed {
            release: claim.to_string(),
            source: source.to_path_buf(),
            at: now,
        },
    )?;
    lines.push(format!(
        "applied release {} to {}: build and install it as you build sion",
        &claim[..16.min(claim.len())],
        source.display()
    ));
    Ok(lines)
}

/// The digest `text` names, read as hex.
pub fn digest_of(text: &str) -> Result<Hash32, CliError> {
    Hash32::from_hex(text).map_err(|e| CliError::Usage(format!("{text}: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_copy_with_a_link_of_its_own_is_not_taken() {
        let root = std::env::temp_dir().join(format!("sion-code-files-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join("target")).unwrap();
        std::fs::write(root.join("src/a.rs"), "x").unwrap();
        std::fs::write(root.join(".git/config"), "x").unwrap();
        std::fs::write(root.join("target/out"), "x").unwrap();
        let files = files_of(&root).unwrap();
        assert_eq!(
            files.into_iter().collect::<Vec<_>>(),
            vec![PathBuf::from("src/a.rs")]
        );
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/etc/passwd", root.join("src/link")).unwrap();
            assert!(files_of(&root)
                .unwrap_err()
                .to_string()
                .contains("symbolic link"));
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_release_is_named_by_what_it_records() {
        let variant = Variant {
            learned: String::new(),
            models: vec!["m".into()],
        };
        let mut release = Release {
            id: String::new(),
            parent: None,
            agent: ObjectId::from_hash(Hash32::ZERO),
            base: "b".repeat(40),
            patch: "p".repeat(64),
            files: Vec::new(),
            build: "true".into(),
            test: "true".into(),
            candidate: vec!["x".into()],
            instructions: None,
            variant,
            why: String::new(),
            train: "t".into(),
            sealed: "s".into(),
            seal_claim: None,
            train_before: Outcome::default(),
            train_after: Outcome::default(),
            screen: (0.1, 0.5),
            sealed_before: Outcome::default(),
            sealed_after: Outcome::default(),
            confirm: (0.1, 0.5),
            z: 2.5,
            confirmation: 1,
            budget: 4,
            at: 7,
            claim: None,
        };
        release.identify();
        let first = release.id.clone();
        release.claim = Some("c".into());
        release.identify();
        assert_eq!(release.id, first, "the claim is not part of what names it");
        assert!(!release.claimed().unwrap().is_empty());
        release.at = 8;
        release.identify();
        assert_ne!(release.id, first);
    }
}
