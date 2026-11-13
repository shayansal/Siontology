//! What an agent's own work gives for training an open model on it: its
//! good sessions as examples to fine-tune on, and its tasks as
//! environments to reinforce in ([`sion_agent::episode`]).
//!
//! `sion agent improve export` writes, into a directory:
//!
//! - `sft.jsonl` and `sft-validation.jsonl`: the profile's sessions that
//!   went well (marked good, or their check passed and no mark says
//!   otherwise), each a line in OpenAI's chat format with tool calls, the
//!   tools the model was shown beside them (`{"messages": [...], "tools":
//!   [...], "metadata": {...}}`), as fine-tuning tools read them; a share
//!   of them, drawn by each session's id, held apart for validation;
//! - `tasks.json` and `environments.jsonl`: the profile's tasks (those of
//!   its seal that changes are chosen on, or all it has harvested), each
//!   with its prompt, its tools and its case, to be run one at a time as an
//!   episode with the policy outside (`sion agent improve env`);
//! - `README.md`: what each file is.
//!
//! A session is passed over when it did not finish, its conversation was
//! compacted, it held confidential content, it holds a secret this machine
//! keeps (a credential or a login's password), or its task is among those
//! sealed. What is exported is recorded, so that a later seal never seals
//! a task trained on: a model that has seen a task would pass it by
//! memory, and its confirmation would mean nothing.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sion_agent::compare::TaskSet;
use zeroize::Zeroizing;

use crate::error::CliError;

/// The share of examples held apart for validation by default, in percent.
pub const VALIDATION: u32 = 10;

fn io(path: &Path) -> impl Fn(std::io::Error) -> CliError + '_ {
    move |e| CliError::Usage(format!("{}: {e}", path.display()))
}

/// The digest a task is known by when it is exported: of its words.
pub fn task_digest(task: &str) -> String {
    sion_core::hash::hash_parts("sion-cli/training/task/v1", &[task.as_bytes()]).to_hex()
}

/// Where what was exported is recorded.
pub fn exported_path() -> PathBuf {
    crate::model_routes::dir().join("exported.json")
}

/// The tasks exported for training, by profile.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Exported {
    by_profile: BTreeMap<String, BTreeSet<String>>,
}

fn read_exported() -> Result<Exported, CliError> {
    let path = exported_path();
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text)
            .map_err(|e| CliError::Usage(format!("{}: {e}", path.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Exported::default()),
        Err(e) => Err(io(&path)(e)),
    }
}

/// The digests of the tasks of `profile` exported for training: none of
/// them is ever sealed ([`crate::sealed::seal`]).
pub fn trained(profile: &str) -> Result<BTreeSet<String>, CliError> {
    Ok(read_exported()?
        .by_profile
        .remove(profile)
        .unwrap_or_default())
}

fn record(profile: &str, digests: BTreeSet<String>) -> Result<(), CliError> {
    let path = exported_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(io(dir))?;
    }
    let _held = crate::pairing::Held::take(&path.with_extension("lock"))?;
    let mut all = read_exported()?;
    all.by_profile
        .entry(profile.to_string())
        .or_default()
        .extend(digests);
    crate::profiles::write_json(&path, &all)
}

/// The secrets this machine keeps, at least 8 characters long: the
/// credentials kept and set, and the logins' passwords.
pub(crate) fn kept_secrets() -> Vec<Zeroizing<String>> {
    let mut secrets: Vec<Zeroizing<String>> = Vec::new();
    for name in crate::credentials::NAMES {
        if let Some(value) = crate::credentials::var(name) {
            if !name.ends_with("_URL") && !name.ends_with("_BASE") && !name.ends_with("_ENDPOINT") {
                secrets.push(Zeroizing::new(value));
            }
        }
    }
    for name in [
        "AWS_SECRET_ACCESS_KEY",
        "AWS_SESSION_TOKEN",
        "GOOGLE_OAUTH_ACCESS_TOKEN",
    ] {
        if let Ok(value) = std::env::var(name) {
            secrets.push(Zeroizing::new(value));
        }
    }
    if let Ok(store) = crate::logins::Logins::here().load() {
        for entry in store.logins.values() {
            secrets.push(Zeroizing::new(entry.password.clone()));
        }
    }
    secrets.retain(|s| s.trim().chars().count() >= 8);
    secrets
}

