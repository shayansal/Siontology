//! The agent improving itself from its own work: it writes its own
//! changes, they are tried on tasks from its work, and one is kept only
//! when it is better on tasks sealed before it was written.
//!
//! A round:
//!
//! 1. **A variant to build on** is drawn from the archive of every variant
//!    tried ([`sion_agent::archive`]): by how productive its line has been
//!    (the Huxley–Gödel Machine's draw), among those that still fail a task.
//! 2. **The proposer** — a model, under the improver's instructions
//!    ([`Improver`]) — is shown what the operator tells the agent (fixed),
//!    what the agent has learned so far, the models it may run on, the
//!    tasks it failed among those changes may be chosen on, and what was
//!    tried before and how it went. It proposes changes with
//!    `propose_change`: what the agent has learned, rewritten, and the
//!    models it runs on. It is never shown a sealed task, nor anything a
//!    sealed task's run said.
//! 3. **Gates that hold whatever it says** ([`sion_agent::guard`]): a size
//!    limit, models only from those allowed, nothing already tried, and no
//!    run of words shared with a sealed task (nor a canary's code word,
//!    which stops the loop).
//! 4. **Staged trial.** Each change runs every task once; one clearly
//!    worse than the variant it was made from goes no further. Then every
//!    task is run in full, and the change joins the archive, kept or not.
//! 5. **Screening**, on the tasks changes are chosen on: better than the
//!    agent as it now is beyond chance (Newcombe's 95% interval above
//!    zero).
//! 6. **Confirmation**, on the sealed tasks: better beyond chance at the
//!    seal's corrected level ([`crate::sealed::Seal::z`]), failing no
//!    sealed task the agent passes, and passing no canary. Each spends one
//!    of the seal's confirmations.
//!
//! A confirmed change becomes the agent the next ones are weighed against,
//! is recorded with its evidence ([`Step`], and on the ledger with a
//! wallet), and with `--apply` becomes the profile's: what it learned is
//! kept apart from what its operator tells it ([`with_learned`]), so the
//! loop never rewrites the operator's words, and [`undo`] restores what a
//! change replaced.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sion_agent::archive::{temperature, Archive, Draws};
use sion_agent::compare::{newcombe, newcombe_with, Arm, ArmResult, TaskSet};
use sion_agent::model::{Model, ToolSpec};
use sion_agent::policy::{Autonomy, Policy};
use sion_agent::session::{Limits, Runtime, State};
use sion_agent::tools::{Effect, Outcome as ToolOutcome, Tool, Toolbox};
use sion_core::hash::Hash32;
use sion_core::ids::ObjectId;

use crate::error::CliError;
use crate::profiles::Profile;
use crate::self_improve::Outcome;

/// The most bytes of what an agent has learned.
pub const MAX_LEARNED: usize = 4 << 10;

/// The most models a variant runs on.
pub const MAX_MODELS: usize = 4;

/// The heading what an agent learned is given in its prompt, after what
/// its operator tells it.
pub const LEARNED: &str =
    "What you have learned from your own work (kept because it measurably helped):";

fn io(path: &std::path::Path) -> impl Fn(std::io::Error) -> CliError + '_ {
    move |e| CliError::Usage(format!("{}: {e}", path.display()))
}

fn dir() -> PathBuf {
    crate::model_routes::dir()
}

/// What an agent has learned, as it is now.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Learned {
    /// The words.
    pub text: String,
    /// When they were made the agent's (seconds since the epoch).
    pub at: u64,
    /// The kept change they came from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<String>,
}

/// Where a profile's learned words are kept.
pub fn learned_path(profile: &str) -> PathBuf {
    dir().join(format!("{profile}-learned.json"))
}

/// What `profile`'s agent has learned, if anything: a file that cannot be
/// read counts as nothing learned.
pub fn learned(profile: &str) -> Option<Learned> {
    let text = std::fs::read_to_string(learned_path(profile)).ok()?;
    serde_json::from_str(&text).ok()
}

fn set_learned(profile: &str, learned: Option<&Learned>) -> Result<(), CliError> {
    let path = learned_path(profile);
    match learned {
        Some(learned) => crate::profiles::write_json(&path, learned),
        None => match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(io(&path)(e)),
        },
    }
}

/// What an operator tells the agent, then what it learned under its own
/// heading: none if both are empty.
pub fn with_learned(operator: Option<&str>, learned: &str) -> Option<String> {
    let operator = operator.map(str::trim).filter(|t| !t.is_empty());
    let learned = learned.trim();
    match (operator, learned.is_empty()) {
        (None, true) => None,
        (Some(operator), true) => Some(operator.to_string()),
        (None, false) => Some(format!("{LEARNED}\n{learned}")),
        (Some(operator), false) => Some(format!("{operator}\n\n{LEARNED}\n{learned}")),
    }
}

/// What `profile`'s sessions are told beyond the fixed system prompt: its
/// operator's words, then what it learned.
pub fn instructions_of(profile: &Profile) -> Option<String> {
    let learned = learned(&profile.name).map(|l| l.text).unwrap_or_default();
    with_learned(profile.instructions.as_deref(), &learned)
}

/// What `new` changes of `old`, in a few words: what it adds or drops at
/// either end, or the part that differs, shown short.
pub fn changed_text(old: &str, new: &str) -> String {
    let (old, new) = (old.trim(), new.trim());
    if old == new {
        return "the same words".into();
    }
    if old.is_empty() {
        return format!("{:?}", clip(new, 80));
    }
    if new.is_empty() {
        return "nothing".into();
    }
    let a: Vec<char> = old.chars().collect();
    let b: Vec<char> = new.chars().collect();
    let prefix = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let suffix = a[prefix..]
        .iter()
        .rev()
        .zip(b[prefix..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let added: String = b[prefix..b.len() - suffix].iter().collect();
    let dropped: String = a[prefix..a.len() - suffix].iter().collect();
    match (added.trim().is_empty(), dropped.trim().is_empty()) {
        (false, true) => format!("adding {:?}", clip(&added, 80)),
        (true, false) => format!("dropping {:?}", clip(&dropped, 80)),
        _ => format!(
            "{:?} in place of {:?}",
            clip(&added, 60),
            clip(&dropped, 40)
        ),
    }
}

/// What the loop changes of an agent: what it learned, and the models it
/// runs on, best first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Variant {
    /// What it learned.
    pub learned: String,
    /// Its models, best first.
    pub models: Vec<String>,
}

impl Variant {
    /// `profile`'s agent as it is.
    pub fn of(profile: &Profile) -> Variant {
        Variant {
            learned: learned(&profile.name).map(|l| l.text).unwrap_or_default(),
            models: profile.models.clone(),
        }
    }

    /// Its digest: two variants with the same words and models have the
    /// same one.
    pub fn digest(&self) -> Hash32 {
        let models = self.models.join("\n");
        sion_core::hash::hash_parts(
            "sion-cli/improve/variant/v1",
            &[self.learned.trim().as_bytes(), models.as_bytes()],
        )
    }

    /// What it changes of `from`, in a line.
    pub fn change_from(&self, from: &Variant) -> String {
        let mut parts = Vec::new();
        if self.learned.trim() != from.learned.trim() {
            parts.push(if self.learned.trim().is_empty() {
                "forgets what it learned".to_string()
            } else if from.learned.trim().is_empty() {
                format!("learns {:?}", clip(&self.learned, 80))
            } else {
                format!(
                    "rewrites what it learned, {}",
                    changed_text(&from.learned, &self.learned)
                )
            });
        }
        if self.models != from.models {
            parts.push(format!("runs on {}", self.models.join(", ")));
        }
        if parts.is_empty() {
            "changes nothing".into()
        } else {
            parts.join(" and ")
        }
    }
}

/// The instructions the proposer works under, unless an improver's own say
/// otherwise.
pub const PROPOSER_SYSTEM: &str = "You improve a system from the evidence of how it did. You \
     are shown what it is — the parts that are fixed and the parts you may change — how it did \
     on its tasks, the tasks it failed and what went wrong, and what was tried before and how it \
     went. Propose changes with the tool you are given, filling only the parts you may change, \
     each with why it would help. Write general lessons that would have prevented the failures, \
     never the answers to particular tasks: each change is tried on these tasks and then \
     confirmed on tasks you are not shown, so a change that only fits the tasks you see will not \
     be kept. When you are done, say so.";

/// How the agent's changes are written: the improver, which can itself be
/// improved ([`crate::improve_loop::improve_improver`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Improver {
    /// The proposer's instructions.
    pub instructions: String,
    /// The changes it proposes a round, at most.
    pub proposals: u32,
    /// The failed tasks it is shown, at most.
    pub failures: u32,
    /// Whether it is shown what was tried before and how it went.
    pub history: bool,
}

impl Default for Improver {
    fn default() -> Improver {
        Improver {
            instructions: PROPOSER_SYSTEM.into(),
            proposals: 3,
            failures: 6,
            history: true,
        }
    }
}

impl Improver {
    /// Whether it can be used: instructions within the limit, 1 to 8
    /// proposals, 1 to 20 failures shown.
    pub fn check(&self) -> Result<(), String> {
        if self.instructions.trim().is_empty() || self.instructions.len() > MAX_LEARNED {
            return Err(format!(
                "an improver's instructions are 1 to {MAX_LEARNED} bytes"
            ));
        }
        if !(1..=8).contains(&self.proposals) {
            return Err("an improver proposes 1 to 8 changes a round".into());
        }
        if !(1..=20).contains(&self.failures) {
            return Err("an improver is shown 1 to 20 failed tasks".into());
        }
        Ok(())
    }

    /// Its digest.
    pub fn digest(&self) -> Hash32 {
        sion_core::hash::hash_parts(
            "sion-cli/improve/improver/v1",
            &[
                self.instructions.trim().as_bytes(),
                &self.proposals.to_be_bytes(),
                &self.failures.to_be_bytes(),
                &[u8::from(self.history)],
            ],
        )
    }
}

/// Where a profile's improver is kept.
pub fn improver_path(profile: &str) -> PathBuf {
    dir().join(format!("{profile}-improver.json"))
}

/// `profile`'s improver: its own, or the default.
pub fn improver(profile: &str) -> Result<Improver, CliError> {
    let path = improver_path(profile);
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text)
            .map_err(|e| CliError::Usage(format!("{}: {e}", path.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Improver::default()),
        Err(e) => Err(io(&path)(e)),
    }
}

/// A change the proposer proposed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Proposal {
    /// What the agent would have learned, in full.
    pub learned: Option<String>,
    /// The models it would run on.
    pub models: Option<Vec<String>>,
    /// Why it would help.
    pub why: String,
}

