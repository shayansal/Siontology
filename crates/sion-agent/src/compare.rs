//! Comparisons of ways of working on a set of tasks: one agent, one agent
//! several times with its answers put to a vote (self-consistency), and
//! graphs of agents, each on the same token budget for a case, each case
//! run several times, with the cases' stand-in tools and checks
//! ([`crate::evals`]).
//!
//! One agent is the default. A graph earns a task family's traffic only by
//! beating both baselines on the same cases and the same budget: its pass
//! rate above each by Newcombe's 95% interval for a difference of
//! proportions, whose lower end must be above zero ([`Comparison::earned`]).
//! That is the test under which most measured gains of several agents
//! shrink, which is why it is the bar.
//!
//! What a comparison reports is what its runs did: pass rates with 95%
//! Wilson intervals, the passes of each repeat, and the tokens the models
//! reported. The intervals treat every run as independent, though repeats
//! of a case are not, so they are narrower than they would be over more
//! cases. With a scripted model, the figures are the script's.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sion_core::hash::Hash32;

use crate::evals::{calls_of, check, check_cases, Case, Check, FixtureTool, MAX_CASES};
use crate::graph::{Answer, Executor, Graph, GraphRun, Node, Standing};
use crate::judge::{self, Counts};
use crate::model::Model;
use crate::policy::{Autonomy, Policy};
use crate::session::{Ended, Limits, Runtime, State};
use crate::tools::Toolbox;

/// The system prompt of a comparison's runs, unless the set gives its own.
pub const SYSTEM: &str = "You are an AI agent. Use the tools to do the task. When it is done, \
                          reply with the answer the task asks for, and nothing else.";

/// The most samples a vote takes.
pub const MAX_SAMPLES: u32 = 9;

/// The most times a case is run by each way of working.
pub const MAX_REPEATS: u32 = 20;

/// Fresh models for a run, in order of preference, from any thread.
pub type Models<'a> = &'a (dyn Fn() -> Result<Vec<Box<dyn Model>>, String> + Sync);

/// A set of tasks to compare ways of working on: cases as an evaluation
/// set has them, for no skill in particular.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TaskSet {
    /// Its name: lowercase letters, digits and hyphens.
    pub name: String,
    /// What its tasks have in common, in words.
    #[serde(default)]
    pub about: String,
    /// The system prompt of its runs (default: [`SYSTEM`]).
    #[serde(default)]
    pub system: Option<String>,
    /// The cases.
    pub cases: Vec<Case>,
}

impl TaskSet {
    /// Reads and checks a set: a name, and 1 to [`MAX_CASES`] cases as an
    /// evaluation set's are checked.
    pub fn parse(bytes: &[u8]) -> Result<TaskSet, String> {
        let set: TaskSet =
            serde_json::from_slice(bytes).map_err(|e| format!("not a set of tasks: {e}"))?;
        let named = !set.name.is_empty()
            && set.name.len() <= 64
            && set
                .name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        if !named {
            return Err(format!(
                "{:?} is not a set's name: lowercase letters, digits and hyphens",
                set.name
            ));
        }
        if set.cases.is_empty() || set.cases.len() > MAX_CASES {
            return Err(format!("a set has 1 to {MAX_CASES} cases"));
        }
        check_cases(&set.cases)?;
        Ok(set)
    }

    /// The system prompt of its runs.
    pub fn system(&self) -> &str {
        self.system.as_deref().unwrap_or(SYSTEM)
    }
}

/// A way of working on a case.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "arm", rename_all = "snake_case")]
pub enum Arm {
    /// One agent, in one session, with the whole budget.
    Single,
    /// One agent `samples` times, each with its share of the budget, the
    /// answer given most often (case and spacing aside) winning; a tie goes
    /// to the earliest.
    Vote {
        /// How many sessions.
        samples: u32,
    },
    /// A graph of agents, its nodes sharing the budget: the nodes of a wave
    /// share what is left equally.
    Graph {
        /// The graph.
        graph: Graph,
    },
}

