//! Improving the whole agent, keeping only what measurably helps.
//!
//! `sion agent evolve` improves one skill's instructions against an
//! evaluation set. This improves the agent itself: the words its operator
//! gives it, which model it reaches for, and how it works a task (alone,
//! voting, or as a graph). Each [`Tweak`] is scored on a set of tasks
//! against the agent as it is, and kept only when it clears two bars:
//!
//! 1. It is **significantly better** on the tasks it was tried on —
//!    Newcombe's 95% interval for the difference in pass rate has its
//!    lower end above zero, the same bar a model must clear to earn first
//!    place in a route.
//! 2. It **does not regress** on tasks it never saw: it fails no held-out
//!    task the agent already passes.
//!
//! A tweak that clears both is recorded with its evidence ([`Kept`], a
//! line in `improvements.jsonl` beside the agent): what it changed, the
//! counts on both sets, and the interval. So every change the agent keeps
//! can be read back with the reason it was kept, and one that only looked
//! better on a handful of tasks is turned down.
//!
//! Scoring is given to [`improve`] as a function, so what actually runs the
//! tasks — a real model, or a scripted one in the tests — is the caller's.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use sion_agent::compare::{newcombe, Arm};

use crate::error::CliError;

/// One change to how an agent works.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tweak {
    /// Words added to what the operator tells it, after the rest.
    Instructions {
        /// The words.
        words: String,
    },
    /// The models a kind of task reaches for, in order.
    Route {
        /// The route's name.
        task: String,
        /// Its models, best first.
        models: Vec<String>,
    },
    /// How it works a task: alone, voting, or as a graph.
    Working {
        /// The way of working.
        #[serde(with = "arm_name")]
        arm: Arm,
    },
}

/// An `Arm` as its name, which is all a record needs of it.
mod arm_name {
    use serde::{Deserialize, Deserializer, Serializer};
    use sion_agent::compare::Arm;

    pub fn serialize<S: Serializer>(arm: &Arm, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&arm.name())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Arm, D::Error> {
        let name = String::deserialize(d)?;
        Ok(
            match name.strip_prefix("vote-").and_then(|n| n.parse().ok()) {
                Some(samples) => Arm::Vote { samples },
                None => Arm::Single,
            },
        )
    }
}

impl Tweak {
    /// A line saying what it changes.
    pub fn describe(&self) -> String {
        match self {
            Tweak::Instructions { words } => {
                let shown: String = words.chars().take(60).collect();
                format!("its instructions, adding {shown:?}")
            }
            Tweak::Route { task, models } => {
                format!("the models for {task}: {}", models.join(", "))
            }
            Tweak::Working { arm } => format!("how it works a task: {}", arm.name()),
        }
    }
}

/// How a way of working did on a set of tasks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Outcome {
    /// Tasks it passed.
    pub passed: usize,
    /// Tasks it was tried on.
    pub total: usize,
    /// The tokens it used.
    pub tokens: u64,
}

impl Outcome {
    /// Its pass rate, 0 for no tasks.
    pub fn rate(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            self.passed as f64 / self.total as f64
        }
    }
}

/// How a tweak did on tasks it never saw, case by case, beside the agent
/// as it is: the pair a regression is read from.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeldOut {
    /// Whether the agent as it is passed each held-out task.
    pub before: Vec<bool>,
    /// Whether the tweak passed each.
    pub after: Vec<bool>,
}

impl HeldOut {
    /// The first held-out task the tweak fails and the agent passes, if
    /// there is one: the regression that turns a tweak down.
    pub fn regression(&self) -> Option<usize> {
        self.before
            .iter()
            .zip(&self.after)
            .position(|(before, after)| *before && !after)
    }

    /// What it passed, of what it was tried on.
    pub fn outcome(&self) -> Outcome {
        Outcome {
            passed: self.after.iter().filter(|p| **p).count(),
            total: self.after.len(),
            tokens: 0,
        }
    }
}

/// A tweak kept, with the evidence for keeping it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Kept {
    /// What it changes.
    pub tweak: Tweak,
    /// When it was kept (seconds since the epoch, UTC).
    pub at: u64,
    /// How the agent as it is did on the tasks tried.
    pub before: Outcome,
    /// How the tweak did on them.
    pub after: Outcome,
    /// Newcombe's 95% interval for the difference in pass rate: its lower
    /// end is above zero, which is why it was kept.
    pub interval: (f64, f64),
    /// How it did on tasks it never saw.
    pub held_out: HeldOut,
}

/// A tweak turned down, and why.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TurnedDown {
    /// What it would have changed.
    pub tweak: Tweak,
    /// Why it was turned down.
    pub why: String,
}

/// What an improvement came to.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Improvement {
    /// How the agent as it is did on the tasks tried (nothing tried, if
    /// only named routes' models were).
    pub before: Outcome,
    /// How each named route's models as they are did on them, for a route
    /// whose models were tried.
    pub routes: Vec<(String, Outcome)>,
    /// The tweaks kept, in the order they were tried.
    pub kept: Vec<Kept>,
    /// The tweaks turned down, with the reason for each.
    pub turned_down: Vec<TurnedDown>,
}

impl Improvement {
    /// What it did, a line each.
    pub fn lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        if self.before.total > 0 {
            lines.push(format!(
                "the agent as it is: {}/{} tasks passed",
                self.before.passed, self.before.total
            ));
        }
        for (route, before) in &self.routes {
            lines.push(format!(
                "route {route} as it is: {}/{} tasks passed",
                before.passed, before.total
            ));
        }
        for kept in &self.kept {
            lines.push(format!(
                "kept {}: {}/{} passed, up from {}/{} (95% interval {:+.0}% to {:+.0}%), and no held-out task it passed now fails",
                kept.tweak.describe(),
                kept.after.passed,
                kept.after.total,
                kept.before.passed,
                kept.before.total,
                kept.interval.0 * 100.0,
                kept.interval.1 * 100.0
            ));
        }
        for down in &self.turned_down {
            lines.push(format!(
                "turned down {}: {}",
                down.tweak.describe(),
                down.why
            ));
        }
        if self.kept.is_empty() {
            lines.push("nothing measurably better: the agent is left as it is".into());
        }
        lines
    }
}

/// Runs a way of working on the tasks: the agent as it is, given `None`.
pub type Score<'a> = &'a dyn Fn(Option<&Tweak>) -> Result<Outcome, CliError>;

/// Runs a way of working on tasks it never saw, pass by pass.
pub type ScoreHeldOut<'a> = &'a dyn Fn(Option<&Tweak>) -> Result<Vec<bool>, CliError>;

/// Whether `after` is significantly better than `before`: Newcombe's 95%
/// interval for the difference in pass rate, and whether its lower end is
/// above zero.
pub fn better(after: &Outcome, before: &Outcome) -> ((f64, f64), bool) {
    let interval = newcombe((after.passed, after.total), (before.passed, before.total));
    (interval, interval.0 > 0.0)
}

/// Tries each of `proposals` against the agent as it is: `score` runs a
/// tweak (or, given `None`, the agent as it is) on the tasks, and
/// `score_held_out` runs it on tasks it never saw. A tweak is kept only if
/// it is significantly better on the tasks tried and regresses on none of
/// the held-out ones.
pub fn improve(
    proposals: &[Tweak],
    now: u64,
    score: Score<'_>,
    score_held_out: ScoreHeldOut<'_>,
) -> Result<Improvement, CliError> {
    let before = score(None)?;
    if before.total == 0 {
        return Err(CliError::Usage(
            "there are no tasks to measure an improvement on".into(),
        ));
    }
    let held_before = score_held_out(None)?;
    let mut improvement = Improvement {
        before,
        ..Improvement::default()
    };
    for tweak in proposals {
        let after = score(Some(tweak))?;
        let (interval, significant) = better(&after, &before);
        if !significant {
            improvement.turned_down.push(TurnedDown {
                tweak: tweak.clone(),
                why: format!(
                    "{}/{} passed against {}/{}: not better beyond chance (95% interval {:+.0}% to {:+.0}%)",
                    after.passed,
                    after.total,
                    before.passed,
                    before.total,
                    interval.0 * 100.0,
                    interval.1 * 100.0
                ),
            });
            continue;
        }
        let held_out = HeldOut {
            before: held_before.clone(),
            after: score_held_out(Some(tweak))?,
        };
        if let Some(at) = held_out.regression() {
            improvement.turned_down.push(TurnedDown {
                tweak: tweak.clone(),
                why: format!(
                    "better on the tasks tried, but it fails held-out task {} that the agent passes",
                    at + 1
                ),
            });
            continue;
        }
        improvement.kept.push(Kept {
            tweak: tweak.clone(),
            at: now,
            before,
            after,
            interval,
            held_out,
        });
    }
    Ok(improvement)
}