impl Proposal {
    /// `parent` changed by it.
    pub fn applied_to(&self, parent: &Variant) -> Variant {
        Variant {
            learned: self
                .learned
                .clone()
                .map(|t| t.trim().to_string())
                .unwrap_or_else(|| parent.learned.clone()),
            models: self.models.clone().unwrap_or_else(|| parent.models.clone()),
        }
    }
}

/// The tool through which the proposer proposes.
struct ProposeChange {
    found: Arc<Mutex<Vec<Proposal>>>,
    most: usize,
}

impl Tool for ProposeChange {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "propose_change".into(),
            description: "Propose a change to the agent: `learned`, the full new text of what it \
                          has learned; `models`, the models to run it on, best first; and `why`."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "learned": {"type": "string"},
                    "models": {"type": "array", "items": {"type": "string"}},
                    "why": {"type": "string"}
                },
                "required": ["why"]
            }),
        }
    }

    fn effect(&self) -> Effect {
        Effect::Write
    }

    fn call(&self, input: &Value, _key: &str) -> ToolOutcome {
        let learned = input["learned"].as_str().map(str::to_string);
        let models: Option<Vec<String>> = input["models"].as_array().map(|list| {
            list.iter()
                .filter_map(|m| m.as_str())
                .map(|m| m.trim().to_string())
                .filter(|m| !m.is_empty())
                .collect()
        });
        if learned.is_none() && models.is_none() {
            return ToolOutcome::error("give `learned`, `models`, or both");
        }
        let mut found = self.found.lock().expect("the proposals' lock");
        if found.len() >= self.most {
            return ToolOutcome::error(format!(
                "that is all {} changes this round takes",
                self.most
            ));
        }
        found.push(Proposal {
            learned,
            models,
            why: input["why"].as_str().unwrap_or_default().to_string(),
        });
        ToolOutcome::ok(
            "proposed: it will be tried on the tasks, then confirmed on tasks you are not shown",
        )
    }
}

pub(crate) fn clip(text: &str, most: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= most {
        return text.to_string();
    }
    let cut: String = text.chars().take(most).collect();
    format!("{cut}…")
}

/// What the proposer is shown of a variant's runs: the tasks it failed, at
/// most `most`, each with what went wrong and its answer.
pub(crate) fn failures_shown(set: &TaskSet, result: &ArmResult, most: usize) -> (String, usize) {
    let mut text = String::new();
    let mut failed = 0;
    for case in &set.cases {
        let runs: Vec<_> = result.trials.iter().filter(|t| t.case == case.id).collect();
        let Some(failing) = runs.iter().find(|t| !t.passed) else {
            continue;
        };
        failed += 1;
        if failed > most {
            continue;
        }
        let passed = runs.iter().filter(|t| t.passed).count();
        text.push_str(&format!(
            "- Task {}: {}\n  Passed {passed} of {} runs. What went wrong: {}\n  Its answer: {}\n",
            case.id,
            clip(&case.task, 1_500),
            runs.len(),
            clip(&failing.failures.join("; "), 600),
            if failing.answer.trim().is_empty() {
                "(none)".to_string()
            } else {
                clip(&failing.answer, 400)
            }
        ));
    }
    (text, failed)
}

/// The calls that did not go as the agent expected that the proposer is
/// shown, at most.
pub const SURPRISES_SHOWN: usize = 12;

/// What the proposer is shown of what the agent did not expect in its runs
/// ([`sion_agent::expect`]): each call that went otherwise than it said,
/// once for a task however many of the task's runs it came up in, those of
/// runs that failed first, at most `most`. With them, how many there were.
///
/// A line holds the tool's name, the agent's expectation in its own words
/// and how the call differed, which the runtime words itself: nothing of
/// what a tool answered is in it.
pub fn surprises_shown(set: &TaskSet, result: &ArmResult, most: usize) -> (String, usize) {
    struct Seen<'a> {
        case: &'a str,
        line: &'a str,
        runs: usize,
        failed: usize,
    }
    let mut seen: Vec<Seen<'_>> = Vec::new();
    for case in &set.cases {
        for trial in result.trials.iter().filter(|t| t.case == case.id) {
            let mut once = BTreeSet::new();
            for line in &trial.surprises {
                if !once.insert(line.as_str()) {
                    continue;
                }
                let failed = usize::from(!trial.passed);
                match seen
                    .iter_mut()
                    .find(|s| s.case == case.id && s.line == line)
                {
                    Some(entry) => {
                        entry.runs += 1;
                        entry.failed += failed;
                    }
                    None => seen.push(Seen {
                        case: &case.id,
                        line,
                        runs: 1,
                        failed,
                    }),
                }
            }
        }
    }
    let total = seen.len();
    // Stable: within each half, the tasks' own order.
    seen.sort_by_key(|s| s.failed == 0);
    let mut text = String::new();
    for entry in seen.iter().take(most) {
        let runs = match (entry.runs, entry.failed) {
            (1, 0) => "in a run that passed".to_string(),
            (1, _) => "in a run that failed".to_string(),
            (n, 0) => format!("in {n} runs, all passed"),
            (n, failed) if failed == n => format!("in {n} runs, all failed"),
            (n, failed) => format!("in {n} runs, {failed} failed"),
        };
        text.push_str(&format!(
            "- Task {} ({runs}): {}\n",
            entry.case,
            clip(entry.line, 500)
        ));
    }
    (text, total)
}

/// What the proposer is told about the agent and its failures.
#[allow(clippy::too_many_arguments)]
fn proposer_prompt(
    operator: Option<&str>,
    parent: &Variant,
    allowed: &[String],
    set: &TaskSet,
    result: &ArmResult,
    improver: &Improver,
    history: &[String],
) -> String {
    let (failures, failed) = failures_shown(set, result, improver.failures as usize);
    let shown = failed.min(improver.failures as usize);
    let mut text = format!(
        "The system to improve is an AI agent. Fixed: what its operator tells it. You may \
         change, with propose_change: `learned`, the full new text of what it has learned (at \
         most {MAX_LEARNED} bytes), and `models`, the models it runs on, best first, from those \
         it may run on.\n\n\
         What its operator tells it (fixed):\n{}\n\nWhat it has learned so far:\n{}\n\n\
         The models it runs on, best first: {}\nModels it may run on: {}\n\n\
         How it did: {} of {} runs of the tasks passed.\n\nTasks it failed ({shown} of {failed} shown):\n{failures}",
        operator.map(str::trim).filter(|t| !t.is_empty()).unwrap_or("(nothing)"),
        if parent.learned.trim().is_empty() {
            "(nothing yet)"
        } else {
            parent.learned.trim()
        },
        parent.models.join(", "),
        allowed.join(", "),
        result.passed(),
        result.total(),
    );
    let (surprises, surprised) = surprises_shown(set, result, SURPRISES_SHOWN);
    if surprised > 0 {
        text.push_str(&format!(
            "\nWhat it did not expect ({} of {surprised} shown). With a call, the agent says \
             what it expects the call to do, and its runtime compares that with what the call \
             did. Each of these is a call that went otherwise, with what the agent expected in \
             its own words: something it believed about its tools or its task was wrong.\n\
             {surprises}",
            surprised.min(SURPRISES_SHOWN),
        ));
    }
    if improver.history && !history.is_empty() {
        text.push_str("\nWhat was tried before, and how it went:\n");
        for line in history.iter().rev().take(20).rev() {
            text.push_str(&format!("- {line}\n"));
        }
    }
    text.push_str(&format!("\nPropose up to {} changes.", improver.proposals));
    text
}

/// Asks `proposer`, under `improver`, for changes to `parent`. Returns
/// them and the tokens it used.
#[allow(clippy::too_many_arguments)]
pub fn propose(
    proposer: &dyn Model,
    improver: &Improver,
    operator: Option<&str>,
    parent: &Variant,
    allowed: &[String],
    set: &TaskSet,
    result: &ArmResult,
    history: &[String],
    round: usize,
) -> (Vec<Proposal>, u64) {
    let prompt = proposer_prompt(operator, parent, allowed, set, result, improver, history);
    let found = Arc::new(Mutex::new(Vec::new()));
    let mut tools = Toolbox::new();
    tools.add(ProposeChange {
        found: found.clone(),
        most: improver.proposals as usize,
    });
    let limits = Limits {
        turns: improver.proposals + 2,
        turns_per_progress: 0,
        max_turns: improver.proposals + 2,
        ..Limits::default()
    };
    let runtime = Runtime {
        models: vec![proposer],
        tools: &tools,
        policy: Policy {
            autonomy: Autonomy::L3,
            ..Policy::default()
        },
        limits: limits.clone(),
        system: improver.instructions.clone(),
        now: &|| 0,
        stopped: &|| false,
        watch: None,
        steer: None,
        checkpoint: None,
        verify: None,
        code: None,
        hooks: None,
        extras: Default::default(),
    };
    let mut state = State::start(&format!("improve-propose-{round}"), &prompt, &limits);
    runtime.run(&mut state);
    let proposals = std::mem::take(&mut *found.lock().expect("the proposals' lock"));
    (proposals, state.usage.input + state.usage.output)
}

/// How a variant did on a set of tasks.
#[derive(Clone, Debug)]
pub struct Scored {
    /// Its runs.
    pub result: ArmResult,
    /// Runs passed and made, canaries apart.
    pub outcome: Outcome,
    /// Whether it passed each task, canaries apart, in the set's order.
    pub cases: Vec<bool>,
    /// The canaries it passed.
    pub canaries: Vec<String>,
}

/// Runs every task of `set` (whose digest is `digest`) `runs` times with
/// `profile`'s agent as `variant` makes it, each run within
/// `budget_tokens`.
pub fn score(
    profile: &Profile,
    variant: &Variant,
    set: &TaskSet,
    digest: Hash32,
    runs: u32,
    budget_tokens: u64,
) -> Result<Scored, CliError> {
    if variant.models.is_empty() {
        return Err(CliError::Usage(format!(
            "profile {} has no model to run the tasks with",
            profile.name
        )));
    }
    let instructions =
        with_learned(profile.instructions.as_deref(), &variant.learned).unwrap_or_default();
    let mut tasks = set.clone();
    tasks.system = Some(sion_agent::evolve::with_instructions(
        set.system(),
        &instructions,
    ));
    let make = crate::skill_improve::models_of(&variant.models);
    let comparison = sion_agent::compare::compare(
        &tasks,
        digest,
        &[Arm::Single],
        &make,
        None,
        budget_tokens,
        runs,
        // An agent that says what it expects does so on its tasks too.
        &Limits {
            expect: profile.expect,
            ..Limits::default()
        },
    )
    .map_err(CliError::Usage)?;
    let result = comparison
        .arms
        .into_iter()
        .next()
        .ok_or_else(|| CliError::Usage("the tasks ran nothing".into()))?;
    Ok(scored(set, result))
}

