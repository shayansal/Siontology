//! Projects an agent owns, running for weeks: a goal, a budget, the
//! milestones it is working towards, and a check-in that reports to you.
//!
//! A work item ([`crate::work`]) is one piece of work an agent takes on and
//! finishes. A project is the longer thing: a goal it keeps returning to
//! over weeks, with money it may spend on the way, milestones to work
//! towards, and a standing arrangement to tell you where it stands
//! ([`Project::check_in`]). The agent writes to the project's log as it
//! works ([`Project::note`]); a check-in reports what it has done since the
//! last one, and nothing is lost if one is missed.
//!
//! A project is a place to keep a long task honest, not new permission: the
//! agent still acts under its charter, its autonomy level and its budget.
//! A project that has run past its end, spent its budget or been stopped
//! takes no more work ([`Project::may_work`]), which is what keeps a
//! long-running agent from wandering on indefinitely.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use sion_agent::schedule::When;
use sion_core::Amount;

use crate::error::CliError;

/// Where a project stands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// Being worked on.
    #[default]
    Running,
    /// Set aside, to be taken up again.
    Paused,
    /// Its goal reached, as the person says.
    Done,
    /// Given up.
    Stopped,
}

/// One thing a project is working towards.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Milestone {
    /// What it is.
    pub what: String,
    /// When it was reached, if it has been (seconds since the epoch).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reached: Option<u64>,
}

/// Something the project did, or something that happened to it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// When (seconds since the epoch).
    pub at: u64,
    /// What, in a line.
    pub what: String,
    /// What it spent on it, if anything.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spent: Option<Amount>,
}

/// A project an agent owns.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    /// Its name, unique to the profile.
    pub name: String,
    /// The profile whose agent owns it.
    pub profile: String,
    /// What it is for.
    pub goal: String,
    /// The USD it may spend in all.
    pub budget: Amount,
    /// What it has spent.
    #[serde(default)]
    pub spent: Amount,
    /// When it reports where it stands.
    pub check_in: When,
    /// The last time it reported (seconds since the epoch).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reported_at: Option<u64>,
    /// Where its reports go (a channel, as a schedule's).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deliver: Option<String>,
    /// When it ends, whatever it has reached (seconds since the epoch).
    pub until: u64,
    /// Where it stands.
    #[serde(default)]
    pub state: State,
    /// What it is working towards.
    #[serde(default)]
    pub milestones: Vec<Milestone>,
    /// What it has done, oldest first.
    #[serde(default)]
    pub log: Vec<Entry>,
    /// When it was started.
    pub started_at: u64,
}

/// The most entries a project's log keeps: the oldest go once it is full,
/// so a project that runs for months does not grow without end.
pub const MAX_LOG: usize = 2_000;

/// Why a project may not be worked on now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Closed {
    /// It is not running.
    NotRunning(State),
    /// Its time is up.
    Ended,
    /// Its budget is spent.
    Spent,
}

impl Closed {
    /// A line saying why.
    pub fn describe(&self) -> String {
        match self {
            Closed::NotRunning(state) => format!("it is {state:?}, not running").to_lowercase(),
            Closed::Ended => "its time is up".into(),
            Closed::Spent => "its budget is spent".into(),
        }
    }
}

impl Project {
    /// A project just started.
    pub fn new(
        name: &str,
        profile: &str,
        goal: &str,
        budget: Amount,
        check_in: When,
        until: u64,
        now: u64,
    ) -> Project {
        Project {
            name: name.to_string(),
            profile: profile.to_string(),
            goal: goal.to_string(),
            budget,
            spent: Amount::ZERO,
            check_in,
            reported_at: None,
            deliver: None,
            until,
            state: State::Running,
            milestones: Vec::new(),
            log: Vec::new(),
            started_at: now,
        }
    }

    /// Why it may not be worked on at `now`, if it may not.
    pub fn may_work(&self, now: u64) -> Result<(), Closed> {
        if self.state != State::Running {
            return Err(Closed::NotRunning(self.state));
        }
        if now > self.until {
            return Err(Closed::Ended);
        }
        if self.left().micros() == 0 {
            return Err(Closed::Spent);
        }
        Ok(())
    }

    /// The USD it has left to spend.
    pub fn left(&self) -> Amount {
        Amount(self.budget.micros().saturating_sub(self.spent.micros()))
    }