/// Where an agent's kept improvements are recorded.
pub fn log_path(profile: &str) -> PathBuf {
    crate::model_routes::dir().join(format!("{profile}-improvements.jsonl"))
}

/// Records what an improvement kept, one line of JSON each, so that every
/// change the agent keeps can be read back with the evidence for it.
pub fn record(profile: &str, improvement: &Improvement) -> Result<(), CliError> {
    if improvement.kept.is_empty() {
        return Ok(());
    }
    let path = log_path(profile);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| CliError::Usage(e.to_string()))?;
    }
    let mut text = String::new();
    for kept in &improvement.kept {
        let line = serde_json::to_string(kept)
            .map_err(|e| CliError::Usage(format!("the improvement could not be written: {e}")))?;
        text.push_str(&line);
        text.push('\n');
    }
    use std::io::Write as _;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| CliError::Usage(format!("{}: {e}", path.display())))?;
    file.write_all(text.as_bytes())
        .map_err(|e| CliError::Usage(format!("{}: {e}", path.display())))
}

/// Every improvement an agent has kept, oldest first.
pub fn kept(profile: &str) -> Result<Vec<Kept>, CliError> {
    let path = log_path(profile);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(CliError::Usage(format!("{}: {e}", path.display()))),
    };
    Ok(text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect())
}

/// The route name a [`Tweak::Route`] gives to mean the profile's own
/// models, the ones a task reaches for when no route takes it.
pub const DEFAULT_ROUTE: &str = "default";

/// What an operator tells the agent, `instructions`, with `words` added
/// after them.
pub fn with_words(instructions: Option<&str>, words: &str) -> String {
    match instructions.map(str::trim).filter(|i| !i.is_empty()) {
        Some(before) => format!("{before}\n\n{}", words.trim()),
        None => words.trim().to_string(),
    }
}

/// Whether `tweak` can be tried: words that are there and not too long,
/// models named, a vote of 2 to 9.
pub fn check(tweak: &Tweak) -> Result<(), CliError> {
    let usage = |why: String| Err(CliError::Usage(why));
    match tweak {
        Tweak::Instructions { words } if words.trim().is_empty() => {
            usage("words to add are empty".into())
        }
        Tweak::Instructions { words } if words.len() > sion_agent::evolve::MAX_INSTRUCTIONS => {
            usage(format!(
                "words to add are at most {} bytes",
                sion_agent::evolve::MAX_INSTRUCTIONS
            ))
        }
        Tweak::Route { task, models } if task.trim().is_empty() || models.is_empty() => {
            usage("a route to try names the route and its models: <route>=<model>,<model>".into())
        }
        Tweak::Working {
            arm: Arm::Vote { samples },
        } if !(2..=sion_agent::compare::MAX_SAMPLES).contains(samples) => usage(format!(
            "a vote takes 2 to {} samples",
            sion_agent::compare::MAX_SAMPLES
        )),
        Tweak::Working {
            arm: Arm::Graph { .. },
        } => usage("a graph is compared with `sion agent graph`, not tried here".into()),
        _ => Ok(()),
    }
}

/// How the agent of `profile`, changed by `tweak` (or as it is, given
/// `None`), does on every case of `set` (whose digest is `digest`): each
/// case run `runs` times with `budget_tokens`, each run starting with the
/// set's system prompt and then the operator's instructions, with the
/// profile's models in order.
pub fn run_tasks(
    profile: &crate::profiles::Profile,
    set: &sion_agent::compare::TaskSet,
    digest: sion_core::hash::Hash32,
    tweak: Option<&Tweak>,
    runs: u32,
    budget_tokens: u64,
) -> Result<sion_agent::compare::ArmResult, CliError> {
    let mut instructions = crate::improve_loop::instructions_of(profile).unwrap_or_default();
    let mut models = profile.models.clone();
    let mut arm = Arm::Single;
    match tweak {
        Some(Tweak::Instructions { words }) => {
            instructions = with_words(Some(&instructions), words)
        }
        Some(Tweak::Route { models: given, .. }) => models = given.clone(),
        Some(Tweak::Working { arm: given }) => arm = given.clone(),
        None => {}
    }
    if models.is_empty() {
        return Err(CliError::Usage(format!(
            "profile {} has no model to run the tasks with",
            profile.name
        )));
    }
    let mut tasks = set.clone();
    tasks.system = Some(sion_agent::evolve::with_instructions(
        set.system(),
        &instructions,
    ));
    let make = crate::skill_improve::models_of(&models);
    let comparison = sion_agent::compare::compare(
        &tasks,
        digest,
        &[arm],
        &make,
        None,
        budget_tokens,
        runs,
        &sion_agent::Limits::default(),
    )
    .map_err(CliError::Usage)?;
    comparison
        .arms
        .into_iter()
        .next()
        .ok_or_else(|| CliError::Usage("the tasks ran nothing".into()))
}

/// Whether each case of `set` was passed in `result`, in the set's order:
/// passed in more than half of its runs.
pub fn per_case(
    set: &sion_agent::compare::TaskSet,
    result: &sion_agent::compare::ArmResult,
) -> Vec<bool> {
    set.cases
        .iter()
        .map(|case| {
            let runs: Vec<bool> = result
                .trials
                .iter()
                .filter(|trial| trial.case == case.id)
                .map(|trial| trial.passed)
                .collect();
            let passed = runs.iter().filter(|passed| **passed).count();
            !runs.is_empty() && passed * 2 > runs.len()
        })
        .collect()
}

/// Tries `proposals` on the agent of `profile`: scored on the set of
/// tasks in `tasks` (its exact bytes) against the agent as it is, and on
/// the set in `held_out`, which none may then fail where the agent passes
/// ([`improve`]). Each case is run `runs` times with `budget_tokens`.
pub fn try_changes(
    profile: &crate::profiles::Profile,
    tasks: &[u8],
    held_out: &[u8],
    proposals: &[Tweak],
    runs: u32,
    budget_tokens: u64,
    now: u64,
) -> Result<Improvement, CliError> {
    use sion_agent::compare::TaskSet;
    let set = TaskSet::parse(tasks).map_err(CliError::Usage)?;
    let held = TaskSet::parse(held_out).map_err(CliError::Usage)?;
    let digest = sion_agent::evals::digest(tasks);
    let held_digest = sion_agent::evals::digest(held_out);
    if digest == held_digest {
        return Err(CliError::Usage(
            "the held-out tasks are the tasks tried: hold out tasks the changes are not chosen on"
                .into(),
        ));
    }
    if proposals.is_empty() {
        return Err(CliError::Usage(
            "give a change to try: --instructions, --route, --vote or --from".into(),
        ));
    }
    for tweak in proposals {
        check(tweak)?;
    }
    // A named route's models are tried against that route's models as
    // they are; everything else against the agent as it is, on its own
    // models.
    let route_of = |tweak: &Tweak| match tweak {
        Tweak::Route { task, .. } if task != DEFAULT_ROUTE => Some(task.clone()),
        _ => None,
    };
    let mut groups: Vec<(Option<String>, Vec<Tweak>)> = Vec::new();
    for tweak in proposals {
        let route = route_of(tweak);
        match groups.iter_mut().find(|(r, _)| *r == route) {
            Some((_, group)) => group.push(tweak.clone()),
            None => groups.push((route, vec![tweak.clone()])),
        }
    }
    let mut improvement = Improvement::default();
    for (route, group) in groups {
        let mut base = profile.clone();
        if let Some(name) = &route {
            let found = profile
                .routes
                .iter()
                .find(|r| &r.name == name)
                .ok_or_else(|| {
                    CliError::Usage(format!(
                        "profile {} has no route {name} (sion agent routes set), or try `default`",
                        profile.name
                    ))
                })?;
            base.models = found.models.clone();
        }
        let score = |tweak: Option<&Tweak>| -> Result<Outcome, CliError> {
            let result = run_tasks(&base, &set, digest, tweak, runs, budget_tokens)?;
            Ok(Outcome {
                passed: result.passed(),
                total: result.total(),
                tokens: result.tokens(),
            })
        };
        let score_held_out = |tweak: Option<&Tweak>| -> Result<Vec<bool>, CliError> {
            let result = run_tasks(&base, &held, held_digest, tweak, runs, budget_tokens)?;
            Ok(per_case(&held, &result))
        };
        let tried = improve(&group, now, &score, &score_held_out)?;
        match route {
            Some(name) => improvement.routes.push((name, tried.before)),
            None => improvement.before = tried.before,
        }
        improvement.kept.extend(tried.kept);
        improvement.turned_down.extend(tried.turned_down);
    }
    Ok(improvement)
}