/// How the runs in `result` of `set`'s tasks came out: runs passed and
/// made and each task passed, canaries apart, and the canaries passed.
pub fn scored(set: &TaskSet, result: ArmResult) -> Scored {
    let canary: BTreeSet<&str> = set
        .cases
        .iter()
        .filter(|c| sion_agent::guard::is_canary(c))
        .map(|c| c.id.as_str())
        .collect();
    let per_case = crate::self_improve::per_case(set, &result);
    let cases = set
        .cases
        .iter()
        .zip(&per_case)
        .filter(|(c, _)| !canary.contains(c.id.as_str()))
        .map(|(_, p)| *p)
        .collect();
    let counted: Vec<_> = result
        .trials
        .iter()
        .filter(|t| !canary.contains(t.case.as_str()))
        .collect();
    let canaries = result
        .trials
        .iter()
        .filter(|t| t.passed && canary.contains(t.case.as_str()))
        .map(|t| t.case.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let outcome = Outcome {
        passed: counted.iter().filter(|t| t.passed).count(),
        total: counted.len(),
        tokens: result.tokens(),
    };
    Scored {
        result,
        outcome,
        cases,
        canaries,
    }
}

/// A change the loop kept, with the evidence for keeping it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Step {
    /// Its id: hex digits of the digest of what it records.
    pub id: String,
    /// The kept change it follows, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// The agent.
    pub agent: ObjectId,
    /// What it changes, in a line.
    pub change: String,
    /// Why the proposer said it would help.
    pub why: String,
    /// The agent before it.
    pub from: Variant,
    /// The agent after it.
    pub to: Variant,
    /// The digest of the tasks it was chosen on.
    pub train: String,
    /// The digest of the sealed tasks it was confirmed on.
    pub sealed: String,
    /// The seal's claim, if the seal was published.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seal_claim: Option<String>,
    /// How the agent before did on the tasks it was chosen on.
    pub train_before: Outcome,
    /// How it did after.
    pub train_after: Outcome,
    /// Newcombe's 95% interval for the difference there.
    pub screen: (f64, f64),
    /// How the agent before did on the sealed tasks, canaries apart.
    pub sealed_before: Outcome,
    /// How it did after.
    pub sealed_after: Outcome,
    /// The interval for the difference there, at the seal's level.
    pub confirm: (f64, f64),
    /// The z of that level.
    pub z: f64,
    /// Which of the seal's confirmations this was; for a reusable holdout,
    /// how much of its budget was used when it was kept.
    pub confirmation: u32,
    /// Of how many.
    pub budget: u32,
    /// If the sealed tasks were a reusable holdout
    /// ([`sion_agent::holdout`]): the gain it answered before the change
    /// was confirmed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answered: Option<sion_agent::holdout::Answer>,
    /// The digest of the practice tasks among those it was chosen on, if
    /// there were any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub practice: Option<String>,
    /// What a challenger's variants of the tasks it newly passed showed, if
    /// it was challenged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub challenge: Option<Challenge>,
    /// When it was kept (seconds since the epoch).
    pub at: u64,
    /// The claim that published it, if one did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim: Option<String>,
}

impl Step {
    /// What its claim holds: all it records but the claim itself.
    pub fn claimed(&self) -> Result<Vec<u8>, CliError> {
        let mut bare = self.clone();
        bare.claim = None;
        serde_json::to_vec(&bare)
            .map_err(|e| CliError::Usage(format!("the change could not be written: {e}")))
    }

    fn identify(&mut self) {
        let mut bare = self.clone();
        bare.id = String::new();
        bare.claim = None;
        let bytes = serde_json::to_vec(&bare).unwrap_or_default();
        self.id = sion_core::hash::hash_domain("sion-cli/improve/step/v1", &bytes).to_hex()[..16]
            .to_string();
    }
}

/// What the profile had before a change was made its own.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Before {
    /// What it had learned.
    pub learned: Option<String>,
    /// Its models.
    pub models: Vec<String>,
}

/// A line of a profile's lineage.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "record", rename_all = "snake_case")]
pub enum Record {
    /// A change kept.
    Kept(Step),
    /// A kept change made the profile's.
    Applied {
        /// The change.
        step: String,
        /// What it replaced.
        before: Before,
        /// When.
        at: u64,
    },
    /// An applied change taken back.
    Undone {
        /// The change.
        step: String,
        /// When.
        at: u64,
    },
    /// A change to the agent's own code kept, as a release.
    Released(crate::code_improve::Release),
    /// A release applied to a source checkout.
    Installed {
        /// The release.
        release: String,
        /// The checkout.
        source: PathBuf,
        /// When.
        at: u64,
    },
    /// A release taken back out of a source checkout.
    Uninstalled {
        /// The release.
        release: String,
        /// The checkout.
        source: PathBuf,
        /// When.
        at: u64,
    },
    /// A change to the improver kept.
    ImproverKept(ImproverStep),
    /// A kept change to the improver made the profile's.
    ImproverApplied {
        /// The change.
        step: String,
        /// The improver it replaced.
        before: Improver,
        /// When.
        at: u64,
    },
}

/// Where a profile's lineage is kept: one record a line.
pub fn lineage_path(profile: &str) -> PathBuf {
    dir().join(format!("{profile}-lineage.jsonl"))
}

/// Every record of a profile's lineage, oldest first.
pub fn records(profile: &str) -> Result<Vec<Record>, CliError> {
    let path = lineage_path(profile);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(io(&path)(e)),
    };
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            serde_json::from_str(l).map_err(|e| CliError::Usage(format!("{}: {e}", path.display())))
        })
        .collect()
}

/// The changes a profile kept, oldest first.
pub fn lineage(profile: &str) -> Result<Vec<Step>, CliError> {
    Ok(records(profile)?
        .into_iter()
        .filter_map(|r| match r {
            Record::Kept(step) => Some(step),
            _ => None,
        })
        .collect())
}

/// Records `step`, a change kept, in `profile`'s lineage.
pub fn record(profile: &str, step: &Step) -> Result<(), CliError> {
    append(profile, &Record::Kept(step.clone()))
}

pub(crate) fn append(profile: &str, record: &Record) -> Result<(), CliError> {
    let path = lineage_path(profile);
    std::fs::create_dir_all(dir()).map_err(io(&dir()))?;
    let line = serde_json::to_string(record)
        .map_err(|e| CliError::Usage(format!("the lineage could not be written: {e}")))?;
    use std::io::Write as _;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(io(&path))?;
    file.write_all(format!("{line}\n").as_bytes())
        .map_err(io(&path))
}

/// Makes `step`'s agent `profile`'s: what it learned, and its models.
/// Records what it replaced. Returns a line for each change made.
pub fn apply(profile: &str, step: &Step, now: u64) -> Result<Vec<String>, CliError> {
    let mut all = crate::profiles::all()?;
    let found = all
        .get_mut(profile)
        .ok_or_else(|| CliError::Usage(format!("no agent profile {profile}")))?;
    let before = Before {
        learned: learned(profile).map(|l| l.text),
        models: found.models.clone(),
    };
    let mut lines = Vec::new();
    if step.to.learned.trim() != before.learned.clone().unwrap_or_default().trim() {
        let kept = (!step.to.learned.trim().is_empty()).then(|| Learned {
            text: step.to.learned.trim().to_string(),
            at: now,
            step: Some(step.id.clone()),
        });
        set_learned(profile, kept.as_ref())?;
        lines.push(format!(
            "profile {profile} now has learned what step {} kept",
            step.id
        ));
    }
    if step.to.models != found.models {
        found.models = step.to.models.clone();
        crate::profiles::save_all(&all)?;
        lines.push(format!(
            "profile {profile} now runs on {}",
            step.to.models.join(", ")
        ));
    }
    append(
        profile,
        &Record::Applied {
            step: step.id.clone(),
            before,
            at: now,
        },
    )?;
    Ok(lines)
}

/// Takes back the last change made `profile`'s that is not taken back
/// yet: what it learned and its models as they were before it.
pub fn undo(profile: &str, now: u64) -> Result<Vec<String>, CliError> {
    let records = records(profile)?;
    let applied_step = |r: &Record| match r {
        Record::Applied { step, .. } | Record::ImproverApplied { step, .. } => Some(step.clone()),
        _ => None,
    };
    let mut undone: BTreeSet<usize> = BTreeSet::new();
    for (at, record) in records.iter().enumerate() {
        if let Record::Undone { step, .. } = record {
            if let Some(applied) = records[..at]
                .iter()
                .rposition(|r| applied_step(r).as_ref() == Some(step))
            {
                undone.insert(applied);
            }
        }
    }
    let Some((_, last)) = records
        .iter()
        .enumerate()
        .rev()
        .find(|(i, r)| applied_step(r).is_some() && !undone.contains(i))
    else {
        return Err(CliError::Usage(format!(
            "profile {profile} has no change made its own to take back"
        )));
    };
    let line = match last {
        Record::Applied { step, before, .. } => {
            set_learned(
                profile,
                before
                    .learned
                    .as_ref()
                    .filter(|t| !t.trim().is_empty())
                    .map(|text| Learned {
                        text: text.clone(),
                        at: now,
                        step: None,
                    })
                    .as_ref(),
            )?;
            let mut all = crate::profiles::all()?;
            if let Some(found) = all.get_mut(profile) {
                if found.models != before.models {
                    found.models = before.models.clone();
                    crate::profiles::save_all(&all)?;
                }
            }
            format!(
                "took back step {step}: profile {profile} has what it learned and its models as they were before it"
            )
        }
        Record::ImproverApplied { step, before, .. } => {
            crate::profiles::write_json(&improver_path(profile), before)?;
            format!("took back step {step}: profile {profile}'s improver is as it was before it")
        }
        _ => unreachable!("only applied changes are taken back"),
    };
    let step = applied_step(last).unwrap_or_default();
    append(profile, &Record::Undone { step, at: now })?;
    Ok(vec![line])
}

/// What a loop is given to run with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    /// Rounds, at most.
    pub rounds: usize,
    /// Times each task is run by each variant.
    pub runs: u32,
    /// Tokens each run may use.
    pub budget_tokens: u64,
    /// Tokens the whole loop may use, its proposer's included (0: no limit
    /// but the rounds).
    pub total_tokens: u64,
    /// Models the proposer may choose among, besides the profile's own.
    pub models: Vec<String>,
    /// The seed of the archive's draws.
    pub seed: u64,
}