    /// Notes `what` it did at `now`, having spent `spent`: refused if the
    /// spending would take it past its budget, so the budget holds whatever
    /// the agent tries.
    pub fn note(&mut self, now: u64, what: &str, spent: Option<Amount>) -> Result<(), CliError> {
        if let Some(spent) = spent {
            if spent.micros() > self.left().micros() {
                return Err(CliError::Usage(format!(
                    "project {} has {} USD left, not {spent}",
                    self.name,
                    self.left()
                )));
            }
            self.spent = Amount(self.spent.micros().saturating_add(spent.micros()));
        }
        self.log.push(Entry {
            at: now,
            what: what.to_string(),
            spent,
        });
        if self.log.len() > MAX_LOG {
            let over = self.log.len() - MAX_LOG;
            self.log.drain(..over);
        }
        Ok(())
    }

    /// Marks the milestone whose text is `what` reached at `now`; adds it
    /// if the project does not have it.
    pub fn reached(&mut self, now: u64, what: &str) {
        match self.milestones.iter_mut().find(|m| m.what == what) {
            Some(milestone) => milestone.reached = Some(now),
            None => self.milestones.push(Milestone {
                what: what.to_string(),
                reached: Some(now),
            }),
        }
    }

    /// The milestones reached, of all of them.
    pub fn progress(&self) -> (usize, usize) {
        (
            self.milestones
                .iter()
                .filter(|m| m.reached.is_some())
                .count(),
            self.milestones.len(),
        )
    }

    /// When it next reports, after the last one (or its start).
    pub fn next_check_in(&self) -> Option<u64> {
        self.check_in
            .next_after(self.reported_at.unwrap_or(self.started_at))
    }

    /// Whether it is due to report at `now`.
    pub fn due(&self, now: u64) -> bool {
        self.state == State::Running && self.next_check_in().is_some_and(|at| at <= now)
    }

    /// What it has done since it last reported.
    pub fn since_report(&self) -> &[Entry] {
        let from = self.reported_at.unwrap_or(0);
        let at = self
            .log
            .iter()
            .position(|entry| entry.at > from)
            .unwrap_or(self.log.len());
        &self.log[at..]
    }

    /// Its report at `now`: where it stands, what it did since the last
    /// one, and what is left. Nothing since the last report is said so,
    /// rather than left out.
    pub fn report(&self, now: u64) -> String {
        let (reached, all) = self.progress();
        let mut lines = vec![
            format!("{} — {}", self.name, self.goal),
            format!(
                "  {:?}, {} of {} milestones reached, {} of {} USD spent",
                self.state, reached, all, self.spent, self.budget
            ),
        ];
        let since = self.since_report();
        if since.is_empty() {
            lines.push("  nothing since the last report".into());
        } else {
            lines.push(format!("  since the last report ({}):", since.len()));
            for entry in since {
                lines.push(format!("    {}", entry.what));
            }
        }
        for milestone in self.milestones.iter().filter(|m| m.reached.is_none()) {
            lines.push(format!("  still to do: {}", milestone.what));
        }
        if let Err(closed) = self.may_work(now) {
            lines.push(format!("  it takes no more work: {}", closed.describe()));
        } else if let Some(next) = self.next_check_in() {
            lines.push(format!(
                "  the next report is due at {}",
                sion_agent::schedule::rfc3339(next)
            ));
        }
        lines.join("\n")
    }

    /// Marks it reported at `now`.
    pub fn reported(&mut self, now: u64) {
        self.reported_at = Some(now);
    }
}

/// Where a profile's projects are kept.
pub fn path(profile: &str) -> PathBuf {
    crate::model_routes::dir().join(format!("{profile}-projects.json"))
}

/// A profile's projects, by name.
pub fn all(profile: &str) -> Result<BTreeMap<String, Project>, CliError> {
    let path = path(profile);
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text)
            .map_err(|e| CliError::Usage(format!("{}: {e}", path.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(e) => Err(CliError::Usage(format!("{}: {e}", path.display()))),
    }
}

/// Keeps `projects` as the profile's, written whole and renamed into
/// place, so a crash never leaves half a file.
pub fn keep(profile: &str, projects: &BTreeMap<String, Project>) -> Result<(), CliError> {
    let path = path(profile);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| CliError::Usage(e.to_string()))?;
    }
    let text = serde_json::to_string_pretty(projects)
        .map_err(|e| CliError::Usage(format!("the projects could not be written: {e}")))?;
    let partial = path.with_extension("json.partial");
    std::fs::write(&partial, text).map_err(|e| CliError::Usage(e.to_string()))?;
    std::fs::rename(&partial, &path).map_err(|e| CliError::Usage(e.to_string()))
}