/// Of the changes `kept` that `kind` picks, the one that passed the most
/// tasks (the first, of equals).
fn best_of(kept: &[Kept], kind: fn(&Tweak) -> bool) -> Option<&Kept> {
    let mut best: Option<&Kept> = None;
    for one in kept.iter().filter(|one| kind(&one.tweak)) {
        let better = match best {
            None => true,
            Some(so_far) => one.after.passed > so_far.after.passed,
        };
        if better {
            best = Some(one);
        }
    }
    best
}

/// Makes what `improvement` kept `profile`'s: of the words kept, those
/// that passed the most tasks are added to its instructions; of the
/// models kept for a route, the best become that route's (`default`: its
/// own models). Each was measured on its own against the agent as it was.
/// A way of working is not a profile's setting, so a vote kept is only
/// recorded. Returns a line for each change, made or not.
pub fn apply(
    profile: &mut crate::profiles::Profile,
    improvement: &Improvement,
) -> Result<Vec<String>, CliError> {
    let mut lines = Vec::new();
    if let Some(kept) = best_of(&improvement.kept, |t| {
        matches!(t, Tweak::Instructions { .. })
    }) {
        if let Tweak::Instructions { words } = &kept.tweak {
            profile.instructions = Some(with_words(profile.instructions.as_deref(), words));
            lines.push(format!(
                "profile {} now has {}",
                profile.name,
                kept.tweak.describe()
            ));
        }
    }
    let routes: Vec<&String> = {
        let mut names: Vec<&String> = improvement
            .kept
            .iter()
            .filter_map(|k| match &k.tweak {
                Tweak::Route { task, .. } => Some(task),
                _ => None,
            })
            .collect();
        names.sort();
        names.dedup();
        names
    };
    for route in routes {
        let mut best: Option<&Kept> = None;
        for one in &improvement.kept {
            if matches!(&one.tweak, Tweak::Route { task, .. } if task == route) {
                let better = match best {
                    None => true,
                    Some(so_far) => one.after.passed > so_far.after.passed,
                };
                if better {
                    best = Some(one);
                }
            }
        }
        let Some(Kept {
            tweak: Tweak::Route { models, .. },
            ..
        }) = best
        else {
            continue;
        };
        if route == DEFAULT_ROUTE {
            profile.models = models.clone();
            lines.push(format!(
                "profile {} now reaches for {}",
                profile.name,
                models.join(", ")
            ));
            continue;
        }
        match profile.routes.iter_mut().find(|r| &r.name == route) {
            Some(found) => {
                let before = std::mem::replace(&mut found.models, models.clone());
                if let Err(why) = found.check() {
                    found.models = before;
                    lines.push(format!("route {route} kept as it was: {why}"));
                } else {
                    lines.push(format!(
                        "route {route} of profile {} now has {}",
                        profile.name,
                        models.join(", ")
                    ));
                }
            }
            None => lines.push(format!(
                "profile {} has no route {route}: the models kept for it are recorded, not set (sion agent routes set)",
                profile.name
            )),
        }
    }
    if improvement
        .kept
        .iter()
        .any(|k| matches!(k.tweak, Tweak::Working { .. }))
    {
        lines.push(
            "how it works a task is not a profile's setting: the way of working kept is recorded only"
                .into(),
        );
    }
    Ok(lines)
}