/// The tasks a loop chooses changes on and confirms them on.
#[derive(Debug)]
pub struct Problem<'a> {
    /// Tasks changes are chosen on.
    pub train: &'a TaskSet,
    /// Their digest.
    pub train_digest: Hash32,
    /// Tasks changes are confirmed on, which the proposer is never shown.
    pub sealed: &'a TaskSet,
    /// Their digest.
    pub sealed_digest: Hash32,
    /// The z each confirmation is held to.
    pub z: f64,
    /// Whether the sealed tasks are a reusable holdout
    /// ([`sion_agent::holdout`]): asked first about a change's gain, it
    /// turns down, without spending a confirmation, a change whose gain the
    /// sealed tasks answer is not above its threshold; any other change is
    /// confirmed as on any seal.
    pub reusable: bool,
    /// The digest of the practice tasks among `train`, if there are any
    /// ([`crate::practice`]).
    pub practice: Option<Hash32>,
    /// A model that writes variants of the tasks a change newly passes, if
    /// one is set: a change must be better on them too, beyond chance,
    /// before it is confirmed ([`challenge`]).
    pub challenger: Option<Challenger<'a>>,
}

/// A model that writes variants of the tasks a change newly passes, to
/// catch a change that only fits them.
pub struct Challenger<'a>(pub &'a dyn Model);

impl std::fmt::Debug for Challenger<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Challenger({})", self.0.id())
    }
}

/// The most tasks a change newly passes that the challenger varies.
pub const CHALLENGED: usize = 4;

/// The most variants the challenger writes of each.
pub const VARIANTS: usize = 2;

/// What the challenger is told it is.
pub const CHALLENGER_SYSTEM: &str = "You test changes to an AI agent. A change made the agent \
pass tasks it failed before. You write variants of those tasks that a change which only fits them \
would fail and one which does what they need would pass, each with write_task, with a solution \
and failures that show its checks mean what they say.";

/// What a challenger's variants showed of a change.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Challenge {
    /// The digest of the variants that held up.
    pub variants: String,
    /// How many there were.
    pub cases: usize,
    /// How the agent before the change did on them.
    pub before: Outcome,
    /// How it did after.
    pub after: Outcome,
    /// Newcombe's 95% interval for the difference.
    pub interval: (f64, f64),
}

/// The challenger's prompt: the change, and the tasks it newly passes.
pub fn challenger_prompt(change: &str, tasks: &[&sion_agent::evals::Case], most: usize) -> String {
    let mut text = format!(
        "A change to an agent made it pass the tasks below, which it failed before.\n\nThe change: {}\n\nThe tasks:\n",
        clip(change, 4_000)
    );
    for case in tasks {
        text.push_str(&sion_agent::practice::shown(case));
    }
    text.push_str(&format!(
        "\nWrite up to {most} variants of these tasks with write_task, `of` naming the task each \
         varies: the same kind of request, changed where a change that only fits these tasks would \
         go wrong (other names, amounts or wording, a step more or less), while a change that does \
         what they need would still pass."
    ));
    text
}

/// What came of challenging a change.
struct Challenged {
    /// What its variants showed, when any held up.
    challenge: Option<Challenge>,
    /// Why variants were refused.
    refused: Vec<String>,
    /// A canary whose code word a variant held.
    canary: Option<String>,
    /// The tokens used.
    tokens: u64,
}

/// Has `challenger` write variants of `tasks`, which `child` newly passes
/// after `change`, and runs `incumbent` and `child` on those that hold up:
/// none that is a sealed task or a task to choose on over again.
#[allow(clippy::too_many_arguments)]
fn challenge(
    profile: &Profile,
    incumbent: &Variant,
    child: &Variant,
    change: &str,
    tasks: &[&sion_agent::evals::Case],
    challenger: &dyn Model,
    sealed: &[sion_agent::evals::Case],
    train: &TaskSet,
    settings: &Settings,
) -> Result<Challenged, CliError> {
    let most = tasks.len() * VARIANTS;
    let writing = sion_agent::practice::write(
        challenger,
        CHALLENGER_SYSTEM,
        &challenger_prompt(change, tasks, most),
        most,
    );
    let mut found = Challenged {
        challenge: None,
        refused: writing.refused.clone(),
        canary: None,
        tokens: writing.tokens,
    };
    let mut cases: Vec<sion_agent::evals::Case> = Vec::new();
    for (at, written) in writing.written.iter().enumerate() {
        let case = written.case(&format!("variant-{at}"));
        match crate::practice::against_sealed(&case, sealed) {
            Some(crate::practice::Against::Canary(canary)) => {
                found.canary = Some(canary);
                return Ok(found);
            }
            Some(crate::practice::Against::Copy(_)) => {
                found
                    .refused
                    .push(format!("variant {} is a sealed task", at + 1));
                continue;
            }
            None => {}
        }
        if train
            .cases
            .iter()
            .chain(&cases)
            .any(|c| sion_agent::practice::same_task(&c.task, &case.task))
        {
            found
                .refused
                .push(format!("variant {} is a task to choose on", at + 1));
            continue;
        }
        cases.push(case);
    }
    if cases.is_empty() {
        return Ok(found);
    }
    let set = TaskSet {
        name: "challenge".into(),
        about: format!("variants a challenger wrote of the tasks {change} newly passes"),
        system: train.system.clone(),
        cases,
    };
    let digest = crate::practice::digest_of(&set)?;
    let before = score(
        profile,
        incumbent,
        &set,
        digest,
        settings.runs,
        settings.budget_tokens,
    )?;
    let after = score(
        profile,
        child,
        &set,
        digest,
        settings.runs,
        settings.budget_tokens,
    )?;
    found.tokens += before.outcome.tokens + after.outcome.tokens;
    let interval = newcombe(
        (after.outcome.passed, after.outcome.total),
        (before.outcome.passed, before.outcome.total),
    );
    found.challenge = Some(Challenge {
        variants: digest.to_hex(),
        cases: set.cases.len(),
        before: before.outcome,
        after: after.outcome,
        interval,
    });
    Ok(found)
}

/// What a reusable holdout answered: the answer, how much of its budget
/// is used and how much it had, and its threshold.
pub type Asked = (sion_agent::holdout::Answer, u32, u32, f64);

/// What a loop does with its confirmations and the changes it keeps.
pub struct Sinks<'a> {
    /// Spends a confirmation: returns which it is and of how many, or an
    /// error when none is left.
    pub spend: &'a mut dyn FnMut() -> Result<(u32, u32), CliError>,
    /// Asks a reusable holdout about a gain measured on the tasks to choose
    /// on and on the sealed tasks; an error when its budget is spent.
    pub ask: &'a mut dyn FnMut(f64, f64) -> Result<Asked, CliError>,
    /// Records a kept change (publishing it, making it the profile's).
    pub kept: &'a mut dyn FnMut(&mut Step) -> Result<Vec<String>, CliError>,
    /// Whether to stop before the next model is asked (the kill switch).
    pub stopped: &'a dyn Fn() -> bool,
    /// The time, in seconds since the epoch.
    pub now: &'a dyn Fn() -> u64,
}

impl std::fmt::Debug for Sinks<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Sinks")
    }
}

/// What a loop came to.
#[derive(Clone, Debug, Default)]
pub struct Report {
    /// What it did, a line each.
    pub lines: Vec<String>,
    /// The changes it kept, in order.
    pub kept: Vec<Step>,
    /// The tokens it used.
    pub tokens: u64,
    /// The agent as it left it.
    pub last: Option<Variant>,
    /// How the agent as it left it did on the sealed tasks, if they were
    /// run.
    pub sealed: Option<Outcome>,
    /// Why it stopped, if before its rounds were done.
    pub stopped: Option<String>,
    /// Every variant tried.
    pub archive: Option<Archive<Variant>>,
}

/// One round's attempt, as the proposer's history tells it: the change
/// and how it went, never what a sealed task's run said.
pub(crate) fn told(change: &str, how: &str) -> String {
    format!("{change}: {how}")
}