impl Arm {
    /// Its name in a report: `single`, `vote-<samples>`, or the graph's.
    pub fn name(&self) -> String {
        match self {
            Arm::Single => "single".into(),
            Arm::Vote { samples } => format!("vote-{samples}"),
            Arm::Graph { graph } => graph.name.clone(),
        }
    }

    /// Whether it is a baseline: one agent, alone or voting.
    pub fn baseline(&self) -> bool {
        !matches!(self, Arm::Graph { .. })
    }
}

/// One run of a case by one way of working.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Trial {
    /// The case.
    pub case: String,
    /// Which repeat, from 1.
    pub repeat: u32,
    /// Whether every check held.
    pub passed: bool,
    /// The checks that did not hold, in words.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failures: Vec<String>,
    /// The tokens its models reported.
    pub tokens: u64,
    /// Its answer, if it gave one.
    #[serde(default)]
    pub answer: String,
    /// What a judge said of its steps, if one was asked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub judged: Option<Counts>,
}

/// A way of working and its runs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ArmResult {
    /// The way of working.
    pub arm: Arm,
    /// Its runs, case by case, repeat by repeat.
    pub trials: Vec<Trial>,
}

impl ArmResult {
    /// Runs that passed.
    pub fn passed(&self) -> usize {
        self.trials.iter().filter(|t| t.passed).count()
    }

    /// Runs in all.
    pub fn total(&self) -> usize {
        self.trials.len()
    }

    /// The share that passed.
    pub fn rate(&self) -> f64 {
        if self.trials.is_empty() {
            0.0
        } else {
            self.passed() as f64 / self.total() as f64
        }
    }

    /// The tokens of every run.
    pub fn tokens(&self) -> u64 {
        self.trials.iter().map(|t| t.tokens).sum()
    }

    /// How many cases passed in each repeat, in order.
    pub fn per_repeat(&self) -> Vec<usize> {
        let repeats = self.trials.iter().map(|t| t.repeat).max().unwrap_or(0);
        (1..=repeats)
            .map(|r| {
                self.trials
                    .iter()
                    .filter(|t| t.repeat == r && t.passed)
                    .count()
            })
            .collect()
    }

    /// What a judge said of every run's steps, if it was asked.
    pub fn judged(&self) -> Option<Counts> {
        let mut all = Counts::default();
        let mut any = false;
        for counts in self.trials.iter().filter_map(|t| t.judged) {
            all.add(counts);
            any = true;
        }
        any.then_some(all)
    }
}

/// The 95% Wilson score interval for `passed` of `total`.
pub fn wilson(passed: usize, total: usize) -> (f64, f64) {
    if total == 0 {
        return (0.0, 1.0);
    }
    let z = 1.959_964;
    let n = total as f64;
    let p = passed as f64 / n;
    let denominator = 1.0 + z * z / n;
    let centre = (p + z * z / (2.0 * n)) / denominator;
    let half = z * (p * (1.0 - p) / n + z * z / (4.0 * n * n)).sqrt() / denominator;
    ((centre - half).max(0.0), (centre + half).min(1.0))
}

/// Newcombe's 95% interval (his method 10, from two Wilson intervals) for
/// the difference of two pass rates, `a`'s minus `b`'s.
pub fn newcombe(a: (usize, usize), b: (usize, usize)) -> (f64, f64) {
    let rate = |(k, n): (usize, usize)| if n == 0 { 0.0 } else { k as f64 / n as f64 };
    let (pa, pb) = (rate(a), rate(b));
    let (la, ua) = wilson(a.0, a.1);
    let (lb, ub) = wilson(b.0, b.1);
    let d = pa - pb;
    (
        d - ((pa - la).powi(2) + (ub - pb).powi(2)).sqrt(),
        d + ((ua - pa).powi(2) + (pb - lb).powi(2)).sqrt(),
    )
}