/// `sion agent improve …`
#[derive(Debug, clap::Subcommand)]
pub enum ImproveCommand {
    /// Try changes to a profile's agent on a set of tasks: words added to
    /// its instructions, the models a route reaches for, a vote. Each is
    /// run against the agent as it is and kept only if it is better beyond
    /// chance (Newcombe's 95% interval above zero) and fails no held-out
    /// task the agent passes; what is kept is recorded with its evidence.
    Try {
        /// The profile.
        profile: String,
        /// The tasks the changes are tried on: a set of tasks, as `sion
        /// agent routes eval --set` takes (a name and cases with checks).
        #[arg(long, value_name = "FILE")]
        tasks: std::path::PathBuf,
        /// Tasks held out: none of the changes is chosen on them, and a
        /// change that fails one the agent passes is turned down.
        #[arg(long = "held-out", value_name = "FILE")]
        held_out: std::path::PathBuf,
        /// Words to add after what the operator tells it (repeat to try
        /// several, each on its own).
        #[arg(long = "instructions", value_name = "TEXT")]
        instructions: Vec<String>,
        /// Models to try, best first: `<route>=<model>,<model>`, the route
        /// `default` for the profile's own models (repeat to try several).
        #[arg(long = "route", value_name = "ROUTE=MODELS")]
        routes: Vec<String>,
        /// Answer by a vote of this many runs, the answer given most often
        /// winning (2 to 9; repeat to try several).
        #[arg(long = "vote", value_name = "SAMPLES")]
        votes: Vec<u32>,
        /// Changes to try from a file: a JSON list of
        /// `{"instructions": {"words": …}}`, `{"route": {"task": …,
        /// "models": […]}}` and `{"working": {"arm": "vote-3"}}`.
        #[arg(long = "from", value_name = "FILE")]
        from: Option<std::path::PathBuf>,
        /// How many times each task is run by each way of working.
        #[arg(long, default_value_t = 4)]
        runs: u32,
        /// The tokens each run of a task may use.
        #[arg(long = "budget-tokens", default_value_t = 20_000)]
        budget_tokens: u64,
        /// Make what was kept the profile's: the best words kept added to
        /// its instructions, the best models kept for a route that route's.
        #[arg(long)]
        apply: bool,
    },
    /// The changes a profile's agent has kept, each with the evidence for
    /// keeping it.
    Kept {
        /// The profile.
        profile: String,
    },
    /// Say how one of the agent's sessions went, so that it can be learnt
    /// from (`--good` or `--bad`); a session whose check passed needs none.
    Mark {
        /// The session (its id, or a unique prefix of it).
        session: String,
        /// It went well.
        #[arg(long, conflicts_with = "bad", required_unless_present = "bad")]
        good: bool,
        /// It went badly.
        #[arg(long)]
        bad: bool,
        /// What you say of it.
        #[arg(long, default_value = "")]
        note: String,
    },
    /// Turn the profile's sessions whose outcome is known (marked, or their
    /// check passed) into a set of tasks: each task asked again, of
    /// stand-ins that answer what its tools answered, checked against what
    /// the session did.
    Harvest {
        /// The profile.
        profile: String,
        /// Where to write the set (default: printed).
        #[arg(long, value_name = "FILE")]
        out: Option<std::path::PathBuf>,
    },
    /// Seal a set of tasks for the profile: a share held apart to confirm
    /// changes on, which no proposer is shown, with canary cases among them
    /// and a number of confirmations. With --wallet, the seal's digests
    /// are published on the ledger before anything is tried on it.
    Seal {
        /// The profile.
        profile: String,
        /// The tasks (a set, as `sion agent improve harvest` writes).
        #[arg(long, value_name = "FILE")]
        tasks: std::path::PathBuf,
        /// The share sealed, in percent.
        #[arg(long, default_value_t = crate::sealed::SHARE)]
        share: u32,
        /// The confirmations the seal allows; each is held to 5% split
        /// among them all.
        #[arg(long, default_value_t = crate::sealed::BUDGET)]
        confirmations: u32,
        /// Canary cases added to the sealed tasks.
        #[arg(long, default_value_t = crate::sealed::CANARIES)]
        canaries: usize,
        /// A seed mixed into the split (default: drawn).
        #[arg(long)]
        seed: Option<u64>,
        /// Publish the seal as this wallet's claim.
        #[arg(long)]
        wallet: Option<String>,
    },
    /// Let the agent improve itself: a proposer model writes changes from
    /// the tasks it failed among the seal's tasks to choose on; each is
    /// tried there, and kept only when confirmed better on the sealed
    /// tasks. With --apply a kept change becomes the profile's; with
    /// --wallet each is published on the ledger.
    Run {
        /// The profile.
        profile: String,
        /// The model that proposes changes.
        #[arg(long, value_name = "MODEL")]
        proposer: String,
        /// Rounds, at most.
        #[arg(long, default_value_t = 4)]
        rounds: usize,
        /// How many times each task is run by each variant.
        #[arg(long, default_value_t = 4)]
        runs: u32,
        /// The tokens each run of a task may use.
        #[arg(long = "budget-tokens", default_value_t = 20_000)]
        budget_tokens: u64,
        /// The tokens the whole run may use, its proposer's included (0:
        /// no limit but the rounds).
        #[arg(long = "total-tokens", default_value_t = 0)]
        total_tokens: u64,
        /// A model the proposer may choose, besides the profile's own
        /// (repeat for more).
        #[arg(long = "model", value_name = "MODEL")]
        models: Vec<String>,
        /// The seed of the archive's draws (default: drawn).
        #[arg(long)]
        seed: Option<u64>,
        /// Make each kept change the profile's.
        #[arg(long)]
        apply: bool,
        /// Publish each kept change as this wallet's claim.
        #[arg(long)]
        wallet: Option<String>,
    },
    /// The changes the agent kept from its own work, with their evidence;
    /// with --check, each held to its claim on the ledger.
    Lineage {
        /// The profile.
        profile: String,
        /// Check the claims on the ledger.
        #[arg(long)]
        check: bool,
    },
    /// Make a kept change the profile's.
    Adopt {
        /// The profile.
        profile: String,
        /// The kept change (its id, or a unique prefix of it).
        step: String,
    },
    /// Take back the last change made the profile's.
    Undo {
        /// The profile.
        profile: String,
    },
    /// Let the agent change its own code: a coding agent edits a copy of
    /// the source, its commands in the sandbox; a change that keeps to the
    /// paths it may touch, builds, passes its tests, is better on the
    /// seal's tasks to choose on and is confirmed on the sealed tasks, the
    /// candidate scored by this harness, becomes a release (published with
    /// --wallet). Nothing is installed.
    Code {
        /// The profile.
        profile: String,
        /// The source: a git checkout, whose last commit is the code as it
        /// stands.
        #[arg(long, value_name = "DIR")]
        source: std::path::PathBuf,
        /// The model that changes the code.
        #[arg(long, value_name = "MODEL")]
        coder: String,
        /// The build, run with /bin/sh at the source's root in the sandbox.
        #[arg(long, default_value = "cargo build --offline -p sion-cli")]
        build: String,
        /// The tests.
        #[arg(long, default_value = "cargo test --offline -p sion-agent --lib")]
        test: String,
        /// The program that runs a case, and its arguments, at the root.
        #[arg(long, default_value = "target/debug/sion agent case --stdio")]
        candidate: String,
        /// A path a change may touch (repeat; default: the agent's runtime,
        /// crates/sion-agent/src/, apart from what scores or guards it).
        #[arg(long = "allow", value_name = "PATH")]
        allow: Vec<String>,
        /// The most files a change may touch.
        #[arg(long = "max-files", default_value_t = 8)]
        max_files: usize,
        /// The most lines it may add and remove.
        #[arg(long = "max-lines", default_value_t = 400)]
        max_lines: usize,
        /// A directory the sandbox shows read-only, a toolchain's (repeat).
        #[arg(long = "readable", value_name = "DIR")]
        readable: Vec<std::path::PathBuf>,
        /// A variable the build and the tests are given: NAME=VALUE
        /// (repeat).
        #[arg(long = "env", value_name = "NAME=VALUE")]
        env: Vec<String>,
        /// Seconds the build and the tests may each take.
        #[arg(long = "build-secs", default_value_t = 1_800)]
        build_secs: u64,
        /// Rounds, at most.
        #[arg(long, default_value_t = 2)]
        rounds: usize,
        /// How many times each task is run.
        #[arg(long, default_value_t = 2)]
        runs: u32,
        /// The tokens each run of a task may use.
        #[arg(long = "budget-tokens", default_value_t = 20_000)]
        budget_tokens: u64,
        /// Publish each release as this wallet's claim.
        #[arg(long)]
        wallet: Option<String>,
    },
    /// Rebuild and score a release someone published: the source at its
    /// commit with its patch, and as it stood, each built and tested in
    /// the sandbox and scored by this harness on the sealed tasks it was
    /// confirmed on; publish what you found if it holds up.
    Attest {
        /// The release's claim.
        release: String,
        /// A checkout of the source holding the release's commit.
        #[arg(long, value_name = "DIR")]
        source: std::path::PathBuf,
        /// The release's patch.
        #[arg(long, value_name = "FILE")]
        patch: std::path::PathBuf,
        /// The sealed tasks it was confirmed on, as its publisher gives
        /// them.
        #[arg(long, value_name = "FILE")]
        tasks: std::path::PathBuf,
        /// The wallet that attests.
        #[arg(long)]
        wallet: String,
        /// A directory the sandbox shows read-only (repeat).
        #[arg(long = "readable", value_name = "DIR")]
        readable: Vec<std::path::PathBuf>,
        /// A variable the build and the tests are given (repeat).
        #[arg(long = "env", value_name = "NAME=VALUE")]
        env: Vec<String>,
        /// A path a change may touch, as the publisher allowed (repeat).
        #[arg(long = "allow", value_name = "PATH")]
        allow: Vec<String>,
        /// Seconds the build and the tests may each take.
        #[arg(long = "build-secs", default_value_t = 1_800)]
        build_secs: u64,
        /// How many times each task is run.
        #[arg(long, default_value_t = 2)]
        runs: u32,
        /// The tokens each run of a task may use.
        #[arg(long = "budget-tokens", default_value_t = 20_000)]
        budget_tokens: u64,
    },
    /// Apply a release to a checkout of the source, only if its patch is
    /// the one published and verifiers you name attest it (or, with
    /// --undo, take it back out). Build and install the result as you
    /// build sion.
    Install {
        /// The profile whose lineage records it.
        profile: String,
        /// The release's claim.
        release: String,
        /// The checkout.
        #[arg(long, value_name = "DIR")]
        source: std::path::PathBuf,
        /// The release's patch.
        #[arg(long, value_name = "FILE")]
        patch: std::path::PathBuf,
        /// An attestation's claim (repeat).
        #[arg(long = "attestation", value_name = "CLAIM")]
        attestations: Vec<String>,
        /// An account, or a wallet's name, whose attestation counts
        /// (repeat).
        #[arg(long = "verifier", value_name = "ACCOUNT")]
        verifiers: Vec<String>,
        /// Attestations needed, by different verifiers.
        #[arg(long, default_value_t = 1)]
        need: usize,
        /// A path the patch may touch (repeat; default: the agent's
        /// runtime, crates/sion-agent/src/, apart from what scores or
        /// guards it). The patch is held to these whatever its publisher
        /// and attesters allowed.
        #[arg(long = "allow", value_name = "PATH")]
        allow: Vec<String>,
        /// Take the release back out instead.
        #[arg(long)]
        undo: bool,
    },
    /// Improve the improver: the proposer's instructions and settings,
    /// graded by how much the agent improves on tasks it was not shown
    /// when the improver writes its changes, over problems (pairs of task
    /// sets). The improver proposes its own changes, under its own
    /// instructions; one is kept only when the agent improves more with
    /// it, beyond chance, on the problems given and then on problems held
    /// out, which no proposer is shown. With --show, the improver as it is.
    Improver {
        /// The profile.
        profile: String,
        /// Show the profile's improver, and do nothing else.
        #[arg(long)]
        show: bool,
        /// The model that proposes the agent's changes.
        #[arg(long, value_name = "MODEL", required_unless_present = "show")]
        proposer: Option<String>,
        /// The model the improver proposes its own changes with (default:
        /// the proposer).
        #[arg(long = "meta-proposer", value_name = "MODEL")]
        meta_proposer: Option<String>,
        /// A problem to grade on: `<tasks to choose on>,<held-out tasks>`
        /// (repeat).
        #[arg(long = "problem", value_name = "TRAIN,HELD")]
        problems: Vec<String>,
        /// A problem held out, to confirm changes on (repeat).
        #[arg(long = "held-out-problem", value_name = "TRAIN,HELD")]
        held_out: Vec<String>,
        /// Rounds of changes to the improver, at most.
        #[arg(long, default_value_t = 2)]
        rounds: usize,
        /// Rounds of the agent's loop on each problem.
        #[arg(long = "agent-rounds", default_value_t = 2)]
        agent_rounds: usize,
        /// How many times each task is run.
        #[arg(long, default_value_t = 2)]
        runs: u32,
        /// The tokens each run of a task may use.
        #[arg(long = "budget-tokens", default_value_t = 20_000)]
        budget_tokens: u64,
        /// The confirmations of changes to the improver this allows.
        #[arg(long, default_value_t = 4)]
        confirmations: u32,
        /// The seed of the archives' draws (default: drawn).
        #[arg(long)]
        seed: Option<u64>,
        /// Make a kept change the profile's improver.
        #[arg(long)]
        apply: bool,
    },
}