/// Runs the loop on `profile`'s agent: up to `settings.rounds` rounds,
/// `proposer` writing changes under `improver`, each tried on
/// `problem.train` and confirmed on `problem.sealed`. `archive` is where
/// earlier runs on the same tasks left their variants, to go on from.
#[allow(clippy::too_many_arguments)]
pub fn improve(
    profile: &Profile,
    problem: &Problem<'_>,
    proposer: &dyn Model,
    improver: &Improver,
    settings: &Settings,
    archive: Option<Archive<Variant>>,
    sinks: &mut Sinks<'_>,
) -> Result<Report, CliError> {
    improver.check().map_err(CliError::Usage)?;
    let mut report = Report::default();
    let mut allowed: Vec<String> = profile.models.clone();
    for model in &settings.models {
        if !allowed.contains(model) {
            allowed.push(model.clone());
        }
    }
    let sealed_cases: Vec<sion_agent::evals::Case> = problem.sealed.cases.clone();
    let start = Variant::of(profile);
    if let Some((case, _)) = sion_agent::guard::leak(&start.learned, &sealed_cases) {
        return Err(CliError::Usage(format!(
            "what the agent has already learned shares words with sealed task {case}: seal tasks it has not learned from"
        )));
    }
    let first = score(
        profile,
        &start,
        problem.train,
        problem.train_digest,
        settings.runs,
        settings.budget_tokens,
    )?;
    report.tokens += first.outcome.tokens;
    let mut archive = match archive {
        Some(mut archive) => {
            // The agent as it is now joins the archive if it is not in it.
            if let Some(node) = archive
                .nodes
                .iter_mut()
                .find(|n| n.variant.digest() == start.digest())
            {
                node.passed = first.outcome.passed;
                node.total = first.outcome.total;
            } else {
                archive
                    .add(
                        0,
                        start.clone(),
                        first.outcome.passed,
                        first.outcome.total,
                        "the agent as it now is",
                    )
                    .map_err(CliError::Usage)?;
            }
            archive
        }
        None => Archive::new(start.clone(), first.outcome.passed, first.outcome.total),
    };
    let mut results: Vec<Option<ArmResult>> = vec![None; archive.nodes.len()];
    let start_id = archive
        .nodes
        .iter()
        .position(|n| n.variant.digest() == start.digest())
        .unwrap_or(0);
    results[start_id] = Some(first.result.clone());
    let mut tried: BTreeSet<Hash32> = archive.nodes.iter().map(|n| n.variant.digest()).collect();
    let mut history: Vec<String> = archive
        .nodes
        .iter()
        .skip(1)
        .map(|n| {
            told(
                &n.note,
                &format!(
                    "{}/{} runs passed{}",
                    n.passed,
                    n.total,
                    if n.confirmed { ", kept" } else { "" }
                ),
            )
        })
        .collect();
    let mut incumbent = start.clone();
    let mut incumbent_train = first.outcome;
    let mut incumbent_cases = first.cases.clone();
    let mut incumbent_sealed: Option<Scored> = None;
    let mut last_step: Option<String> = crate::improve_loop::lineage(&profile.name)
        .ok()
        .and_then(|l| l.last().map(|s| s.id.clone()));
    report.lines.push(format!(
        "the agent as it is: {}/{} runs of the tasks to choose on passed",
        first.outcome.passed, first.outcome.total
    ));
    let mut draws = Draws::new(settings.seed);
    let over =
        |report: &Report| settings.total_tokens > 0 && report.tokens >= settings.total_tokens;
    'rounds: for round in 0..settings.rounds {
        if (sinks.stopped)() {
            report.stopped = Some("the kill switch is on".into());
            break;
        }
        if over(&report) {
            report.stopped = Some(format!("its {} tokens are used", settings.total_tokens));
            break;
        }
        let tau = temperature(round, settings.rounds);
        let Some(parent) = archive.pick(&mut draws, tau, &|n| n.failing()) else {
            report.stopped = Some(
                "every variant passes every task it was tried on: nothing left to learn from these tasks"
                    .into(),
            );
            break;
        };
        let parent_variant = archive.nodes[parent].variant.clone();
        let parent_result = match results.get(parent).cloned().flatten() {
            Some(result) => result,
            None => {
                let scored = score(
                    profile,
                    &parent_variant,
                    problem.train,
                    problem.train_digest,
                    settings.runs,
                    settings.budget_tokens,
                )?;
                report.tokens += scored.outcome.tokens;
                archive.nodes[parent].passed = scored.outcome.passed;
                archive.nodes[parent].total = scored.outcome.total;
                results[parent] = Some(scored.result.clone());
                scored.result
            }
        };
        report.lines.push(format!(
            "round {}: building on variant {parent} ({}), {}/{} runs passed",
            round + 1,
            archive.nodes[parent].note,
            archive.nodes[parent].passed,
            archive.nodes[parent].total
        ));
        let (_, surprised) = surprises_shown(problem.train, &parent_result, 0);
        if surprised > 0 {
            report.lines.push(format!(
                "  what it did not expect: {surprised} calls, each counted once a task; the \
                 proposer is shown {}",
                surprised.min(SURPRISES_SHOWN)
            ));
        }
        let (proposals, used) = propose(
            proposer,
            improver,
            profile.instructions.as_deref(),
            &parent_variant,
            &allowed,
            problem.train,
            &parent_result,
            &history,
            round,
        );
        report.tokens += used;
        if proposals.is_empty() {
            report.lines.push("  the proposer proposed nothing".into());
            history.push(format!("round {}: nothing was proposed", round + 1));
            continue;
        }
        for proposal in proposals.into_iter().take(improver.proposals as usize) {
            if (sinks.stopped)() {
                report.stopped = Some("the kill switch is on".into());
                break 'rounds;
            }
            let child = proposal.applied_to(&parent_variant);
            let change = child.change_from(&parent_variant);
            let refuse =
                |report: &mut Report, history: &mut Vec<String>, why: &str, told_why: &str| {
                    report.lines.push(format!("  turned down: {change}: {why}"));
                    history.push(told(&change, told_why));
                };
            if child.learned.len() > MAX_LEARNED {
                refuse(
                    &mut report,
                    &mut history,
                    &format!(
                        "what it learned would be {} bytes, more than {MAX_LEARNED}",
                        child.learned.len()
                    ),
                    "turned down: too long",
                );
                continue;
            }
            if child.models.is_empty() || child.models.len() > MAX_MODELS {
                refuse(
                    &mut report,
                    &mut history,
                    &format!("it runs on 1 to {MAX_MODELS} models"),
                    "turned down: no models, or too many",
                );
                continue;
            }
            if let Some(model) = child.models.iter().find(|m| !allowed.contains(m)) {
                refuse(
                    &mut report,
                    &mut history,
                    &format!("{model} is not among the models it may run on"),
                    "turned down: a model not allowed",
                );
                continue;
            }
            if !tried.insert(child.digest()) {
                refuse(
                    &mut report,
                    &mut history,
                    "it was tried already",
                    "turned down: tried already",
                );
                continue;
            }
            if let Some((case, _)) = sion_agent::guard::leak(&child.learned, &sealed_cases) {
                if case.starts_with(sion_agent::guard::CANARY) {
                    report.stopped = Some(format!(
                        "a proposal holds the code word of {case}: the sealed tasks have reached the proposer; seal fresh tasks"
                    ));
                    break 'rounds;
                }
                refuse(
                    &mut report,
                    &mut history,
                    &format!("it shares a run of words with sealed task {case}"),
                    "turned down: it repeats a task it should not know",
                );
                continue;
            }
            // Staged: one run of each task first, when there are more.
            if settings.runs > 1 {
                let quick = score(
                    profile,
                    &child,
                    problem.train,
                    problem.train_digest,
                    1,
                    settings.budget_tokens,
                )?;
                report.tokens += quick.outcome.tokens;
                let parent_rate = archive.nodes[parent].rate();
                if (quick.outcome.passed as f64 + 1.0) < parent_rate * quick.outcome.total as f64 {
                    archive
                        .add(
                            parent,
                            child.clone(),
                            quick.outcome.passed,
                            quick.outcome.total,
                            &change,
                        )
                        .map_err(CliError::Usage)?;
                    results.push(Some(quick.result));
                    refuse(&mut report, &mut history, &format!("{}/{} on a first run of each task, clearly worse than what it was made from", quick.outcome.passed, quick.outcome.total), &format!("{}/{} on a first run of each task: worse", quick.outcome.passed, quick.outcome.total));
                    continue;
                }
            }
            let trained = score(
                profile,
                &child,
                problem.train,
                problem.train_digest,
                settings.runs,
                settings.budget_tokens,
            )?;
            report.tokens += trained.outcome.tokens;
            let id = archive
                .add(
                    parent,
                    child.clone(),
                    trained.outcome.passed,
                    trained.outcome.total,
                    &change,
                )
                .map_err(CliError::Usage)?;
            results.push(Some(trained.result.clone()));
            let screen = newcombe(
                (trained.outcome.passed, trained.outcome.total),
                (incumbent_train.passed, incumbent_train.total),
            );
            if screen.0 <= 0.0 {
                report.lines.push(format!(
                    "  tried {change}: {}/{} against {}/{} (95% interval {:+.0}% to {:+.0}%): not better beyond chance",
                    trained.outcome.passed, trained.outcome.total, incumbent_train.passed, incumbent_train.total,
                    screen.0 * 100.0, screen.1 * 100.0
                ));
                history.push(told(
                    &change,
                    &format!(
                        "{}/{} runs passed against {}/{}: not better beyond chance",
                        trained.outcome.passed,
                        trained.outcome.total,
                        incumbent_train.passed,
                        incumbent_train.total
                    ),
                ));
                continue;
            }
            if over(&report) {
                report.stopped = Some(format!("its {} tokens are used", settings.total_tokens));
                break 'rounds;
            }
            // Challenged, when a challenger is set: variants of the tasks it
            // newly passes, written to catch a change that only fits them.
            let mut challenged: Option<Challenge> = None;
            if let Some(challenger) = &problem.challenger {
                let newly: Vec<&sion_agent::evals::Case> = problem
                    .train
                    .cases
                    .iter()
                    .zip(trained.cases.iter().zip(&incumbent_cases))
                    .filter(|(_, (now, before))| **now && !**before)
                    .map(|(case, _)| case)
                    .take(CHALLENGED)
                    .collect();
                if !newly.is_empty() {
                    let found = challenge(
                        profile,
                        &incumbent,
                        &child,
                        &change,
                        &newly,
                        challenger.0,
                        &sealed_cases,
                        problem.train,
                        settings,
                    )?;
                    report.tokens += found.tokens;
                    if let Some(canary) = found.canary {
                        report.stopped = Some(format!(
                            "a variant the challenger wrote holds the code word of {canary}: the sealed tasks have reached it; seal fresh tasks"
                        ));
                        break 'rounds;
                    }
                    match found.challenge {
                        Some(shown) if shown.interval.0 <= 0.0 => {
                            report.lines.push(format!(
                                "  tried {change}: better on the tasks to choose on ({}/{} against {}/{}), but a shortcut: on {} variants the challenger wrote of the tasks it newly passes, {}/{} against {}/{} (95% interval {:+.0}% to {:+.0}%)",
                                trained.outcome.passed, trained.outcome.total, incumbent_train.passed, incumbent_train.total,
                                shown.cases, shown.after.passed, shown.after.total, shown.before.passed, shown.before.total,
                                shown.interval.0 * 100.0, shown.interval.1 * 100.0
                            ));
                            history.push(told(
                                &change,
                                &format!(
                                    "{}/{} runs passed against {}/{}, but on variants of the tasks it newly passed, written to catch a change that only fits them, {}/{} against {}/{}: not kept",
                                    trained.outcome.passed, trained.outcome.total, incumbent_train.passed, incumbent_train.total,
                                    shown.after.passed, shown.after.total, shown.before.passed, shown.before.total
                                ),
                            ));
                            continue;
                        }
                        Some(shown) => {
                            report.lines.push(format!(
                                "  challenged {change}: on {} variants the challenger wrote of the tasks it newly passes, {}/{} against {}/{} (95% interval {:+.0}% to {:+.0}%)",
                                shown.cases, shown.after.passed, shown.after.total, shown.before.passed, shown.before.total,
                                shown.interval.0 * 100.0, shown.interval.1 * 100.0
                            ));
                            challenged = Some(shown);
                        }
                        None => report.lines.push(format!(
                            "  challenged {change}: no variant held up ({}), so it goes on unchallenged",
                            if found.refused.is_empty() {
                                "none was written".to_string()
                            } else {
                                clip(&found.refused.join("; "), 400)
                            }
                        )),
                    }
                    if over(&report) {
                        report.stopped =
                            Some(format!("its {} tokens are used", settings.total_tokens));
                        break 'rounds;
                    }
                }
            }
            // Confirmation, on tasks the proposer never saw.
            let before = match &incumbent_sealed {
                Some(scored) => scored.clone(),
                None => {
                    let scored = score(
                        profile,
                        &incumbent,
                        problem.sealed,
                        problem.sealed_digest,
                        settings.runs,
                        settings.budget_tokens,
                    )?;
                    report.tokens += scored.outcome.tokens;
                    if let Some(canary) = scored.canaries.first() {
                        report.stopped = Some(format!(
                            "the agent as it is passed canary {canary}: the sealed tasks' answers have reached it, or its scoring is gamed; seal fresh tasks"
                        ));
                        break 'rounds;
                    }
                    incumbent_sealed = Some(scored.clone());
                    scored
                }
            };
            // On a reusable holdout the sealed tasks are run first and asked
            // about the change's gain: a change they turn down spends no
            // confirmation. Whatever is kept is kept by a confirmation.
            let fixed_first = !problem.reusable;
            let mut spent = None;
            if fixed_first {
                match (sinks.spend)() {
                    Ok(confirmation) => spent = Some(confirmation),
                    Err(why) => {
                        report.stopped = Some(why.to_string());
                        break 'rounds;
                    }
                }
            }
            let after = score(
                profile,
                &child,
                problem.sealed,
                problem.sealed_digest,
                settings.runs,
                settings.budget_tokens,
            )?;
            report.tokens += after.outcome.tokens;
            if let Some(canary) = after.canaries.first() {
                report.stopped = Some(format!(
                    "a change passed canary {canary}: the sealed tasks' answers have reached it, or its scoring is gamed; seal fresh tasks"
                ));
                break 'rounds;
            }
            let rate = |o: &Outcome| o.passed as f64 / o.total.max(1) as f64;
            let mut answered = None;
            let mut turned_down = None;
            if problem.reusable {
                let train_gain = rate(&trained.outcome) - rate(&incumbent_train);
                let sealed_gain = rate(&after.outcome) - rate(&before.outcome);
                let (answer, used, budget, threshold) = match (sinks.ask)(train_gain, sealed_gain) {
                    Ok(asked) => asked,
                    Err(why) => {
                        report.stopped = Some(why.to_string());
                        break 'rounds;
                    }
                };
                answered = Some(answer);
                // Only an answer the sealed tasks gave turns a change down:
                // one taken from the tasks to choose on says no more than
                // that they agreed within the threshold, which is not that
                // the change helps.
                if answer.source == sion_agent::holdout::Source::Holdout
                    && answer.value <= threshold
                {
                    turned_down = Some(format!(
                        "the reusable holdout turned it down: the sealed tasks answered a gain of {:+.0}%, with noise, not above its threshold of {:.0}% (its budget {used} of {budget} used)",
                        answer.value * 100.0,
                        threshold * 100.0
                    ));
                } else {
                    match (sinks.spend)() {
                        Ok(confirmation) => spent = Some(confirmation),
                        Err(why) => {
                            report.stopped = Some(why.to_string());
                            break 'rounds;
                        }
                    }
                }
            }
            let (confirmation, budget) = spent.unwrap_or((0, 0));
            let confirm = newcombe_with(
                (after.outcome.passed, after.outcome.total),
                (before.outcome.passed, before.outcome.total),
                problem.z,
            );
            let z = problem.z;
            let verdict = match turned_down {
                Some(why) => Err(why),
                None => {
                    let regressed = before
                        .cases
                        .iter()
                        .zip(&after.cases)
                        .position(|(b, a)| *b && !a);
                    match regressed {
                        Some(at) => Err(format!("it fails sealed task {} that the agent passes", at + 1)),
                        None if confirm.0 <= 0.0 => Err(format!(
                            "{}/{} against {}/{} on the sealed tasks (interval at the seal's level {:+.0}% to {:+.0}%): not better beyond chance",
                            after.outcome.passed, after.outcome.total, before.outcome.passed, before.outcome.total,
                            confirm.0 * 100.0, confirm.1 * 100.0
                        )),
                        None => Ok(format!(
                            "interval at the seal's level {:+.0}% to {:+.0}%; confirmation {confirmation} of {budget}{}",
                            confirm.0 * 100.0, confirm.1 * 100.0,
                            match &answered {
                                Some(answer) => format!(", after the reusable holdout answered a gain of {:+.0}%", answer.value * 100.0),
                                None => String::new(),
                            }
                        )),
                    }
                }
            };
            if let Err(why) = &verdict {
                let why = if spent.is_some() {
                    format!("{why} (confirmation {confirmation} of {budget})")
                } else {
                    why.clone()
                };
                report.lines.push(format!(
                    "  tried {change}: better on the tasks to choose on ({}/{} against {}/{}), but not confirmed: {why}",
                    trained.outcome.passed, trained.outcome.total, incumbent_train.passed, incumbent_train.total
                ));
                history.push(told(&change, &format!(
                    "{}/{} runs passed against {}/{}, but it was not confirmed on the tasks you are not shown",
                    trained.outcome.passed, trained.outcome.total, incumbent_train.passed, incumbent_train.total
                )));
                continue;
            }
            let mut step = Step {
                id: String::new(),
                parent: last_step.clone(),
                agent: profile.agent,
                change: change.clone(),
                why: proposal.why.clone(),
                from: incumbent.clone(),
                to: child.clone(),
                train: problem.train_digest.to_hex(),
                sealed: problem.sealed_digest.to_hex(),
                seal_claim: None,
                train_before: incumbent_train,
                train_after: trained.outcome,
                screen,
                sealed_before: before.outcome,
                sealed_after: after.outcome,
                confirm,
                z,
                confirmation,
                budget,
                answered,
                practice: problem.practice.map(|d| d.to_hex()),
                challenge: challenged,
                at: (sinks.now)(),
                claim: None,
            };
            step.identify();
            let lines = (sinks.kept)(&mut step)?;
            report.lines.push(format!(
                "  kept {change}: {}/{} against {}/{} on the tasks to choose on, and {}/{} against {}/{} on the sealed tasks ({}), as step {}",
                trained.outcome.passed, trained.outcome.total, incumbent_train.passed, incumbent_train.total,
                after.outcome.passed, after.outcome.total, before.outcome.passed, before.outcome.total,
                verdict.as_deref().unwrap_or_default(), step.id
            ));
            report
                .lines
                .extend(lines.into_iter().map(|l| format!("  {l}")));
            history.push(told(&change, &format!(
                "{}/{} runs passed against {}/{}, and it was confirmed on the tasks you are not shown: kept",
                trained.outcome.passed, trained.outcome.total, incumbent_train.passed, incumbent_train.total
            )));
            archive.nodes[id].confirmed = true;
            last_step = Some(step.id.clone());
            incumbent = child;
            incumbent_train = trained.outcome;
            incumbent_cases = trained.cases.clone();
            incumbent_sealed = Some(after);
            report.kept.push(step);
        }
    }
    if report.kept.is_empty() {
        report
            .lines
            .push("nothing was confirmed better: the agent is left as it is".into());
    }
    if let Some(why) = &report.stopped {
        report.lines.push(format!("stopped: {why}"));
    }
    report.sealed = incumbent_sealed.map(|s| s.outcome);
    report.last = Some(incumbent);
    report.archive = Some(archive);
    Ok(report)
}