/// Whether `text` holds any of `secrets`.
pub(crate) fn holds_secret(text: &str, secrets: &[Zeroizing<String>]) -> bool {
    secrets.iter().any(|secret| text.contains(secret.trim()))
}

/// Whether `id` falls in the validation share `percent`: drawn from the
/// id alone, so that a session stays on its side from export to export.
pub fn in_validation(id: &str, percent: u32) -> bool {
    let digest = sion_core::hash::hash_parts("sion-cli/training/split/v1", &[id.as_bytes()]);
    let draw = u32::from(u16::from_be_bytes([digest.0[0], digest.0[1]]));
    draw * 100 < percent * 65_536
}

/// The tools of a session as its model saw them; for a session kept before
/// they were, the tools it called, by name.
pub(crate) fn tools_of(saved: &crate::runtime::Saved) -> Vec<sion_agent::ToolSpec> {
    if !saved.tools.is_empty() {
        return saved.tools.clone();
    }
    let names: BTreeSet<String> = sion_agent::evals::calls_of(&saved.state)
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    names
        .into_iter()
        .map(|name| sion_agent::ToolSpec {
            name,
            description: String::new(),
            input_schema: json!({"type": "object"}),
        })
        .collect()
}

/// What an export wrote.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Report {
    /// Examples to fine-tune on.
    pub examples: usize,
    /// Of them, held apart for validation.
    pub validation: usize,
    /// Tasks to reinforce in.
    pub environments: usize,
    /// Whether the tasks are those of the profile's seal to choose changes
    /// on (else all it has harvested).
    pub from_seal: bool,
    /// The sessions passed over, and why.
    pub passed_over: Vec<String>,
}

impl Report {
    /// The lines that say what was written.
    pub fn lines(&self, out: &Path) -> Vec<String> {
        let mut lines = vec![
            format!(
                "{} examples to fine-tune on in {} ({} of them held apart in {})",
                self.examples,
                out.join("sft.jsonl").display(),
                self.validation,
                out.join("sft-validation.jsonl").display()
            ),
            format!(
                "{} tasks to reinforce in, in {} ({}): run one with sion agent improve env --tasks {} --case <id>",
                self.environments,
                out.join("environments.jsonl").display(),
                if self.from_seal {
                    "the seal's tasks to choose changes on; the sealed ones stay sealed"
                } else {
                    "all the profile's harvested tasks: none of them will be sealed"
                },
                out.join("tasks.json").display()
            ),
        ];
        lines.extend(
            self.passed_over
                .iter()
                .map(|why| format!("passed over {why}")),
        );
        lines
    }
}

/// One environment line: a task's prompt, tools and case.
pub fn environment_line(case: &sion_agent::evals::Case, system: &str, tasks: &str) -> Value {
    let tools: Vec<sion_agent::ToolSpec> = case
        .tools
        .iter()
        .map(|fixture| {
            sion_agent::tools::Tool::spec(&sion_agent::evals::FixtureTool(fixture.clone()))
        })
        .collect();
    json!({
        "id": case.id,
        "prompt": [
            {"role": "system", "content": system},
            {"role": "user", "content": case.task},
        ],
        "tools": sion_agent::providers::openai_tools(&tools),
        "checks": case.checks,
        "reward": "the share of the checks that hold",
        "run": format!("sion agent improve env --tasks {tasks} --case {}", case.id),
    })
}

const README: &str = "# What an agent's work gives for training

Written by `sion agent improve export`.

- `sft.jsonl`, `sft-validation.jsonl`: sessions that went well, one a line
  in OpenAI's chat format with tool calls (`messages`), the tools the
  model was shown (`tools`) and where each came from (`metadata`). The
  validation share is drawn from each session's id.