impl ImproveCommand {
    /// Whether it needs a node: publishing or checking on the ledger.
    pub fn needs_ledger(&self) -> bool {
        match self {
            ImproveCommand::Seal { wallet, .. } | ImproveCommand::Run { wallet, .. } => {
                wallet.is_some()
            }
            ImproveCommand::Lineage { check, .. } => *check,
            ImproveCommand::Code { wallet, .. } => wallet.is_some(),
            ImproveCommand::Attest { .. } | ImproveCommand::Install { .. } => true,
            _ => false,
        }
    }
}

/// The changes `sion agent improve try` was given, in the order tried:
/// words, then routes, then votes, then the file's.
pub fn proposals(
    instructions: &[String],
    routes: &[String],
    votes: &[u32],
    from: Option<&std::path::Path>,
) -> Result<Vec<Tweak>, CliError> {
    let mut proposals: Vec<Tweak> = instructions
        .iter()
        .map(|words| Tweak::Instructions {
            words: words.clone(),
        })
        .collect();
    for route in routes {
        let (task, models) = route.split_once('=').ok_or_else(|| {
            CliError::Usage(format!(
                "{route:?}: a route to try is <route>=<model>,<model>"
            ))
        })?;
        proposals.push(Tweak::Route {
            task: task.trim().to_string(),
            models: models
                .split(',')
                .map(str::trim)
                .filter(|m| !m.is_empty())
                .map(str::to_string)
                .collect(),
        });
    }
    proposals.extend(votes.iter().map(|samples| Tweak::Working {
        arm: Arm::Vote { samples: *samples },
    }));
    if let Some(path) = from {
        let text = std::fs::read_to_string(path)
            .map_err(|e| CliError::Usage(format!("{}: {e}", path.display())))?;
        let more: Vec<Tweak> = serde_json::from_str(&text).map_err(|e| {
            CliError::Usage(format!("{}: not a list of changes: {e}", path.display()))
        })?;
        proposals.extend(more);
    }
    Ok(proposals)
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

fn drawn() -> u64 {
    rand_core::RngCore::next_u64(&mut rand_core::OsRng)
}

/// Runs `sion agent improve …` with a node: publishing a seal or kept
/// changes, checking a lineage; anything else as [`run`] does.
pub fn run_with(
    session: &mut crate::session::Session<'_>,
    ledger: &dyn crate::ledger::Ledger,
    command: ImproveCommand,
) -> Result<(), CliError> {
    match command {
        ImproveCommand::Seal {
            profile,
            tasks,
            share,
            confirmations,
            canaries,
            seed,
            wallet: Some(wallet),
        } => {
            seal_command(&profile, &tasks, share, confirmations, canaries, seed)?;
            let claim = crate::sealed::publish(session, &wallet, &profile)?;
            println!(
                "published as claim {}, before anything was tried on it",
                claim.as_hash().to_hex()
            );
            Ok(())
        }
        ImproveCommand::Run {
            profile,
            proposer,
            rounds,
            runs,
            budget_tokens,
            total_tokens,
            models,
            seed,
            apply: make_it_so,
            wallet: Some(wallet),
        } => {
            let found = crate::profiles::get(&profile)?;
            let seal = crate::sealed::current(&profile)?;
            let mut publish =
                |step: &mut crate::improve_loop::Step| -> Result<Vec<String>, CliError> {
                    step.seal_claim = seal.claim.clone();
                    let subject = format!("agent {} kept a change: {}", found.agent, step.change);
                    let payload = step.claimed()?;
                    let claim = crate::sealed::publish_step(
                        session,
                        &wallet,
                        &found.agent,
                        payload,
                        subject,
                    )?;
                    step.claim = Some(claim.as_hash().to_hex());
                    Ok(vec![format!(
                        "published as claim {}",
                        claim.as_hash().to_hex()
                    )])
                };
            run_loop(
                &found,
                &seal,
                &proposer,
                crate::improve_loop::Settings {
                    rounds,
                    runs,
                    budget_tokens,
                    total_tokens,
                    models,
                    seed: seed.unwrap_or_else(drawn),
                },
                make_it_so,
                &mut publish,
            )
        }
        ImproveCommand::Code {
            profile,
            source,
            coder,
            build,
            test,
            candidate,
            allow,
            max_files,
            max_lines,
            readable,
            env,
            build_secs,
            rounds,
            runs,
            budget_tokens,
            wallet: Some(wallet),
        } => {
            let found = crate::profiles::get(&profile)?;
            let settings = code_settings(
                found.shell.clone(),
                &found.name,
                source,
                build,
                test,
                &candidate,
                allow,
                max_files,
                max_lines,
                readable,
                &env,
                build_secs,
                rounds,
                runs,
                budget_tokens,
            )?;
            code_command(&found, &settings, &coder, Some((session, wallet.as_str())))
        }
        ImproveCommand::Attest {
            release,
            source,
            patch,
            tasks,
            wallet,
            readable,
            env,
            allow,
            build_secs,
            runs,
            budget_tokens,
        } => {
            let read = |path: &std::path::Path| {
                std::fs::read(path).map_err(|e| CliError::Usage(format!("{}: {e}", path.display())))
            };
            let settings = code_settings(
                None,
                "attest",
                source,
                String::new(),
                String::new(),
                "true",
                allow,
                8,
                400,
                readable,
                &env,
                build_secs,
                0,
                runs,
                budget_tokens,
            )?;
            let (_, lines) = crate::code_improve::attest(
                session,
                ledger,
                &wallet,
                &release,
                &read(&patch)?,
                &read(&tasks)?,
                &settings,
                now(),
            )?;
            for line in lines {
                println!("{line}");
            }
            Ok(())
        }
        ImproveCommand::Install {
            profile,
            release,
            source,
            patch,
            attestations,
            verifiers,
            need,
            allow,
            undo,
        } => {
            let mut rules = sion_agent::guard::PatchRules::default();
            if !allow.is_empty() {
                rules.allowed = allow;
            }
            let bytes = std::fs::read(&patch)
                .map_err(|e| CliError::Usage(format!("{}: {e}", patch.display())))?;
            let verifiers: Vec<sion_core::ids::AccountId> =
                verifiers.iter().map(|v| session.id(v)).collect();
            for line in crate::code_improve::install(
                ledger,
                &profile,
                &release,
                &source,
                &bytes,
                &attestations,
                &verifiers,
                need,
                &rules,
                undo,
                now(),
            )? {
                println!("{line}");
            }
            Ok(())
        }
        ImproveCommand::Lineage {
            profile,
            check: true,
        } => {
            print_lineage(&profile)?;
            for line in crate::sealed::check_lineage(ledger, &profile)? {
                println!("{line}");
            }
            Ok(())
        }
        other => run(other),
    }
}

/// How `sion agent improve code` and `attest` change, build and score
/// code: in `shell`, a profile's own sandbox, if given (bubblewrap
/// otherwise), never with a network; the copies under `work`.
#[allow(clippy::too_many_arguments)]
fn code_settings(
    shell: Option<sion_agent::sandbox::Sandbox>,
    work: &str,
    source: std::path::PathBuf,
    build: String,
    test: String,
    candidate: &str,
    allow: Vec<String>,
    max_files: usize,
    max_lines: usize,
    readable: Vec<std::path::PathBuf>,
    env: &[String],
    build_secs: u64,
    rounds: usize,
    runs: u32,
    budget_tokens: u64,
) -> Result<crate::code_improve::CodeSettings, CliError> {
    let mut sandbox = shell.unwrap_or_else(|| {
        sion_agent::sandbox::Sandbox::new(sion_agent::sandbox::Backend::Bubblewrap)
    });
    sandbox.network = sion_agent::sandbox::Network::None;
    sandbox.readable.extend(readable);
    sandbox.limits = sion_agent::sandbox::Limits {
        timeout_secs: build_secs,
        cpu_seconds: build_secs,
        memory_mib: 8_192,
        file_mib: 4_096,
        processes: 512,
        ..sion_agent::sandbox::Limits::default()
    };
    let mut rules = sion_agent::guard::PatchRules::default();
    if !allow.is_empty() {
        rules.allowed = allow;
    }
    rules.max_files = max_files;
    rules.max_lines = max_lines;
    let env = env
        .iter()
        .map(|pair| {
            pair.split_once('=')
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .ok_or_else(|| CliError::Usage(format!("{pair:?}: a variable is NAME=VALUE")))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let candidate: Vec<String> = candidate.split_whitespace().map(str::to_string).collect();
    Ok(crate::code_improve::CodeSettings {
        source,
        build,
        test,
        candidate,
        rules,
        sandbox,
        env,
        rounds,
        runs,
        budget_tokens,
        answer_secs: 120,
        work: crate::model_routes::dir().join("code-work").join(work),
    })
}

/// Runs code improvement for `profile` on its seal: each release recorded
/// beside its lineage and, given a session and wallet, published.
fn code_command(
    profile: &crate::profiles::Profile,
    settings: &crate::code_improve::CodeSettings,
    coder: &str,
    mut publishing: Option<(&mut crate::session::Session<'_>, &str)>,
) -> Result<(), CliError> {
    let seal = crate::sealed::current(&profile.name)?;
    let (train, _) = seal.train_set()?;
    let (sealed, _) = seal.sealed_set()?;
    let problem = crate::improve_loop::Problem {
        train: &train,
        train_digest: crate::code_improve::digest_of(&seal.train)?,
        sealed: &sealed,
        sealed_digest: crate::code_improve::digest_of(&seal.sealed)?,
        z: seal.z(),
    };
    let model = crate::runtime::model(coder, 0)?;
    let name = profile.name.clone();
    let sealed_now = seal.sealed.clone();
    let mut spend = || -> Result<(u32, u32), CliError> {
        let spent = crate::sealed::spend(&name, &sealed_now)?;
        Ok((spent.used, spent.budget))
    };
    let mut kept = |release: &mut crate::code_improve::Release,
                    patch: &str|
     -> Result<Vec<String>, CliError> {
        release.seal_claim = seal.claim.clone();
        let mut lines = Vec::new();
        if let Some((session, wallet)) = publishing.as_mut() {
            let claim = crate::code_improve::publish(session, wallet, release)?;
            release.claim = Some(claim.as_hash().to_hex());
            lines.push(format!("published as claim {}", claim.as_hash().to_hex()));
        }
        crate::code_improve::record(&name, release, patch)?;
        lines.push(format!("its patch is {}", release.patch_path().display()));
        Ok(lines)
    };
    let mut sinks = crate::code_improve::CodeSinks {
        spend: &mut spend,
        kept: &mut kept,
        now: &now,
    };
    let report =
        crate::code_improve::improve_code(profile, &problem, model.as_ref(), settings, &mut sinks)?;
    for line in &report.lines {
        println!("{line}");
    }
    println!("{} tokens used", report.tokens);
    Ok(())
}

fn seal_command(
    profile: &str,
    tasks: &std::path::Path,
    share: u32,
    confirmations: u32,
    canaries: usize,
    seed: Option<u64>,
) -> Result<(), CliError> {
    let found = crate::profiles::get(profile)?;
    let bytes =
        std::fs::read(tasks).map_err(|e| CliError::Usage(format!("{}: {e}", tasks.display())))?;
    let seal = crate::sealed::seal(
        &found,
        &bytes,
        share,
        confirmations,
        canaries,
        seed.unwrap_or_else(drawn),
        now(),
    )?;
    println!(
        "sealed {} of {} tasks for profile {profile} (with {} canaries), {} left to choose changes on; {} confirmations allowed",
        seal.sealed_cases,
        seal.sealed_cases + seal.train_cases,
        seal.canaries,
        seal.train_cases,
        seal.budget
    );
    println!("tasks to choose on: {}", seal.train_path().display());
    println!(
        "sealed tasks: {} (no proposer is shown them)",
        seal.sealed_path().display()
    );
    Ok(())
}

/// Runs the loop on `profile`'s seal, `kept` told of each change it keeps
/// before it is recorded.
fn run_loop(
    profile: &crate::profiles::Profile,
    seal: &crate::sealed::Seal,
    proposer: &str,
    settings: crate::improve_loop::Settings,
    make_it_so: bool,
    kept: &mut dyn FnMut(&mut crate::improve_loop::Step) -> Result<Vec<String>, CliError>,
) -> Result<(), CliError> {
    let (train, _) = seal.train_set()?;
    let (sealed, _) = seal.sealed_set()?;
    let train_digest = sion_core::hash::Hash32::from_hex(&seal.train)
        .map_err(|e| CliError::Usage(format!("the seal's digest: {e}")))?;
    let sealed_digest = sion_core::hash::Hash32::from_hex(&seal.sealed)
        .map_err(|e| CliError::Usage(format!("the seal's digest: {e}")))?;
    if seal.left() == 0 {
        return Err(CliError::Usage(format!(
            "the seal's {} confirmations are spent: seal fresh tasks (sion agent improve seal)",
            seal.budget
        )));
    }
    let improver = crate::improve_loop::improver(&profile.name)?;
    let model = crate::runtime::model(proposer, 0)?;
    let problem = crate::improve_loop::Problem {
        train: &train,
        train_digest,
        sealed: &sealed,
        sealed_digest,
        z: seal.z(),
    };
    let name = profile.name.clone();
    let sealed_now = seal.sealed.clone();
    let seal_claim = seal.claim.clone();
    let mut spend = || -> Result<(u32, u32), CliError> {
        let spent = crate::sealed::spend(&name, &sealed_now)?;
        Ok((spent.used, spent.budget))
    };
    let mut record = |step: &mut crate::improve_loop::Step| -> Result<Vec<String>, CliError> {
        if step.seal_claim.is_none() {
            step.seal_claim = seal_claim.clone();
        }
        let mut lines = kept(step)?;
        crate::improve_loop::record(&name, step)?;
        if make_it_so {
            lines.extend(crate::improve_loop::apply(&name, step, now())?);
        }
        Ok(lines)
    };
    let stopped = || crate::runtime::kill_switch().exists();
    let mut sinks = crate::improve_loop::Sinks {
        spend: &mut spend,
        kept: &mut record,
        stopped: &stopped,
        now: &now,
    };
    let archive = crate::improve_loop::archive_for(&profile.name, &seal.train);
    let report = crate::improve_loop::improve(
        profile,
        &problem,
        model.as_ref(),
        &improver,
        &settings,
        archive,
        &mut sinks,
    )?;
    for line in &report.lines {
        println!("{line}");
    }
    if let Some(archive) = &report.archive {
        crate::improve_loop::keep_archive(&profile.name, &seal.train, archive)?;
    }
    println!("{} tokens used", report.tokens);
    if !report.kept.is_empty() {
        println!(
            "recorded in {}",
            crate::improve_loop::lineage_path(&profile.name).display()
        );
        if !make_it_so {
            println!("(--apply makes a kept change the profile's; or sion agent improve adopt)");
        }
    }
    Ok(())
}

fn print_lineage(profile: &str) -> Result<(), CliError> {
    let steps = crate::improve_loop::lineage(profile)?;
    for step in &steps {
        println!(
            "{} step {}: {} — {}/{} up from {}/{} on the tasks to choose on, {}/{} up from {}/{} on sealed tasks (confirmation {} of {}){}",
            sion_agent::schedule::rfc3339(step.at),
            step.id,
            step.change,
            step.train_after.passed,
            step.train_after.total,
            step.train_before.passed,
            step.train_before.total,
            step.sealed_after.passed,
            step.sealed_after.total,
            step.sealed_before.passed,
            step.sealed_before.total,
            step.confirmation,
            step.budget,
            match &step.claim {
                Some(claim) => format!("; claim {}", &claim[..16.min(claim.len())]),
                None => String::new(),
            }
        );
    }
    if steps.is_empty() {
        println!("profile {profile} has kept no change from its own work yet");
    }
    Ok(())
}

/// Runs `sion agent improve …`.
pub fn run(command: ImproveCommand) -> Result<(), CliError> {
    match command {
        ImproveCommand::Mark {
            session,
            good,
            bad,
            note,
        } => {
            let good = good && !bad;
            let id = crate::sealed::mark(&session, good, &note, now())?;
            println!(
                "session {id} marked as having gone {}",
                if good { "well" } else { "badly" }
            );
            Ok(())
        }
        ImproveCommand::Harvest { profile, out } => {
            let found = crate::profiles::get(&profile)?;
            let (set, passed_over) = crate::sealed::harvest(&found)?;
            for line in &passed_over {
                eprintln!("passed over {line}");
            }
            let Some(set) = set else {
                return Err(CliError::Usage(format!(
                    "no session of profile {profile} has a known outcome yet: mark some (sion agent improve mark), or run them with a check (--verify)"
                )));
            };
            let text = serde_json::to_string_pretty(&set)
                .map_err(|e| CliError::Usage(format!("the tasks could not be written: {e}")))?;
            match out {
                Some(path) => {
                    std::fs::write(&path, format!("{text}\n"))
                        .map_err(|e| CliError::Usage(format!("{}: {e}", path.display())))?;
                    println!(
                        "{} tasks from profile {profile}'s sessions, in {}",
                        set.cases.len(),
                        path.display()
                    );
                }
                None => println!("{text}"),
            }
            Ok(())
        }
        ImproveCommand::Seal {
            profile,
            tasks,
            share,
            confirmations,
            canaries,
            seed,
            wallet: None,
        } => {
            seal_command(&profile, &tasks, share, confirmations, canaries, seed)?;
            println!("(kept on this machine only: with --wallet, its digests are published on the ledger first)");
            Ok(())
        }
        ImproveCommand::Run {
            profile,
            proposer,
            rounds,
            runs,
            budget_tokens,
            total_tokens,
            models,
            seed,
            apply: make_it_so,
            wallet: None,
        } => {
            let found = crate::profiles::get(&profile)?;
            let seal = crate::sealed::current(&profile)?;
            run_loop(
                &found,
                &seal,
                &proposer,
                crate::improve_loop::Settings {
                    rounds,
                    runs,
                    budget_tokens,
                    total_tokens,
                    models,
                    seed: seed.unwrap_or_else(drawn),
                },
                make_it_so,
                &mut |_| Ok(Vec::new()),
            )
        }
        ImproveCommand::Lineage { profile, .. } => print_lineage(&profile),
        ImproveCommand::Adopt { profile, step } => {
            let steps = crate::improve_loop::lineage(&profile)?;
            let found: Vec<_> = steps.iter().filter(|s| s.id.starts_with(&step)).collect();
            let one = match found.as_slice() {
                [one] => *one,
                [] => {
                    return Err(CliError::Usage(format!(
                        "profile {profile} kept no step {step}"
                    )))
                }
                _ => return Err(CliError::Usage(format!("{step} names more than one step"))),
            };
            for line in crate::improve_loop::apply(&profile, one, now())? {
                println!("{line}");
            }
            Ok(())
        }
        ImproveCommand::Undo { profile } => {
            for line in crate::improve_loop::undo(&profile, now())? {
                println!("{line}");
            }
            Ok(())
        }
        ImproveCommand::Improver {
            profile,
            show: true,
            ..
        } => {
            let improver = crate::improve_loop::improver(&profile)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&improver)
                    .map_err(|e| CliError::Usage(e.to_string()))?
            );
            Ok(())
        }
        ImproveCommand::Improver {
            profile,
            show: false,
            proposer,
            meta_proposer,
            problems,
            held_out,
            rounds,
            agent_rounds,
            runs,
            budget_tokens,
            confirmations,
            seed,
            apply: make_it_so,
        } => {
            let found = crate::profiles::get(&profile)?;
            let proposer = proposer.ok_or_else(|| CliError::Usage("give --proposer".into()))?;
            let meta_proposer = meta_proposer.unwrap_or_else(|| proposer.clone());
            let read =
                |given: &[String]| -> Result<Vec<crate::improve_loop::MetaProblem>, CliError> {
                    given
                        .iter()
                        .map(|pair| {
                            let (train, held) = pair.split_once(',').ok_or_else(|| {
                                CliError::Usage(format!(
                                    "{pair:?}: a problem is <tasks to choose on>,<held-out tasks>"
                                ))
                            })?;
                            let bytes = |path: &str| {
                                std::fs::read(path)
                                    .map_err(|e| CliError::Usage(format!("{path}: {e}")))
                            };
                            crate::improve_loop::MetaProblem::read(
                                pair,
                                &bytes(train)?,
                                &bytes(held)?,
                            )
                        })
                        .collect()
                };
            let problems = read(&problems)?;
            let held_out = read(&held_out)?;
            let make = |spec: &str| -> Result<Box<dyn sion_agent::model::Model>, CliError> {
                crate::runtime::model(spec, 0)
            };
            let inner = || make(&proposer);
            let meta = || make(&meta_proposer);
            let settings = crate::improve_loop::MetaSettings {
                inner: crate::improve_loop::Settings {
                    rounds: agent_rounds,
                    runs,
                    budget_tokens,
                    total_tokens: 0,
                    models: Vec::new(),
                    seed: seed.unwrap_or_else(drawn),
                },
                inner_confirmations: 2,
                rounds,
                confirmations,
            };
            let start = crate::improve_loop::improver(&profile)?;
            let stopped = || crate::runtime::kill_switch().exists();
            let mut kept = |step: &crate::improve_loop::ImproverStep| {
                crate::improve_loop::record_improver(&profile, step, make_it_so, now())
            };
            let report = crate::improve_loop::improve_improver(
                &found, &start, &problems, &held_out, &inner, &meta, &settings, &stopped, &now,
                &mut kept,
            )?;
            for line in &report.lines {
                println!("{line}");
            }
            println!("{} tokens used", report.tokens);
            if !report.kept.is_empty() && !make_it_so {
                println!("(--apply makes a kept change the profile's improver)");
            }
            Ok(())
        }
        ImproveCommand::Code {
            profile,
            source,
            coder,
            build,
            test,
            candidate,
            allow,
            max_files,
            max_lines,
            readable,
            env,
            build_secs,
            rounds,
            runs,
            budget_tokens,
            wallet: None,
        } => {
            let found = crate::profiles::get(&profile)?;
            let settings = code_settings(
                found.shell.clone(),
                &found.name,
                source,
                build,
                test,
                &candidate,
                allow,
                max_files,
                max_lines,
                readable,
                &env,
                build_secs,
                rounds,
                runs,
                budget_tokens,
            )?;
            code_command(&found, &settings, &coder, None)
        }
        ImproveCommand::Seal { .. }
        | ImproveCommand::Run { .. }
        | ImproveCommand::Code { .. }
        | ImproveCommand::Attest { .. }
        | ImproveCommand::Install { .. } => Err(CliError::Usage(
            "this needs a node: give --node, or leave out --wallet".into(),
        )),
        ImproveCommand::Try {
            profile,
            tasks,
            held_out,
            instructions,
            routes,
            votes,
            from,
            runs,
            budget_tokens,
            apply: make_it_so,
        } => {
            let found = crate::profiles::get(&profile)?;
            let proposals = proposals(&instructions, &routes, &votes, from.as_deref())?;
            let read = |path: &std::path::Path| {
                std::fs::read(path).map_err(|e| CliError::Usage(format!("{}: {e}", path.display())))
            };
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or_default();
            let improvement = try_changes(
                &found,
                &read(&tasks)?,
                &read(&held_out)?,
                &proposals,
                runs,
                budget_tokens,
                now,
            )?;
            for line in improvement.lines() {
                println!("{line}");
            }
            record(&profile, &improvement)?;
            if improvement.kept.is_empty() {
                return Ok(());
            }
            println!("recorded in {}", log_path(&profile).display());
            if !make_it_so {
                println!("(--apply makes what was kept the profile's)");
                return Ok(());
            }
            // Applied to the profile as it is now: one changed while the
            // tasks ran keeps its changes.
            let mut all = crate::profiles::all()?;
            let current = all.get_mut(&profile).ok_or_else(|| {
                CliError::Usage(format!("profile {profile} was removed while the tasks ran"))
            })?;
            for line in apply(current, &improvement)? {
                println!("{line}");
            }
            crate::profiles::save_all(&all)
        }
        ImproveCommand::Kept { profile } => {
            let kept = kept(&profile)?;
            if kept.is_empty() {
                println!("profile {profile} has kept no change yet");
            }
            for one in kept {
                println!(
                    "{}: {}: {}/{} passed, up from {}/{} (95% interval {:+.0}% to {:+.0}%); held-out {}/{}",
                    sion_agent::schedule::rfc3339(one.at),
                    one.tweak.describe(),
                    one.after.passed,
                    one.after.total,
                    one.before.passed,
                    one.before.total,
                    one.interval.0 * 100.0,
                    one.interval.1 * 100.0,
                    one.held_out.outcome().passed,
                    one.held_out.outcome().total,
                );
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(words: &str) -> Tweak {
        Tweak::Instructions {
            words: words.into(),
        }
    }

    /// A scorer: the agent as it is passes `base` of `total`, and each
    /// tweak passes what the list says.
    fn scorer(
        base: usize,
        total: usize,
        each: Vec<usize>,
    ) -> impl Fn(Option<&Tweak>) -> Result<Outcome, CliError> {
        let order = std::sync::Mutex::new(0usize);
        move |tweak: Option<&Tweak>| {
            let passed = match tweak {
                None => base,
                Some(_) => {
                    let mut at = order.lock().unwrap();
                    let passed = each.get(*at).copied().unwrap_or(base);
                    *at += 1;
                    passed
                }
            };
            Ok(Outcome {
                passed,
                total,
                tokens: 100,
            })
        }
    }

    fn held(
        before: Vec<bool>,
        after: Vec<bool>,
    ) -> impl Fn(Option<&Tweak>) -> Result<Vec<bool>, CliError> {
        move |tweak: Option<&Tweak>| {
            Ok(match tweak {
                None => before.clone(),
                Some(_) => after.clone(),
            })
        }
    }

    #[test]
    fn a_tweak_that_clearly_helps_on_many_tasks_is_kept_with_its_evidence() {
        // 40 of 100 before, 80 of 100 after: well beyond chance.
        let score = scorer(40, 100, vec![80]);
        let held_out = held(vec![true, true, false], vec![true, true, true]);
        let improvement =
            improve(&[words("think step by step")], 1_000, &score, &held_out).unwrap();
        assert_eq!(improvement.kept.len(), 1);
        assert!(improvement.turned_down.is_empty());
        let kept = &improvement.kept[0];
        assert_eq!(kept.before.passed, 40);
        assert_eq!(kept.after.passed, 80);
        assert!(kept.interval.0 > 0.0, "{:?}", kept.interval);
        assert_eq!(kept.held_out.regression(), None);
        assert_eq!(kept.at, 1_000);
        let lines = improvement.lines();
        assert!(lines[1].contains("kept its instructions"), "{lines:?}");
        assert!(
            lines[1].contains("80/100 passed, up from 40/100"),
            "{lines:?}"
        );
    }

    #[test]
    fn a_tweak_that_only_looks_better_on_a_few_tasks_is_turned_down() {
        // 3 of 5 before, 4 of 5 after: not beyond chance.
        let score = scorer(3, 5, vec![4]);
        let held_out = held(vec![true], vec![true]);
        let improvement = improve(&[words("be terse")], 1, &score, &held_out).unwrap();
        assert!(improvement.kept.is_empty());
        assert_eq!(improvement.turned_down.len(), 1);
        assert!(
            improvement.turned_down[0]
                .why
                .contains("not better beyond chance"),
            "{:?}",
            improvement.turned_down[0]
        );
        assert!(improvement
            .lines()
            .iter()
            .any(|l| l.contains("the agent is left as it is")));
    }

    #[test]
    fn a_tweak_that_regresses_on_tasks_it_never_saw_is_turned_down() {
        // Clearly better on the tasks tried…
        let score = scorer(40, 100, vec![80]);
        // …but it loses a held-out task the agent passes.
        let held_out = held(vec![true, true], vec![true, false]);
        let improvement = improve(&[words("skip the checks")], 1, &score, &held_out).unwrap();
        assert!(improvement.kept.is_empty());
        assert_eq!(improvement.turned_down.len(), 1);
        assert!(
            improvement.turned_down[0]
                .why
                .contains("fails held-out task 2"),
            "{:?}",
            improvement.turned_down[0]
        );
    }

    #[test]
    fn several_proposals_are_each_weighed_against_the_agent_as_it_is() {
        let score = scorer(40, 100, vec![80, 42, 75]);
        let held_out = held(vec![true], vec![true]);
        let proposals = [
            words("much better"),
            words("barely different"),
            Tweak::Route {
                task: "code".into(),
                models: vec!["anthropic:a".into(), "openai:b".into()],
            },
        ];
        let improvement = improve(&proposals, 1, &score, &held_out).unwrap();
        assert_eq!(improvement.kept.len(), 2, "{:?}", improvement);
        assert_eq!(improvement.turned_down.len(), 1);
        assert!(improvement.kept[1]
            .tweak
            .describe()
            .contains("the models for code"));
    }

    #[test]
    fn with_no_tasks_there_is_nothing_to_measure() {
        let score = scorer(0, 0, vec![0]);
        let held_out = held(Vec::new(), Vec::new());
        assert!(improve(&[words("x")], 1, &score, &held_out).is_err());
    }

    #[test]
    fn a_way_of_working_is_described_and_written_as_its_name() {
        let tweak = Tweak::Working {
            arm: Arm::Vote { samples: 3 },
        };
        assert!(tweak.describe().contains("vote-3"));
        let text = serde_json::to_string(&tweak).unwrap();
        let back: Tweak = serde_json::from_str(&text).unwrap();
        assert_eq!(back, tweak);
    }
}