/// The archive a profile's earlier runs left, with the digest of the tasks
/// its counts are from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ArchiveFile {
    /// The digest of the tasks its variants were tried on.
    pub train: String,
    /// The variants.
    pub archive: Archive<Variant>,
}

/// Where a profile's archive is kept.
pub fn archive_path(profile: &str) -> PathBuf {
    dir().join(format!("{profile}-archive.json"))
}

/// The archive earlier runs left for tasks of digest `train`, if any.
pub fn archive_for(profile: &str, train: &str) -> Option<Archive<Variant>> {
    let text = std::fs::read_to_string(archive_path(profile)).ok()?;
    let kept: ArchiveFile = serde_json::from_str(&text).ok()?;
    (kept.train == train).then_some(kept.archive)
}

/// Keeps `archive`, of variants tried on tasks of digest `train`.
pub fn keep_archive(
    profile: &str,
    train: &str,
    archive: &Archive<Variant>,
) -> Result<(), CliError> {
    crate::profiles::write_json(
        &archive_path(profile),
        &ArchiveFile {
            train: train.to_string(),
            archive: archive.clone(),
        },
    )
}

/// A problem an improver is graded on: tasks to choose the agent's
/// changes on, and tasks to confirm them on.
#[derive(Clone, Debug)]
pub struct MetaProblem {
    /// Its name, for what is said of it.
    pub name: String,
    /// Tasks the agent's changes are chosen on.
    pub train: TaskSet,
    /// Their digest.
    pub train_digest: Hash32,
    /// Tasks they are confirmed on, which the agent's proposer never sees.
    pub held: TaskSet,
    /// Their digest.
    pub held_digest: Hash32,
}

impl MetaProblem {
    /// A problem from two sets' exact bytes.
    pub fn read(name: &str, train: &[u8], held: &[u8]) -> Result<MetaProblem, CliError> {
        let problem = MetaProblem {
            name: name.to_string(),
            train: TaskSet::parse(train).map_err(CliError::Usage)?,
            train_digest: sion_agent::evals::digest(train),
            held: TaskSet::parse(held).map_err(CliError::Usage)?,
            held_digest: sion_agent::evals::digest(held),
        };
        if problem.train_digest == problem.held_digest {
            return Err(CliError::Usage(format!(
                "problem {name}: its held-out tasks are the tasks to choose on"
            )));
        }
        Ok(problem)
    }
}

/// How an improver is graded: the loop it runs on each problem.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MetaSettings {
    /// The loop's settings on each problem.
    pub inner: Settings,
    /// The confirmations the loop on each problem may make.
    pub inner_confirmations: u32,
    /// Rounds of changes to the improver, at most.
    pub rounds: usize,
    /// The confirmations of changes to the improver this allows, each held
    /// to 5% split among them all.
    pub confirmations: u32,
}

/// How an improver did on some problems.
#[derive(Clone, Debug, Default)]
pub struct Graded {
    /// How the agent did on the problems' held-out tasks before the loop,
    /// all problems together.
    pub before: Outcome,
    /// How it did after.
    pub after: Outcome,
    /// What happened on each: its name, before, after, the changes kept,
    /// and what the loop said.
    pub each: Vec<(String, Outcome, Outcome, usize, Vec<String>)>,
    /// The tokens it took.
    pub tokens: u64,
}

fn add(a: Outcome, b: Outcome) -> Outcome {
    Outcome {
        passed: a.passed + b.passed,
        total: a.total + b.total,
        tokens: a.tokens + b.tokens,
    }
}