- `tasks.json`: the tasks, as `sion agent improve harvest` writes them:
  each a task, stand-ins that answer its tools, and checks.
- `environments.jsonl`: each task's prompt, tools and checks, and the
  command that runs it as an episode.

An episode (`sion agent improve env --tasks tasks.json --case <id>`) runs
the task in sion's runtime with the policy outside. It writes, a line
each, `{\"request\": {\"messages\": [...], \"tools\": [...], \"max_tokens\":
n}}` and reads the policy's turn, `{\"message\": {\"role\": \"assistant\",
\"content\": ..., \"tool_calls\": [...]}}` (or `{\"error\": ...}`); when the
run ends it writes `{\"done\": {\"reward\": r, \"passed\": ..., \"failures\":
[...]}}`, the reward the share of the task's checks that hold.

Nothing sealed is here, and none of these tasks will be sealed later: a
model that has seen a task would pass it by memory.
";

/// Exports `profile`'s work for training into `out`, `validation` percent
/// of the examples held apart.
pub fn export(
    profile: &crate::profiles::Profile,
    out: &Path,
    validation: u32,
) -> Result<Report, CliError> {
    if validation > 50 {
        return Err(CliError::Usage(
            "hold apart at most 50 percent for validation".into(),
        ));
    }
    std::fs::create_dir_all(out).map_err(io(out))?;
    let seal = crate::sealed::current(&profile.name).ok();
    let sealed: BTreeSet<String> = match &seal {
        Some(seal) => seal
            .sealed_set()?
            .0
            .cases
            .iter()
            .map(|case| task_digest(&case.task))
            .collect(),
        None => BTreeSet::new(),
    };
    let secrets = kept_secrets();
    let marks = crate::sealed::marks()?;
    let dir = crate::runtime::sessions_dir();
    let mut ids: Vec<String> = match std::fs::read_dir(&dir) {
        Ok(entries) => entries
            .flatten()
            .filter_map(|e| {
                e.file_name()
                    .to_string_lossy()
                    .strip_suffix(".json")
                    .map(str::to_string)
            })
            .collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(io(&dir)(e)),
    };
    ids.sort();
    let mut report = Report::default();
    let mut train = String::new();
    let mut held = String::new();
    let mut trained = BTreeSet::new();
    for id in ids {
        let Ok(saved) = crate::runtime::Saved::load(&id) else {
            continue;
        };
        if saved.wallet != profile.wallet || saved.agent != profile.agent {
            continue;
        }
        let checked =
            saved.verify.is_some() && matches!(saved.ended, Some(sion_agent::Ended::Done { .. }));
        let why = match marks.get(&saved.state.id) {
            Some(mark) if mark.good => "marked good",
            Some(_) => continue,
            None if checked => "its check passed",
            None => continue,
        };
        let short = crate::sealed::short(&saved.state.id, 12).to_string();
        let pass_over = |why: &str| format!("session {short}: {why}");
        if !matches!(saved.ended, Some(sion_agent::Ended::Done { .. })) {
            report.passed_over.push(pass_over("it did not finish"));
            continue;
        }
        if saved.state.compactions > 0 {
            report
                .passed_over
                .push(pass_over("its conversation was compacted"));
            continue;
        }
        if saved.state.standing.confidential {
            report
                .passed_over
                .push(pass_over("it held confidential content"));
            continue;
        }
        let task = match sion_agent::harvest::task_of(&saved.state) {
            Ok(task) => task,
            Err(why) => {
                report.passed_over.push(pass_over(&why));
                continue;
            }
        };
        let digest = task_digest(&task);
        if sealed.contains(&digest) {
            report.passed_over.push(pass_over("its task is sealed"));
            continue;
        }
        let system = if saved.prompt.is_empty() {
            &saved.system
        } else {
            &saved.prompt
        };
        let line = json!({
            "messages": sion_agent::providers::openai_messages(
                system,
                &saved.state.messages,
                &saved.state.images,
                "",
            ),
            "tools": sion_agent::providers::openai_tools(&tools_of(&saved)),
            "metadata": {
                "session": saved.state.id,
                "why": why,
                "note": marks.get(&saved.state.id).map(|m| m.note.clone()).unwrap_or_default(),
                "turns": saved.state.turns_taken,
                "tool_calls": sion_agent::evals::calls_of(&saved.state).len(),
                "read_outside": saved.state.standing.tainted_until.is_some(),
            },
        })
        .to_string();
        if holds_secret(&line, &secrets) {
            report
                .passed_over
                .push(pass_over("it holds a secret this machine keeps"));
            continue;
        }
        trained.insert(digest);
        report.examples += 1;
        if in_validation(&saved.state.id, validation) {
            report.validation += 1;
            held.push_str(&line);
            held.push('\n');
        } else {
            train.push_str(&line);
            train.push('\n');
        }
    }
    // The tasks: those of the seal to choose changes on, or all harvested.
    let set: Option<TaskSet> = match &seal {
        Some(seal) => {
            report.from_seal = true;
            Some(seal.train_set()?.0)
        }
        None => crate::sealed::harvest(profile)?.0,
    };
    let tasks_path = out.join("tasks.json");
    let mut environments = String::new();
    if let Some(set) = &set {
        let text = serde_json::to_string_pretty(set)
            .map_err(|e| CliError::Usage(format!("the tasks could not be written: {e}")))?;
        std::fs::write(&tasks_path, format!("{text}\n")).map_err(io(&tasks_path))?;
        for case in &set.cases {
            if sion_agent::guard::is_canary(case) || sealed.contains(&task_digest(&case.task)) {
                continue;
            }
            trained.insert(task_digest(&case.task));
            environments.push_str(
                &environment_line(case, set.system(), &tasks_path.display().to_string())
                    .to_string(),
            );
            environments.push('\n');
            report.environments += 1;
        }
    }
    for (name, text) in [
        ("sft.jsonl", &train),
        ("sft-validation.jsonl", &held),
        ("environments.jsonl", &environments),
    ] {
        let path = out.join(name);
        std::fs::write(&path, text).map_err(io(&path))?;
    }
    let readme = out.join("README.md");
    std::fs::write(&readme, README).map_err(io(&readme))?;
    record(&profile.name, trained)?;
    Ok(report)
}