/// The projects of `profile` due to report at `now`.
pub fn due(profile: &str, now: u64) -> Result<Vec<Project>, CliError> {
    Ok(all(profile)?
        .into_values()
        .filter(|project| project.due(now))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(budget: u64, until: u64) -> Project {
        Project::new(
            "rewrite",
            "lead",
            "rewrite the parser and ship it",
            Amount::units(budget),
            When::Every {
                seconds: 7 * 24 * 3_600,
                from: 1_000,
            },
            until,
            1_000,
        )
    }

    #[test]
    fn a_project_takes_no_work_once_its_time_or_budget_is_gone() {
        let mut p = project(100, 10_000);
        assert_eq!(p.may_work(2_000), Ok(()));
        // Past its end.
        assert_eq!(p.may_work(10_001), Err(Closed::Ended));
        // Its budget spent.
        p.note(2_000, "paid a contractor", Some(Amount::units(100)))
            .unwrap();
        assert_eq!(p.may_work(2_000), Err(Closed::Spent));
        assert_eq!(p.left(), Amount::ZERO);
        // Stopped.
        let mut p = project(100, 10_000);
        p.state = State::Stopped;
        assert_eq!(p.may_work(2_000), Err(Closed::NotRunning(State::Stopped)));
        assert!(p
            .may_work(2_000)
            .unwrap_err()
            .describe()
            .contains("not running"));
    }

    #[test]
    fn a_budget_holds_whatever_the_agent_tries_to_spend() {
        let mut p = project(10, 10_000);
        p.note(1_100, "bought a key", Some(Amount::units(4)))
            .unwrap();
        // More than is left: refused, and nothing is spent or logged.
        let refused = p.note(1_200, "bought another", Some(Amount::units(7)));
        assert!(refused.is_err());
        assert_eq!(p.spent, Amount::units(4));
        assert_eq!(p.log.len(), 1);
        // Exactly what is left: allowed.
        p.note(1_300, "the rest", Some(Amount::units(6))).unwrap();
        assert_eq!(p.left(), Amount::ZERO);
    }

    #[test]
    fn a_report_says_what_happened_since_the_last_one() {
        let mut p = project(100, 10_000);
        p.milestones = vec![
            Milestone {
                what: "the parser passes its tests".into(),
                reached: None,
            },
            Milestone {
                what: "it is shipped".into(),
                reached: None,
            },
        ];
        p.note(1_100, "read the old parser", None).unwrap();
        p.note(1_200, "wrote the new one", None).unwrap();
        p.reached(1_250, "the parser passes its tests");
        let first = p.report(1_300);
        assert!(first.contains("rewrite the parser"), "{first}");
        assert!(first.contains("1 of 2 milestones"), "{first}");
        assert!(first.contains("read the old parser"), "{first}");
        assert!(first.contains("still to do: it is shipped"), "{first}");
        p.reported(1_300);
        // Nothing since: said so, not left out.
        let quiet = p.report(1_400);
        assert!(quiet.contains("nothing since the last report"), "{quiet}");
        // New work shows in the next report, the old does not.
        p.note(1_500, "shipped it", None).unwrap();
        let next = p.report(1_600);
        assert!(next.contains("shipped it"), "{next}");
        assert!(!next.contains("read the old parser"), "{next}");
    }

    #[test]
    fn a_check_in_falls_due_and_moves_on_once_reported() {
        let mut p = project(100, 10_000_000);
        let week = 7 * 24 * 3_600;
        assert!(!p.due(1_000 + week - 1));
        assert!(p.due(1_000 + week));
        p.reported(1_000 + week);
        // The next one is a week after the report, not after the start.
        assert!(!p.due(1_000 + week + 1));
        assert!(p.due(1_000 + 2 * week));
        // A project not running is never due.
        p.state = State::Paused;
        assert!(!p.due(1_000 + 10 * week));
    }

    #[test]
    fn a_long_log_keeps_its_newest_entries() {
        let mut p = project(100, u64::MAX);
        for i in 0..MAX_LOG + 50 {
            p.note(2_000 + i as u64, &format!("step {i}"), None)
                .unwrap();
        }
        assert_eq!(p.log.len(), MAX_LOG);
        assert_eq!(p.log[0].what, "step 50");
        assert_eq!(p.log[MAX_LOG - 1].what, format!("step {}", MAX_LOG + 49));
    }

    #[test]
    fn a_milestone_reached_twice_keeps_the_later_time() {
        let mut p = project(100, 10_000);
        p.reached(1_100, "shipped");
        assert_eq!(p.progress(), (1, 1));
        p.reached(1_200, "shipped");
        assert_eq!(p.progress(), (1, 1));
        assert_eq!(p.milestones[0].reached, Some(1_200));
    }

    #[test]
    fn projects_are_written_and_read_back() {
        let p = project(100, 10_000);
        let mut projects = BTreeMap::new();
        projects.insert(p.name.clone(), p.clone());
        let text = serde_json::to_string(&projects).unwrap();
        let back: BTreeMap<String, Project> = serde_json::from_str(&text).unwrap();
        assert_eq!(back["rewrite"], p);
    }
}