/// What a comparison came to.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Comparison {
    /// The set's name.
    pub set: String,
    /// The digest of the set's exact bytes.
    pub digest: Hash32,
    /// The tokens each way of working had for a case.
    pub budget_tokens: u64,
    /// How many times each case was run by each.
    pub repeats: u32,
    /// The judge's agreement with labelled steps it was not shown, if one
    /// was asked: (agreed, of).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub judge_calibration: Option<(usize, usize)>,
    /// Each way of working, and its runs.
    pub arms: Vec<ArmResult>,
}

impl Comparison {
    /// The graphs that beat every baseline compared (one agent, and one
    /// agent voting): the lower end of Newcombe's interval for their pass
    /// rate minus each baseline's is above zero. None, unless both
    /// baselines were run.
    pub fn earned(&self) -> Vec<String> {
        let baselines: Vec<&ArmResult> = self.arms.iter().filter(|a| a.arm.baseline()).collect();
        let both = baselines.iter().any(|a| a.arm == Arm::Single)
            && baselines.iter().any(|a| matches!(a.arm, Arm::Vote { .. }));
        if !both {
            return Vec::new();
        }
        self.arms
            .iter()
            .filter(|a| !a.arm.baseline())
            .filter(|graph| {
                baselines.iter().all(|base| {
                    newcombe(
                        (graph.passed(), graph.total()),
                        (base.passed(), base.total()),
                    )
                    .0 > 0.0
                })
            })
            .map(|a| a.arm.name())
            .collect()
    }

    /// The report: a line per way of working, how it compares with one
    /// agent, and what earned the traffic.
    pub fn lines(&self) -> Vec<String> {
        let cases = self
            .arms
            .first()
            .map(|a| a.total() / self.repeats.max(1) as usize)
            .unwrap_or(0);
        let mut lines = vec![format!(
            "set {}: {cases} case{}, each run {} times by each way of working, on {} tokens a case",
            self.set,
            if cases == 1 { "" } else { "s" },
            self.repeats,
            self.budget_tokens
        )];
        if let Some((agreed, of)) = self.judge_calibration {
            lines.push(format!(
                "judge: {agreed} of {of} labelled steps it was not shown judged as labelled"
            ));
        }
        let single = self.arms.iter().find(|a| a.arm == Arm::Single);
        for arm in &self.arms {
            let (low, high) = wilson(arm.passed(), arm.total());
            let mut line = format!(
                "{:<14} {:>3}/{:<3} passed  {:>5.1}% [{:.1}–{:.1}]  repeats {:?}  {} tokens a run",
                arm.arm.name(),
                arm.passed(),
                arm.total(),
                100.0 * arm.rate(),
                100.0 * low,
                100.0 * high,
                arm.per_repeat(),
                arm.tokens() / arm.total().max(1) as u64
            );
            if let (Some(single), false) = (single, arm.arm == Arm::Single) {
                let (low, high) = newcombe(
                    (arm.passed(), arm.total()),
                    (single.passed(), single.total()),
                );
                line.push_str(&format!(
                    "  vs single {:+.1} points [{:+.1}, {:+.1}]",
                    100.0 * (arm.rate() - single.rate()),
                    100.0 * low,
                    100.0 * high
                ));
            }
            if let Some(counts) = arm.judged() {
                line.push_str(&format!(
                    "  steps: {} of {} moved it on, {} harmful",
                    counts.progress,
                    counts.steps(),
                    counts.harmful
                ));
            }
            lines.push(line);
        }
        let earned = self.earned();
        lines.push(if earned.is_empty() {
            "earned: none; one agent stays the default".into()
        } else {
            format!(
                "earned: {}, above both baselines by Newcombe's 95% interval",
                earned.join(", ")
            )
        });
        lines
    }
}

