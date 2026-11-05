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
    /// How the agent as it is did on the tasks tried.
    pub before: Outcome,
    /// The tweaks kept, in the order they were tried.
    pub kept: Vec<Kept>,
    /// The tweaks turned down, with the reason for each.
    pub turned_down: Vec<TurnedDown>,
}

impl Improvement {
    /// What it did, a line each.
    pub fn lines(&self) -> Vec<String> {
        let mut lines = vec![format!(
            "the agent as it is: {}/{} tasks passed",
            self.before.passed, self.before.total
        )];
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