/// Runs case `case` of the set at `tasks` as one episode, the policy
/// answering on standard input what it is asked on standard output: the
/// reward last.
pub fn episode(tasks: &Path, case: &str) -> Result<(), CliError> {
    let bytes = std::fs::read(tasks).map_err(io(tasks))?;
    let set = TaskSet::parse(&bytes).map_err(CliError::Usage)?;
    let found = set
        .cases
        .iter()
        .find(|c| c.id == case)
        .ok_or_else(|| CliError::Usage(format!("{} has no case {case}", tasks.display())))?;
    let policy = sion_agent::episode::OutsidePolicy::new(
        "policy",
        std::io::BufReader::new(std::io::stdin()),
        std::io::stdout(),
    );
    let run = sion_agent::episode::run(
        found,
        set.system(),
        &policy,
        &sion_agent::session::Limits::default(),
    );
    policy
        .tell(&sion_agent::episode::done_line(found, &run))
        .map_err(CliError::Usage)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_stays_on_its_side_of_the_split() {
        let held: usize = (0..2_000)
            .filter(|i| in_validation(&format!("session-{i}"), 10))
            .count();
        assert!((140..=260).contains(&held), "{held}");
        assert_eq!(in_validation("a", 10), in_validation("a", 10));
        assert!(!(0..100).any(|i| in_validation(&format!("s{i}"), 0)));
        assert!((0..100).all(|i| in_validation(&format!("s{i}"), 100)));
    }

    #[test]
    fn a_line_holding_a_kept_secret_is_found() {
        let secrets = vec![Zeroizing::new("sk-live-0123456789".to_string())];
        assert!(holds_secret(
            r#"{"content":"key sk-live-0123456789"}"#,
            &secrets
        ));
        assert!(!holds_secret(r#"{"content":"no key"}"#, &secrets));
    }
}