/// Compares `arms` on every case of `set` (whose digest is `digest`):
/// each case run `repeats` times by each, each run with `budget_tokens`
/// for the case, with fresh models from `models`. With `judging`, a judge
/// reads each run's steps.
#[allow(clippy::too_many_arguments)]
pub fn compare(
    set: &TaskSet,
    digest: Hash32,
    arms: &[Arm],
    models: Models<'_>,
    judging: Option<&dyn Model>,
    budget_tokens: u64,
    repeats: u32,
    limits: &Limits,
) -> Result<Comparison, String> {
    if !(1..=MAX_REPEATS).contains(&repeats) {
        return Err(format!("a case is run 1 to {MAX_REPEATS} times"));
    }
    if budget_tokens == 0 {
        return Err("a comparison needs a token budget for each case".into());
    }
    let mut names = BTreeSet::new();
    for arm in arms {
        match arm {
            Arm::Vote { samples } if !(2..=MAX_SAMPLES).contains(samples) => {
                return Err(format!("a vote takes 2 to {MAX_SAMPLES} samples"))
            }
            Arm::Graph { graph } => graph.check()?,
            _ => {}
        }
        if !names.insert(arm.name()) {
            return Err(format!("{} is compared twice", arm.name()));
        }
    }
    let judge_calibration = judging.map(judge::calibrate).transpose()?;
    let mut results: Vec<ArmResult> = arms
        .iter()
        .map(|arm| ArmResult {
            arm: arm.clone(),
            trials: Vec::new(),
        })
        .collect();
    for repeat in 1..=repeats {
        for case in &set.cases {
            for result in results.iter_mut() {
                let run = run_arm(
                    &result.arm,
                    case,
                    set.system(),
                    models,
                    budget_tokens,
                    limits,
                )?;
                let judged = match judging {
                    Some(model) => {
                        Some(Counts::from(&judge::judge(model, &case.task, &run.steps)?))
                    }
                    None => None,
                };
                result.trials.push(Trial {
                    case: case.id.clone(),
                    repeat,
                    passed: run.failures.is_empty(),
                    failures: run.failures,
                    tokens: run.tokens,
                    answer: run.answer,
                    judged,
                });
            }
        }
    }
    Ok(Comparison {
        set: set.name.clone(),
        digest,
        budget_tokens,
        repeats,
        judge_calibration,
        arms: results,
    })
}

/// What one run of a case came to.
struct Run {
    failures: Vec<String>,
    tokens: u64,
    answer: String,
    steps: Vec<String>,
}

fn run_arm(
    arm: &Arm,
    case: &Case,
    system: &str,
    models: Models<'_>,
    budget: u64,
    limits: &Limits,
) -> Result<Run, String> {
    match arm {
        Arm::Single => {
            let session = session(
                case, system, &case.task, None, false, models, budget, limits,
            )?;
            let calls = calls_of(&session.state);
            Ok(Run {
                failures: failures(case, &calls, &session.ended, None),
                tokens: session.tokens,
                answer: done_text(&session.ended),
                steps: judge::steps_of(&session.state, Some(&session.ended)),
            })
        }
        Arm::Vote { samples } => {
            let share = budget / u64::from(*samples);
            let mut calls = Vec::new();
            let mut steps = Vec::new();
            let mut tokens = 0;
            let mut answers = Vec::new();
            let mut first_ended = None;
            for sample in 1..=*samples {
                let ran = session(case, system, &case.task, None, false, models, share, limits)?;
                calls.extend(calls_of(&ran.state));
                steps.extend(
                    judge::steps_of(&ran.state, Some(&ran.ended))
                        .into_iter()
                        .map(|s| format!("(sample {sample}) {s}")),
                );
                tokens += ran.tokens;
                if let Ended::Done { text } = &ran.ended {
                    answers.push(text.clone());
                }
                if first_ended.is_none() {
                    first_ended = Some(ran.ended);
                }
            }
            let ended = match vote(&answers) {
                Some((text, _, _)) => Ended::Done { text },
                None => first_ended.unwrap_or(Ended::OutOfTurns),
            };
            Ok(Run {
                failures: failures(case, &calls, &ended, None),
                tokens,
                answer: done_text(&ended),
                steps,
            })
        }
        Arm::Graph { graph } => {
            let executor = Fixtures {
                case,
                system,
                models,
                limits,
                left: Mutex::new(budget),
                share: Mutex::new(budget),
                calls: Mutex::new(Vec::new()),
                steps: Mutex::new(Vec::new()),
                tokens: Mutex::new(0),
            };
            let mut run = GraphRun::start(graph, &case.task);
            let standing = run.run(graph, &executor, &mut |_| {});
            let calls = executor.calls.into_inner().unwrap_or_default();
            let (ended, stopped) = match &standing {
                Standing::Done(answer) => (
                    Ended::Done {
                        text: answer.text.clone(),
                    },
                    None,
                ),
                Standing::Stopped(why) => (Ended::OutOfTurns, Some(why.clone())),
                Standing::Going => (Ended::OutOfTurns, Some("it did not finish".into())),
            };
            let mut steps = executor.steps.into_inner().unwrap_or_default();
            if let Ended::Done { text } = &ended {
                steps.push(format!("answer: {}", text.trim()));
            }
            Ok(Run {
                failures: failures(case, &calls, &ended, stopped.as_deref()),
                tokens: executor.tokens.into_inner().unwrap_or_default(),
                answer: done_text(&ended),
                steps,
            })
        }
    }
}