/// Grades `improver` on `problems`: on each, the loop runs on `profile`'s
/// agent as it is, with a fresh proposer from `proposer`, and the agent's
/// held-out runs are counted before and after (the confirmed change's, or
/// the agent's own if none was). `before` keeps each problem's count for
/// the agent as it is, so that it is run once.
#[allow(clippy::too_many_arguments)]
pub fn grade(
    profile: &Profile,
    improver: &Improver,
    problems: &[MetaProblem],
    proposer: &dyn Fn() -> Result<Box<dyn Model>, CliError>,
    settings: &MetaSettings,
    before: &mut std::collections::BTreeMap<String, Outcome>,
    stopped: &dyn Fn() -> bool,
    now: &dyn Fn() -> u64,
) -> Result<Graded, CliError> {
    let mut graded = Graded::default();
    let start = Variant::of(profile);
    for problem in problems {
        let base = match before.get(&problem.name) {
            Some(outcome) => *outcome,
            None => {
                let scored = score(
                    profile,
                    &start,
                    &problem.held,
                    problem.held_digest,
                    settings.inner.runs,
                    settings.inner.budget_tokens,
                )?;
                graded.tokens += scored.outcome.tokens;
                before.insert(problem.name.clone(), scored.outcome);
                scored.outcome
            }
        };
        let model = proposer()?;
        let budget = settings.inner_confirmations.max(1);
        let mut used = 0u32;
        let mut spend = || -> Result<(u32, u32), CliError> {
            if used >= budget {
                return Err(CliError::Usage(format!(
                    "its {budget} confirmations are spent"
                )));
            }
            used += 1;
            Ok((used, budget))
        };
        let mut kept = |_: &mut Step| -> Result<Vec<String>, CliError> { Ok(Vec::new()) };
        // The improver's problems hold fixed confirmations.
        let mut ask = |_: f64, _: f64| -> Result<Asked, CliError> {
            Err(CliError::Usage(
                "an improver's problem is not a reusable holdout".into(),
            ))
        };
        let mut sinks = Sinks {
            spend: &mut spend,
            ask: &mut ask,
            kept: &mut kept,
            stopped,
            now,
        };
        let inner = Problem {
            train: &problem.train,
            train_digest: problem.train_digest,
            sealed: &problem.held,
            sealed_digest: problem.held_digest,
            z: sion_agent::compare::z_for(crate::sealed::ALPHA / f64::from(budget)),
            reusable: false,
            practice: None,
            challenger: None,
        };
        let report = improve(
            profile,
            &inner,
            model.as_ref(),
            improver,
            &settings.inner,
            None,
            &mut sinks,
        )?;
        graded.tokens += report.tokens;
        let after = match (report.kept.is_empty(), report.sealed) {
            (false, Some(outcome)) => outcome,
            _ => base,
        };
        graded.before = add(graded.before, base);
        graded.after = add(graded.after, after);
        graded.each.push((
            problem.name.clone(),
            base,
            after,
            report.kept.len(),
            report.lines,
        ));
    }
    Ok(graded)
}

/// A change to the improver that was kept, with the evidence for it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImproverStep {
    /// Its id.
    pub id: String,
    /// The improver before it.
    pub from: Improver,
    /// The improver after it.
    pub to: Improver,
    /// What it changes, in a line.
    pub change: String,
    /// Why it was proposed.
    pub why: String,
    /// The digest of the improver that proposed it: the one it replaces,
    /// improving itself.
    pub proposed_by: String,
    /// The agent's held-out runs on the problems changes were chosen on,
    /// with the improver before and after.
    pub train_before: Outcome,
    /// After.
    pub train_after: Outcome,
    /// Newcombe's 95% interval for the difference.
    pub screen: (f64, f64),
    /// The same, on the problems held out.
    pub held_before: Outcome,
    /// After.
    pub held_after: Outcome,
    /// The interval there, at the corrected level.
    pub confirm: (f64, f64),
    /// The z of that level.
    pub z: f64,
    /// When it was kept (seconds since the epoch).
    pub at: u64,
}

/// A change to the improver the improver proposed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImproverProposal {
    /// The improver it would make.
    pub to: Improver,
    /// Why.
    pub why: String,
}

/// The tool through which an improver proposes changes to itself.
struct ProposeImprover {
    current: Improver,
    found: Arc<Mutex<Vec<ImproverProposal>>>,
    most: usize,
}

impl Tool for ProposeImprover {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "propose_improver".into(),
            description: "Propose a change to the proposer: `instructions` (the full new text), \
                          `proposals`, `failures`, `history`, any of them, and `why`."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "instructions": {"type": "string"},
                    "proposals": {"type": "integer"},
                    "failures": {"type": "integer"},
                    "history": {"type": "boolean"},
                    "why": {"type": "string"}
                },
                "required": ["why"]
            }),
        }
    }

    fn effect(&self) -> Effect {
        Effect::Write
    }

    fn call(&self, input: &Value, _key: &str) -> ToolOutcome {
        let mut to = self.current.clone();
        let mut any = false;
        if let Some(text) = input["instructions"].as_str() {
            to.instructions = text.trim().to_string();
            any = true;
        }
        let number = |key: &str| {
            input[key]
                .as_u64()
                .map(|n| n.min(u64::from(u32::MAX)) as u32)
        };
        if let Some(n) = number("proposals") {
            to.proposals = n;
            any = true;
        }
        if let Some(n) = number("failures") {
            to.failures = n;
            any = true;
        }
        if let Some(history) = input["history"].as_bool() {
            to.history = history;
            any = true;
        }
        if !any {
            return ToolOutcome::error(
                "give at least one of instructions, proposals, failures, history",
            );
        }
        if let Err(why) = to.check() {
            return ToolOutcome::error(why);
        }
        let mut found = self.found.lock().expect("the proposals' lock");
        if found.len() >= self.most {
            return ToolOutcome::error(format!(
                "that is all {} changes this round takes",
                self.most
            ));
        }
        found.push(ImproverProposal {
            to,
            why: input["why"].as_str().unwrap_or_default().to_string(),
        });
        ToolOutcome::ok("proposed: it will be graded on the problems, then confirmed on problems you are not shown")
    }
}

/// What an improver is told about itself when it improves itself.
fn meta_prompt(improver: &Improver, graded: &Graded, history: &[String]) -> String {
    let mut text = format!(
        "The system to improve is the proposer that writes changes to an AI agent: its \
         instructions (your own instructions, as you read them now) and three settings. It is \
         judged by how much the agent improves, on tasks it was never shown, when it writes the \
         agent's changes. Fixed: how changes are tried and confirmed, the limits on them, and the \
         agent itself. You may change, with propose_improver: `instructions` (the full new text, \
         at most {MAX_LEARNED} bytes), `proposals` (changes it proposes a round, 1 to 8), \
         `failures` (failed tasks it is shown, 1 to 20) and `history` (whether it is shown what \
         was tried before).\n\n\
         Its instructions now:\n{}\n\nIts settings now: proposals {}, failures {}, history {}.\n\n\
         How it did: on {} problems, the agent's runs of their held-out tasks went from {}/{} to {}/{} passed.\n\n\
         Problem by problem:\n",
        improver.instructions.trim(),
        improver.proposals,
        improver.failures,
        improver.history,
        graded.each.len(),
        graded.before.passed,
        graded.before.total,
        graded.after.passed,
        graded.after.total,
    );
    for (name, before, after, kept, lines) in &graded.each {
        text.push_str(&format!(
            "- {name}: {}/{} to {}/{}, {kept} changes kept. What happened:\n",
            before.passed, before.total, after.passed, after.total
        ));
        for line in lines.iter().take(24) {
            text.push_str(&format!("    {}\n", clip(line, 400)));
        }
    }
    if improver.history && !history.is_empty() {
        text.push_str("\nWhat was tried before, and how it went:\n");
        for line in history.iter().rev().take(20).rev() {
            text.push_str(&format!("- {line}\n"));
        }
    }
    text.push_str(&format!("\nPropose up to {} changes.", improver.proposals));
    text
}

/// Asks `model`, under `improver`'s own instructions, for changes to
/// `improver`: the improver improving itself.
fn propose_improver(
    model: &dyn Model,
    improver: &Improver,
    graded: &Graded,
    history: &[String],
    round: usize,
) -> (Vec<ImproverProposal>, u64) {
    let found = Arc::new(Mutex::new(Vec::new()));
    let mut tools = Toolbox::new();
    tools.add(ProposeImprover {
        current: improver.clone(),
        found: found.clone(),
        most: improver.proposals as usize,
    });
    let limits = Limits {
        turns: improver.proposals + 2,
        turns_per_progress: 0,
        max_turns: improver.proposals + 2,
        ..Limits::default()
    };
    let runtime = Runtime {
        models: vec![model],
        tools: &tools,
        policy: Policy {
            autonomy: Autonomy::L3,
            ..Policy::default()
        },
        limits: limits.clone(),
        system: improver.instructions.clone(),
        now: &|| 0,
        stopped: &|| false,
        watch: None,
        steer: None,
        checkpoint: None,
        verify: None,
        code: None,
        hooks: None,
        extras: Default::default(),
    };
    let prompt = meta_prompt(improver, graded, history);
    let mut state = State::start(&format!("improve-improver-{round}"), &prompt, &limits);
    runtime.run(&mut state);
    let proposals = std::mem::take(&mut *found.lock().expect("the proposals' lock"));
    (proposals, state.usage.input + state.usage.output)
}

/// What an improver changes of another, in a line.
pub fn improver_change(to: &Improver, from: &Improver) -> String {
    let mut parts = Vec::new();
    if to.instructions.trim() != from.instructions.trim() {
        parts.push(format!(
            "its instructions, {}",
            changed_text(&from.instructions, &to.instructions)
        ));
    }
    if to.proposals != from.proposals {
        parts.push(format!("{} proposals a round", to.proposals));
    }
    if to.failures != from.failures {
        parts.push(format!("{} failures shown", to.failures));
    }
    if to.history != from.history {
        parts.push(format!(
            "history {}",
            if to.history { "shown" } else { "not shown" }
        ));
    }
    if parts.is_empty() {
        "changes nothing".into()
    } else {
        parts.join(", ")
    }
}

/// What improving the improver came to.
#[derive(Clone, Debug, Default)]
pub struct MetaReport {
    /// What it did, a line each.
    pub lines: Vec<String>,
    /// The changes to the improver it kept, in order.
    pub kept: Vec<ImproverStep>,
    /// The improver it left.
    pub last: Improver,
    /// The tokens it took.
    pub tokens: u64,
}

/// Improves `profile`'s improver: each round, the improver as it is is
/// graded on `problems`, and proposes changes to itself (under its own
/// instructions, through `meta`); each change is graded on `problems`
/// against it and kept only when the agent improves more with it, beyond
/// chance, there and then on `held_problems`, which no proposer is shown,
/// at the level `settings.confirmations` corrects for. A kept change is
/// the improver the next round runs, and proposes, with: the improver that
/// improves the agent is the one that improves itself. `kept` is told of
/// each.
#[allow(clippy::too_many_arguments)]
pub fn improve_improver(
    profile: &Profile,
    start: &Improver,
    problems: &[MetaProblem],
    held_problems: &[MetaProblem],
    proposer: &dyn Fn() -> Result<Box<dyn Model>, CliError>,
    meta: &dyn Fn() -> Result<Box<dyn Model>, CliError>,
    settings: &MetaSettings,
    stopped: &dyn Fn() -> bool,
    now: &dyn Fn() -> u64,
    kept: &mut dyn FnMut(&ImproverStep) -> Result<Vec<String>, CliError>,
) -> Result<MetaReport, CliError> {
    start.check().map_err(CliError::Usage)?;
    if problems.is_empty() || held_problems.is_empty() {
        return Err(CliError::Usage(
            "an improver is graded on problems and confirmed on others held out: give both".into(),
        ));
    }
    let held_cases: Vec<sion_agent::evals::Case> = held_problems
        .iter()
        .flat_map(|p| p.train.cases.iter().chain(&p.held.cases).cloned())
        .collect();
    let z =
        sion_agent::compare::z_for(crate::sealed::ALPHA / f64::from(settings.confirmations.max(1)));
    let mut report = MetaReport {
        last: start.clone(),
        ..MetaReport::default()
    };
    let mut before_train = std::collections::BTreeMap::new();
    let mut before_held = std::collections::BTreeMap::new();
    let mut confirmations = 0u32;
    let mut tried: BTreeSet<Hash32> = BTreeSet::from([start.digest()]);
    let mut history: Vec<String> = Vec::new();
    let mut current = start.clone();
    let mut graded = grade(
        profile,
        &current,
        problems,
        proposer,
        settings,
        &mut before_train,
        stopped,
        now,
    )?;
    report.tokens += graded.tokens;
    report.lines.push(format!(
        "the improver as it is: the agent's held-out runs went from {}/{} to {}/{} on the problems to choose on",
        graded.before.passed, graded.before.total, graded.after.passed, graded.after.total
    ));
    let mut held_current: Option<Graded> = None;
    'rounds: for round in 0..settings.rounds {
        if stopped() {
            report.lines.push("stopped: the kill switch is on".into());
            break;
        }
        let proposed_by = current.digest().to_hex()[..16].to_string();
        let model = meta()?;
        let (proposals, used) =
            propose_improver(model.as_ref(), &current, &graded, &history, round);
        report.tokens += used;
        report.lines.push(format!(
            "round {}: the improver {proposed_by} proposed {} to itself",
            round + 1,
            match proposals.len() {
                1 => "1 change".to_string(),
                n => format!("{n} changes"),
            }
        ));
        for proposal in proposals {
            if stopped() {
                report.lines.push("stopped: the kill switch is on".into());
                break 'rounds;
            }
            let change = improver_change(&proposal.to, &current);
            if !tried.insert(proposal.to.digest()) {
                report
                    .lines
                    .push(format!("  turned down: {change}: tried already"));
                history.push(format!("{change}: tried already"));
                continue;
            }
            if let Some((case, _)) = sion_agent::guard::leak(&proposal.to.instructions, &held_cases)
            {
                report.lines.push(format!(
                    "  turned down: {change}: it shares a run of words with held-out problem task {case}"
                ));
                history.push(format!(
                    "{change}: turned down: it repeats a task it should not know"
                ));
                continue;
            }
            let tried_on = grade(
                profile,
                &proposal.to,
                problems,
                proposer,
                settings,
                &mut before_train,
                stopped,
                now,
            )?;
            report.tokens += tried_on.tokens;
            let screen = newcombe(
                (tried_on.after.passed, tried_on.after.total),
                (graded.after.passed, graded.after.total),
            );
            if screen.0 <= 0.0 {
                report.lines.push(format!(
                    "  tried {change}: the agent ended at {}/{} against {}/{} (95% interval {:+.0}% to {:+.0}%): not better beyond chance",
                    tried_on.after.passed, tried_on.after.total, graded.after.passed, graded.after.total,
                    screen.0 * 100.0, screen.1 * 100.0
                ));
                history.push(format!(
                    "{change}: the agent ended at {}/{} against {}/{}: not better beyond chance",
                    tried_on.after.passed,
                    tried_on.after.total,
                    graded.after.passed,
                    graded.after.total
                ));
                continue;
            }
            if confirmations >= settings.confirmations {
                report.lines.push(format!(
                    "stopped: the {} confirmations are spent",
                    settings.confirmations
                ));
                break 'rounds;
            }
            let base = match &held_current {
                Some(graded) => graded.clone(),
                None => {
                    let g = grade(
                        profile,
                        &current,
                        held_problems,
                        proposer,
                        settings,
                        &mut before_held,
                        stopped,
                        now,
                    )?;
                    report.tokens += g.tokens;
                    held_current = Some(g.clone());
                    g
                }
            };
            confirmations += 1;
            let held = grade(
                profile,
                &proposal.to,
                held_problems,
                proposer,
                settings,
                &mut before_held,
                stopped,
                now,
            )?;
            report.tokens += held.tokens;
            let confirm = newcombe_with(
                (held.after.passed, held.after.total),
                (base.after.passed, base.after.total),
                z,
            );
            if confirm.0 <= 0.0 {
                report.lines.push(format!(
                    "  tried {change}: better on the problems to choose on ({}/{} against {}/{}), but not confirmed on those held out: {}/{} against {}/{} (interval {:+.0}% to {:+.0}%)",
                    tried_on.after.passed, tried_on.after.total, graded.after.passed, graded.after.total,
                    held.after.passed, held.after.total, base.after.passed, base.after.total,
                    confirm.0 * 100.0, confirm.1 * 100.0
                ));
                history.push(format!(
                    "{change}: better on the problems shown, but not confirmed on those held out"
                ));
                continue;
            }
            let mut step = ImproverStep {
                id: String::new(),
                from: current.clone(),
                to: proposal.to.clone(),
                change: change.clone(),
                why: proposal.why.clone(),
                proposed_by: proposed_by.clone(),
                train_before: graded.after,
                train_after: tried_on.after,
                screen,
                held_before: base.after,
                held_after: held.after,
                confirm,
                z,
                at: now(),
            };
            let bytes = serde_json::to_vec(&step).unwrap_or_default();
            step.id = sion_core::hash::hash_domain("sion-cli/improve/improver-step/v1", &bytes)
                .to_hex()[..16]
                .to_string();
            let lines = kept(&step)?;
            report.lines.push(format!(
                "  kept {change}: the agent ended at {}/{} against {}/{} on the problems to choose on, and {}/{} against {}/{} on those held out (interval at the corrected level {:+.0}% to {:+.0}%), as step {}",
                tried_on.after.passed, tried_on.after.total, graded.after.passed, graded.after.total,
                held.after.passed, held.after.total, base.after.passed, base.after.total,
                confirm.0 * 100.0, confirm.1 * 100.0, step.id
            ));
            report
                .lines
                .extend(lines.into_iter().map(|l| format!("  {l}")));
            history.push(format!("{change}: kept"));
            current = proposal.to;
            graded = tried_on;
            held_current = Some(held);
            report.kept.push(step);
            // The improver kept is the one that proposes next.
            continue 'rounds;
        }
    }
    if report.kept.len() > 1 {
        report.lines.push(format!(
            "an improver kept proposed the next change kept {} times: it improved itself, and the improvement went on",
            report.kept.len() - 1
        ));
    }
    if report.kept.is_empty() {
        report
            .lines
            .push("nothing was confirmed better: the improver is left as it is".into());
    }
    report.last = current;
    Ok(report)
}

/// Records `step`, a change to `profile`'s improver kept, and with
/// `make_it_so` makes it the profile's improver.
pub fn record_improver(
    profile: &str,
    step: &ImproverStep,
    make_it_so: bool,
    now: u64,
) -> Result<Vec<String>, CliError> {
    append(profile, &Record::ImproverKept(step.clone()))?;
    if !make_it_so {
        return Ok(Vec::new());
    }
    let before = improver(profile)?;
    crate::profiles::write_json(&improver_path(profile), &step.to)?;
    append(
        profile,
        &Record::ImproverApplied {
            step: step.id.clone(),
            before,
            at: now,
        },
    )?;
    Ok(vec![format!(
        "profile {profile}'s improver is now the one step {} kept",
        step.id
    )])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_the_agent_learned_follows_its_operators_words_under_its_own_heading() {
        assert_eq!(with_learned(None, "  "), None);
        assert_eq!(
            with_learned(Some("Be brief."), ""),
            Some("Be brief.".into())
        );
        let both = with_learned(Some("Be brief."), "Check the payee first.").unwrap();
        assert!(both.starts_with("Be brief.\n\n"));
        assert!(both.ends_with(&format!("{LEARNED}\nCheck the payee first.")));
        assert!(with_learned(None, "x").unwrap().starts_with(LEARNED));
    }

    #[test]
    fn a_variant_says_what_it_changes_and_digests_its_words_and_models() {
        let a = Variant {
            learned: String::new(),
            models: vec!["m1".into()],
        };
        let b = Variant {
            learned: "Check the payee.".into(),
            models: vec!["m2".into(), "m1".into()],
        };
        assert_eq!(a.change_from(&a), "changes nothing");
        let said = b.change_from(&a);
        assert!(
            said.contains("learns \"Check the payee.\"") && said.contains("runs on m2, m1"),
            "{said}"
        );
        assert_ne!(a.digest(), b.digest());
        let spaced = Variant {
            learned: "  Check the payee.  ".into(),
            ..b.clone()
        };
        assert_eq!(spaced.digest(), b.digest());
    }

    #[test]
    fn the_proposer_tool_takes_at_most_its_rounds_share() {
        let found = Arc::new(Mutex::new(Vec::new()));
        let tool = ProposeChange {
            found: found.clone(),
            most: 1,
        };
        assert!(tool.call(&json!({"why": "x"}), "k").is_error);
        assert!(
            !tool
                .call(&json!({"learned": "a", "why": "x"}), "k")
                .is_error
        );
        assert!(
            tool.call(&json!({"models": ["m"], "why": "y"}), "k")
                .is_error
        );
        assert_eq!(found.lock().unwrap().len(), 1);
    }

    #[test]
    fn a_change_of_words_is_said_by_what_it_adds_drops_or_replaces() {
        assert_eq!(
            changed_text("Be brief.", "Be brief. Check the payee."),
            "adding \"Check the payee.\""
        );
        assert_eq!(
            changed_text("Be brief. Check the payee.", "Be brief."),
            "dropping \"Check the payee.\""
        );
        assert_eq!(
            changed_text("Pay acme.", "Pay bob."),
            "\"bob\" in place of \"acme\""
        );
        assert_eq!(changed_text("", "New."), "\"New.\"");
        assert_eq!(changed_text("Same.", " Same. "), "the same words");
    }

    #[test]
    fn an_improver_keeps_to_its_limits() {
        assert_eq!(Improver::default().check(), Ok(()));
        let none = Improver {
            proposals: 0,
            ..Improver::default()
        };
        assert!(none.check().is_err());
        let different = Improver {
            history: false,
            ..Improver::default()
        };
        assert_ne!(different.digest(), Improver::default().digest());
    }
}