/// One run of `case` by one agent with `system` as its system prompt and
/// `budget_tokens`: whether every check held, and those that did not.
pub fn trial(
    case: &Case,
    system: &str,
    models: Models<'_>,
    budget_tokens: u64,
    limits: &Limits,
) -> Result<(bool, Vec<String>), String> {
    let ran = session(
        case,
        system,
        &case.task,
        None,
        false,
        models,
        budget_tokens,
        limits,
    )?;
    let failures = failures(case, &calls_of(&ran.state), &ran.ended, None);
    Ok((failures.is_empty(), failures))
}

fn done_text(ended: &Ended) -> String {
    match ended {
        Ended::Done { text } => text.trim().to_string(),
        _ => String::new(),
    }
}

/// The checks of `case` that did not hold, a graph that stopped saying
/// why it did not finish.
fn failures(
    case: &Case,
    calls: &[(String, Value)],
    ended: &Ended,
    stopped: Option<&str>,
) -> Vec<String> {
    case.checks
        .iter()
        .filter_map(|c| match (c, stopped) {
            (Check::Done, Some(why)) => Some(format!("the graph stopped: {why}")),
            _ => check(c, calls, ended),
        })
        .collect()
}

/// The answer given most often, case and spacing aside, a tie going to the
/// earliest: its first wording, its votes, of how many answers.
pub fn vote(answers: &[String]) -> Option<(String, usize, usize)> {
    let key = |text: &str| {
        text.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
            .trim_end_matches('.')
            .to_string()
    };
    let mut counts: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for (i, answer) in answers.iter().enumerate() {
        let entry = counts.entry(key(answer)).or_insert((0, i));
        entry.0 += 1;
    }
    counts
        .values()
        .max_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)))
        .map(|&(votes, first)| (answers[first].trim().to_string(), votes, answers.len()))
}

/// A session of one agent on `prompt`, with the case's stand-in tools (only
/// those of `only`, if given) and `budget` tokens.
struct Session {
    state: State,
    ended: Ended,
    tokens: u64,
}

#[allow(clippy::too_many_arguments)]
fn session(
    case: &Case,
    system: &str,
    prompt: &str,
    only: Option<&BTreeSet<String>>,
    outside: bool,
    models: Models<'_>,
    budget: u64,
    limits: &Limits,
) -> Result<Session, String> {
    let mut tools = Toolbox::new();
    for fixture in &case.tools {
        if only.map_or(true, |only| only.contains(&fixture.name)) {
            tools.add(FixtureTool(fixture.clone()));
        }
    }
    let fresh = models()?;
    let policy = Policy {
        autonomy: Autonomy::L6,
        ..Policy::default()
    };
    let limits = Limits {
        budget_tokens: Some(budget),
        ..limits.clone()
    };
    let runtime = Runtime {
        models: fresh.iter().map(|m| m.as_ref()).collect(),
        tools: &tools,
        policy: policy.clone(),
        limits: limits.clone(),
        system: system.to_string(),
        now: &|| 0,
        stopped: &|| false,
        watch: None,
        steer: None,
        checkpoint: None,
        verify: None,
    };
    let mut state = State::start(&format!("compare-{}", case.id), prompt, &limits);
    if outside {
        state.standing.taint(&policy, 0);
    }
    let ended = runtime.run(&mut state);
    let tokens = state.usage.total();
    Ok(Session {
        state,
        ended,
        tokens,
    })
}

/// Runs a graph's nodes as sessions with a case's stand-in tools, sharing
/// a budget: the nodes of a wave share what is left equally.
struct Fixtures<'a> {
    case: &'a Case,
    system: &'a str,
    models: Models<'a>,
    limits: &'a Limits,
    left: Mutex<u64>,
    share: Mutex<u64>,
    calls: Mutex<Vec<(String, Value)>>,
    steps: Mutex<Vec<String>>,
    tokens: Mutex<u64>,
}

impl Executor for Fixtures<'_> {
    fn wave(&self, size: usize) {
        let left = *self.left.lock().unwrap_or_else(|e| e.into_inner());
        *self.share.lock().unwrap_or_else(|e| e.into_inner()) = left / size.max(1) as u64;
    }

    fn run(&self, node: &Node, prompt: &str, outside: bool) -> Result<Answer, String> {
        let share = *self.share.lock().unwrap_or_else(|e| e.into_inner());
        if share == 0 {
            return Err("the budget is spent".into());
        }
        let ran = session(
            self.case,
            self.system,
            prompt,
            node.tools.as_ref(),
            outside,
            self.models,
            share,
            self.limits,
        )?;
        {
            let mut left = self.left.lock().unwrap_or_else(|e| e.into_inner());
            *left = left.saturating_sub(ran.tokens);
        }
        *self.tokens.lock().unwrap_or_else(|e| e.into_inner()) += ran.tokens;
        self.calls
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .extend(calls_of(&ran.state));
        self.steps.lock().unwrap_or_else(|e| e.into_inner()).extend(
            judge::steps_of(&ran.state, None)
                .into_iter()
                .map(|s| format!("({}) {s}", node.id)),
        );
        match ran.ended {
            Ended::Done { text } => Ok(Answer {
                text,
                json: None,
                session: None,
                tokens: ran.tokens,
                outside: ran.state.standing.tainted_until.is_some(),
            }),
            other => Err(format!(
                "its session ended without an answer: {}",
                serde_json::to_value(&other)
                    .ok()
                    .and_then(|v| v["ended"].as_str().map(str::to_string))
                    .unwrap_or_default()
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripted::Scripted;
    use serde_json::json;

    const SET: &str = r#"{
      "name": "lookups",
      "about": "two things to look up, put together",
      "cases": [{
        "id": "plan-b",
        "task": "What do plan B and its support cost together?",
        "tools": [
          {"name": "price", "answers": [
            {"when": {"item": "plan b"}, "says": "12 USD"},
            {"when": {"item": "support"}, "says": "3 USD"}]}],
        "checks": [{"says": "15 USD"}, "done"]
      }]
    }"#;

    #[test]
    fn intervals_are_wilson_and_newcombe() {
        let (low, high) = wilson(8, 10);
        assert!(
            (low - 0.490).abs() < 0.001 && (high - 0.943).abs() < 0.001,
            "{low} {high}"
        );
        assert_eq!(wilson(0, 0), (0.0, 1.0));
        // 9/10 against 3/10: the difference's interval is above zero.
        let (low, high) = newcombe((9, 10), (3, 10));
        assert!(low > 0.0 && high < 1.0, "{low} {high}");
        // 6/10 against 5/10 is not.
        assert!(newcombe((6, 10), (5, 10)).0 < 0.0);
    }

    #[test]
    fn the_answer_most_given_wins_a_vote() {
        let answers: Vec<String> = ["15 USD.", "12 USD", "15 usd", "12 USD"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        // Two each: the tie goes to the earliest.
        assert_eq!(vote(&answers), Some(("15 USD.".into(), 2, 4)));
        assert_eq!(vote(&[]), None);
    }

    /// A model for every arm: one agent alone looks up only plan B (and
    /// answers 12 USD), as does each of its samples; the graph's split
    /// asks for both prices, its parts each look one up, and the combine
    /// adds them.
    fn models() -> Result<Vec<Box<dyn Model>>, String> {
        let script = json!([
            {"when": "Split the task into parts", "say": "{\"parts\": [\"the price of plan b\", \"the price of support\"]}"},
            {"when": "Your part of it:\nthe price of plan b", "call": "price", "input": {"item": "plan b"}},
            {"when": "Your part of it:\nthe price of plan b", "say": "12 USD"},
            {"when": "Your part of it:\nthe price of support", "call": "price", "input": {"item": "support"}},
            {"when": "Your part of it:\nthe price of support", "say": "3 USD"},
            {"when": "Put the parts' reports together", "say": "15 USD"},
            {"call": "price", "input": {"item": "plan b"}},
            {"say": "12 USD"}
        ]);
        Ok(vec![Box::new(Scripted::from_json(
            "script",
            &script.to_string(),
        )?)])
    }

    #[test]
    fn a_graph_earns_traffic_only_by_beating_both_baselines() {
        let set = TaskSet::parse(SET.as_bytes()).unwrap();
        let arms = [
            Arm::Single,
            Arm::Vote { samples: 3 },
            Arm::Graph {
                graph: crate::graph::template("fan-out").unwrap(),
            },
        ];
        let comparison = compare(
            &set,
            Hash32::ZERO,
            &arms,
            &models,
            None,
            30_000,
            8,
            &Limits::default(),
        )
        .unwrap();
        let by = |name: &str| {
            comparison
                .arms
                .iter()
                .find(|a| a.arm.name() == name)
                .unwrap()
        };
        assert_eq!(by("single").passed(), 0);
        assert_eq!(by("vote-3").passed(), 0);
        assert_eq!(by("fan-out").passed(), 8, "{:?}", by("fan-out").trials[0]);
        assert_eq!(by("fan-out").per_repeat(), vec![1; 8]);
        assert_eq!(
            by("single").trials[0].failures,
            ["the answer does not say \"15 USD\""]
        );
        // Each vote sample ran with a third of the budget, and the graph's
        // nodes shared it: no run reported more than the case's budget.
        assert!(comparison
            .arms
            .iter()
            .flat_map(|a| &a.trials)
            .all(|t| t.tokens <= 30_000));
        assert_eq!(comparison.earned(), ["fan-out"]);
        let lines = comparison.lines();
        assert!(
            lines[0].starts_with("set lookups: 1 case, each run 8 times"),
            "{lines:?}"
        );
        assert!(
            lines.last().unwrap().starts_with("earned: fan-out"),
            "{lines:?}"
        );

        // With one run, the same result earns nothing: the interval reaches
        // below zero.
        let few = compare(
            &set,
            Hash32::ZERO,
            &arms,
            &models,
            None,
            30_000,
            1,
            &Limits::default(),
        )
        .unwrap();
        assert!(few.earned().is_empty(), "{:?}", few.lines());
        // Without the vote, nothing can earn.
        let alone = compare(
            &set,
            Hash32::ZERO,
            &[Arm::Single, arms[2].clone()],
            &models,
            None,
            30_000,
            8,
            &Limits::default(),
        )
        .unwrap();
        assert!(alone.earned().is_empty());
    }

    #[test]
    fn a_bad_comparison_is_refused() {
        let set = TaskSet::parse(SET.as_bytes()).unwrap();
        let run = |arms: &[Arm], repeats| {
            compare(
                &set,
                Hash32::ZERO,
                arms,
                &models,
                None,
                1_000,
                repeats,
                &Limits::default(),
            )
        };
        assert!(run(&[Arm::Single], 0).is_err());
        assert!(run(&[Arm::Vote { samples: 1 }], 1).is_err());
        assert!(run(&[Arm::Single, Arm::Single], 1).is_err());
        assert!(TaskSet::parse(br#"{"name": "Bad Name", "cases": []}"#).is_err());
    }
}
