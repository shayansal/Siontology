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
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sion_agent::schedule::When;
use sion_agent::tools::{Effect, Outcome as ToolOutcome, Tool};
use sion_agent::ToolSpec;
use sion_core::Amount;

use crate::error::CliError;
use crate::runtime::SharedLedger;

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
    /// Whether, at each check-in, its agent works on it once before the
    /// report is made.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub returns: bool,
    /// When a report was last made and could not be sent: it is made
    /// again at the next check-in after, saying all since the last one sent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tried_at: Option<u64>,
    /// Payments held against its budget whose outcome is not yet known, by
    /// the call that made each: given back if it turns out not to have
    /// been made, and never held twice for one call.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub holds: BTreeMap<String, Amount>,
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
            returns: false,
            tried_at: None,
            holds: BTreeMap::new(),
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

    /// When it next reports, after the last one (or its start), or after
    /// the last that could not be sent.
    pub fn next_check_in(&self) -> Option<u64> {
        let after = self
            .reported_at
            .unwrap_or(self.started_at)
            .max(self.tried_at.unwrap_or(0));
        self.check_in.next_after(after)
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

    /// Holds `amount` for the payment the call `key` is about to make
    /// (`what`), at `now`: refused if it may not be worked on or has less
    /// left; a call held already is not held twice ([`hold`]).
    pub fn hold(
        &mut self,
        now: u64,
        amount: Amount,
        what: &str,
        key: &str,
    ) -> Result<(), CliError> {
        if self.holds.contains_key(key) {
            return Ok(());
        }
        if let Err(closed) = self.may_work(now) {
            return Err(CliError::Usage(format!(
                "project {} takes no more work: {}",
                self.name,
                closed.describe()
            )));
        }
        self.note(now, &format!("paid {what}"), Some(amount))?;
        self.holds.insert(key.to_string(), amount);
        Ok(())
    }

    /// The call `key`'s payment was made: what it held stays spent.
    pub fn settle(&mut self, key: &str) {
        self.holds.remove(key);
    }

    /// The call `key`'s payment was not made (`why`): what it held is
    /// given back, once.
    pub fn release(&mut self, now: u64, key: &str, why: &str) -> Result<(), CliError> {
        let Some(amount) = self.holds.remove(key) else {
            return Ok(());
        };
        self.spent = Amount(self.spent.micros().saturating_sub(amount.micros()));
        self.note(
            now,
            &format!("{amount} USD not paid after all: {why}"),
            None,
        )
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

/// Seconds since the epoch, now.
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

/// The project a session works on ([`crate::runtime::Saved::project`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectLink {
    /// The profile whose project it is.
    pub profile: String,
    /// The project's name.
    pub name: String,
}

/// Projects are read and changed one change at a time: across this
/// process's threads by this, and across processes by a lock file beside
/// the projects' file ([`crate::pairing::Held`]).
static CHANGING: Mutex<()> = Mutex::new(());

/// Takes both locks on `profile`'s projects, for as long as what it
/// returns is kept.
fn locked(
    profile: &str,
) -> Result<(std::sync::MutexGuard<'static, ()>, crate::pairing::Held), CliError> {
    let one = CHANGING.lock().unwrap_or_else(|e| e.into_inner());
    let held = crate::pairing::Held::take(&path(profile).with_extension("lock"))?;
    Ok((one, held))
}

/// The project `link` names.
pub fn get(link: &ProjectLink) -> Result<Project, CliError> {
    all(&link.profile)?.remove(&link.name).ok_or_else(|| {
        CliError::Usage(format!(
            "profile {} has no project {}",
            link.profile, link.name
        ))
    })
}

/// Changes `link`'s project with `change`, read and written whole under
/// one lock: kept only if `change` succeeds.
pub fn change<T>(
    link: &ProjectLink,
    change: impl FnOnce(&mut Project) -> Result<T, CliError>,
) -> Result<T, CliError> {
    let _locked = locked(&link.profile)?;
    let mut projects = all(&link.profile)?;
    let project = projects.get_mut(&link.name).ok_or_else(|| {
        CliError::Usage(format!(
            "profile {} has no project {}",
            link.profile, link.name
        ))
    })?;
    let out = change(project)?;
    keep(&link.profile, &projects)?;
    Ok(out)
}

/// Holds `amount` of `link`'s budget for a payment about to be sent by
/// the call `key` (`what`, in a line), at `now`: refused if the project may
/// not be worked on, or has less left. The payment is in its log from then
/// on, and the call's hold is kept until [`settle`] or [`release`]; a call
/// held already (made again) is not held twice.
pub fn hold(
    link: &ProjectLink,
    now: u64,
    amount: Amount,
    what: &str,
    key: &str,
) -> Result<(), CliError> {
    change(link, |project| project.hold(now, amount, what, key))
}

/// The call `key`'s payment was made: what it held stays spent.
pub fn settle(link: &ProjectLink, key: &str) -> Result<(), CliError> {
    change(link, |project| {
        project.settle(key);
        Ok(())
    })
}

/// The call `key`'s payment was not made after all (`why`): what it held
/// is given back, once.
pub fn release(link: &ProjectLink, now: u64, key: &str, why: &str) -> Result<(), CliError> {
    change(link, |project| project.release(now, key, why))
}

/// The tools a project's session works with: `project_note` and
/// `project_milestone`.
pub fn tools(link: &ProjectLink) -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(NoteTool { link: link.clone() }),
        Box::new(MilestoneTool { link: link.clone() }),
    ]
}

/// What a project tool's change came to, as its outcome.
fn outcome(changed: Result<String, CliError>) -> ToolOutcome {
    match changed {
        Ok(text) => ToolOutcome::ok(text),
        Err(e) => ToolOutcome::error(e.to_string()),
    }
}

/// `project_note`.
#[derive(Debug)]
pub struct NoteTool {
    /// The project it notes in.
    pub link: ProjectLink,
}

impl Tool for NoteTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "project_note".into(),
            description: format!(
                "Record in project {}'s log what you did for it, in a line: its next report \
                 says what was recorded since the last one. Payments you make for it are \
                 recorded already.",
                self.link.name
            ),
            input_schema: json!({"type": "object",
                "properties": {"what": {"type": "string", "description": "what you did, in a line"}},
                "required": ["what"]}),
        }
    }

    /// It writes in the project's log, on this machine.
    fn effect(&self) -> Effect {
        Effect::Write
    }

    fn call(&self, input: &Value, _key: &str) -> ToolOutcome {
        let what = input["what"]
            .as_str()
            .unwrap_or_default()
            .trim()
            .to_string();
        if what.is_empty() {
            return ToolOutcome::error("say what you did in `what`");
        }
        let at = now();
        outcome(change(&self.link, |project| {
            if let Err(closed) = project.may_work(at) {
                return Err(CliError::Usage(format!(
                    "project {} takes no more work: {}; stop working on it",
                    project.name,
                    closed.describe()
                )));
            }
            project.note(at, &what, None)?;
            Ok(format!("Recorded in project {}.", project.name))
        }))
    }
}

/// `project_milestone`.
#[derive(Debug)]
pub struct MilestoneTool {
    /// The project whose milestone it marks.
    pub link: ProjectLink,
}

impl Tool for MilestoneTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "project_milestone".into(),
            description: format!(
                "Mark a milestone of project {} reached, by its words as the task lists it; \
                 only once it is.",
                self.link.name
            ),
            input_schema: json!({"type": "object",
                "properties": {"what": {"type": "string", "description": "the milestone, as listed"}},
                "required": ["what"]}),
        }
    }

    /// It writes in the project, on this machine.
    fn effect(&self) -> Effect {
        Effect::Write
    }

    fn call(&self, input: &Value, _key: &str) -> ToolOutcome {
        let what = input["what"]
            .as_str()
            .unwrap_or_default()
            .trim()
            .to_string();
        if what.is_empty() {
            return ToolOutcome::error("name the milestone in `what`");
        }
        let at = now();
        outcome(change(&self.link, |project| {
            project.reached(at, &what);
            let (reached, all) = project.progress();
            Ok(format!(
                "Milestone reached: {what} ({reached} of {all} in project {}).",
                project.name
            ))
        }))
    }
}

/// What the agent is asked when it works on `project` at `now`: its goal,
/// what it has left, its milestones, what was done lately, and `steer`,
/// what to work on now, if given.
pub fn task_for(project: &Project, now: u64, steer: Option<&str>) -> String {
    let mut lines = vec![
        format!(
            "You own a project, {}. Its goal: {}",
            project.name, project.goal
        ),
        format!(
            "It has {} USD left of its budget of {} USD, and runs until {}.",
            project.left(),
            project.budget,
            sion_agent::schedule::rfc3339(project.until)
        ),
    ];
    let open: Vec<&Milestone> = project
        .milestones
        .iter()
        .filter(|m| m.reached.is_none())
        .collect();
    if !open.is_empty() {
        lines.push("Milestones still to reach:".into());
        lines.extend(open.iter().map(|m| format!("- {}", m.what)));
    }
    let done: Vec<&Milestone> = project
        .milestones
        .iter()
        .filter(|m| m.reached.is_some())
        .collect();
    if !done.is_empty() {
        lines.push("Milestones reached:".into());
        lines.extend(done.iter().map(|m| format!("- {}", m.what)));
    }
    let from = project.log.len().saturating_sub(20);
    if project.log.is_empty() {
        lines.push("Nothing has been done for it yet.".into());
    } else {
        lines.push("What was done for it lately, oldest first:".into());
        lines.extend(project.log[from..].iter().map(|entry| {
            format!(
                "- {}: {}",
                sion_agent::schedule::rfc3339(entry.at),
                entry.what
            )
        }));
    }
    if let Some(steer) = steer.map(str::trim).filter(|s| !s.is_empty()) {
        lines.push(format!("What to work on now: {steer}"));
    }
    lines.push(format!(
        "Work towards the goal within your charter. Record what you do with project_note, and \
         mark a milestone with project_milestone once it is reached. A payment you make counts \
         against the project's budget: one past what it has left is refused. Stop when you \
         have done what can be done now, and say what you did. (Asked at {}.)",
        sion_agent::schedule::rfc3339(now)
    ));
    lines.join("\n")
}

/// Has `profile`'s agent work on project `link` once, on `steer` if given:
/// refused when the project may not be worked on. The session is noted in
/// the project's log; returns the lines its run shows.
pub fn work(
    ledger: SharedLedger,
    profile: &crate::profiles::Profile,
    link: &ProjectLink,
    steer: Option<&str>,
) -> Result<Vec<String>, CliError> {
    let at = now();
    let project = get(link)?;
    if let Err(closed) = project.may_work(at) {
        return Err(CliError::Usage(format!(
            "project {} takes no more work: {}",
            link.name,
            closed.describe()
        )));
    }
    let task = task_for(&project, at, steer);
    let (saved, shown) = profile.start_project(
        ledger,
        &task,
        link.clone(),
        crate::runtime::Hooks::default(),
    )?;
    let said: String = crate::runtime::last_words(&saved)
        .lines()
        .next()
        .unwrap_or_default()
        .chars()
        .take(200)
        .collect();
    let id = &saved.state.id[..saved.state.id.len().min(8)];
    change(link, |project| {
        project.note(now(), &format!("session {id}: {said}"), None)
    })?;
    Ok(shown)
}

/// Makes `link`'s report as at `now` and sends it where its reports go:
/// marked reported once it is sent (or, with nowhere to send it, once it
/// is made). One that could not be sent is made again at the next
/// check-in, saying all since the last one sent. Returns what to print.
pub fn report(link: &ProjectLink, now: u64) -> Result<Vec<String>, CliError> {
    let project = get(link)?;
    let text = project.report(now);
    // Marked reported as of the newest entry it says, if that is later:
    // an entry made since is in the next report, and none twice.
    let newest = project.log.iter().map(|entry| entry.at).max().unwrap_or(0);
    let mut lines = vec![text.clone()];
    let sent = match &project.deliver {
        Some(channel) => {
            let message = json!({
                "project": link.name,
                "profile": link.profile,
                "at": sion_agent::schedule::rfc3339(now),
                "said": text,
            });
            match crate::schedules::deliver(channel, &message) {
                Ok(()) => {
                    lines.push(format!("(sent to {channel})"));
                    true
                }
                Err(e) => {
                    lines.push(format!("(not sent to {channel}: {e})"));
                    false
                }
            }
        }
        None => true,
    };
    change(link, |project| {
        if sent {
            project.reported(now.max(newest));
            project.tried_at = None;
        } else {
            project.tried_at = Some(now);
        }
        Ok(())
    })?;
    Ok(lines)
}

/// Every project of every profile due to report at `now`: one that
/// `returns` is worked on once first, if it may be; then its report is
/// made and sent. What `sion agent scheduler` does as it runs. Returns
/// what to print.
pub fn check_ins(ledger: SharedLedger, now: u64) -> Result<Vec<String>, CliError> {
    let mut lines = Vec::new();
    for profile in crate::profiles::all()?.into_values() {
        for project in due(&profile.name, now)? {
            let link = ProjectLink {
                profile: profile.name.clone(),
                name: project.name.clone(),
            };
            if project.returns && project.may_work(now).is_ok() {
                match work(ledger.clone(), &profile, &link, None) {
                    Ok(shown) => lines.extend(
                        shown
                            .into_iter()
                            .map(|line| format!("project {}: {line}", link.name)),
                    ),
                    Err(e) => lines.push(format!("project {}: the work failed: {e}", link.name)),
                }
            }
            // Reported as of after the work, so what the session did is
            // in this report, not again in the next.
            lines.extend(report(&link, now.max(self::now()))?);
        }
    }
    Ok(lines)
}

/// The soonest any project of any profile is due to report.
pub fn next_check_in() -> Option<u64> {
    let profiles = crate::profiles::all().ok()?;
    profiles
        .keys()
        .filter_map(|name| all(name).ok())
        .flat_map(|projects| projects.into_values())
        .filter(|project| project.state == State::Running)
        .filter_map(|project| project.next_check_in())
        .min()
}

/// `sion agent project …`
#[derive(Debug, clap::Subcommand)]
pub enum ProjectCommand {
    /// Start a project for a profile's agent: its goal, the USD its
    /// payments for it may come to, how long it runs, and how often it
    /// reports where it stands.
    Start {
        /// The profile whose agent owns it.
        profile: String,
        /// Its name (lowercase letters, digits and hyphens).
        name: String,
        /// What it is for, in words.
        #[arg(long)]
        goal: String,
        /// The USD it may spend in all.
        #[arg(long, value_name = "USD")]
        budget: String,
        /// How long it runs: 30d, 12h.
        #[arg(long = "for", value_name = "DURATION")]
        lasts: String,
        /// How often it reports where it stands: 1d, 12h.
        #[arg(long = "check-in", value_name = "INTERVAL", default_value = "1d")]
        check_in: String,
        /// A milestone to work towards (repeat for more).
        #[arg(long = "milestone", value_name = "TEXT")]
        milestones: Vec<String>,
        /// Where its reports go, as a schedule's do: file:<path>,
        /// webhook:<url>, telegram:<chat id>, or <messenger>:<chat>.
        #[arg(long)]
        deliver: Option<String>,
        /// At each check-in, have the agent work on it once before the
        /// report is made (with `sion agent scheduler` running).
        #[arg(long = "work-at-check-in")]
        work_at_check_in: bool,
    },
    /// A profile's projects, a line each.
    List {
        /// The profile.
        profile: String,
    },
    /// Where a project stands, as its next report would say it (without
    /// marking it reported).
    Show {
        /// The profile.
        profile: String,
        /// The project.
        name: String,
    },
    /// Have the agent work on a project now: one session on its goal, its
    /// milestones and what was done lately, with tools to record what it
    /// does; its payments are held to what the project has left.
    Work {
        /// The profile.
        profile: String,
        /// The project.
        name: String,
        /// What to work on now, in words.
        #[arg(long)]
        steer: Option<String>,
    },
    /// Note something done for a project, as its operator, with what it
    /// cost if it spent from the project's budget.
    Note {
        /// The profile.
        profile: String,
        /// The project.
        name: String,
        /// What was done, in a line.
        what: String,
        /// The USD it cost.
        #[arg(long, value_name = "USD")]
        spent: Option<String>,
    },
    /// Mark a milestone reached (added if the project did not list it).
    Reached {
        /// The profile.
        profile: String,
        /// The project.
        name: String,
        /// The milestone, as listed.
        milestone: String,
    },
    /// Make the report now and send it where the project's reports go.
    Report {
        /// The profile.
        profile: String,
        /// The project.
        name: String,
    },
    /// Set a project aside: it takes no work until it is resumed.
    Pause {
        /// The profile.
        profile: String,
        /// The project.
        name: String,
    },
    /// Take a paused project up again.
    Resume {
        /// The profile.
        profile: String,
        /// The project.
        name: String,
    },
    /// Mark a project's goal reached: it takes no more work.
    Done {
        /// The profile.
        profile: String,
        /// The project.
        name: String,
    },
    /// Give a project up: it takes no more work.
    Stop {
        /// The profile.
        profile: String,
        /// The project.
        name: String,
    },
}

impl ProjectCommand {
    /// Whether it needs the ledger: only an agent at work does.
    pub fn needs_ledger(&self) -> bool {
        matches!(self, ProjectCommand::Work { .. })
    }
}

/// Puts `link`'s project in `state`, from one of `from`.
fn set_state(link: &ProjectLink, state: State, from: &[State]) -> Result<(), CliError> {
    change(link, |project| {
        if !from.contains(&project.state) {
            return Err(CliError::Usage(
                format!("project {} is {:?}", project.name, project.state).to_lowercase(),
            ));
        }
        project.state = state;
        project.note(
            now(),
            &format!("{state:?} by its operator").to_lowercase(),
            None,
        )
    })?;
    println!(
        "project {} of profile {} is {}",
        link.name,
        link.profile,
        format!("{state:?}").to_lowercase()
    );
    Ok(())
}

/// Runs `sion agent project …` but `work`, which needs the ledger.
pub fn run_local(command: ProjectCommand) -> Result<(), CliError> {
    let link = |profile: &str, name: &str| ProjectLink {
        profile: profile.to_string(),
        name: name.to_string(),
    };
    match command {
        ProjectCommand::Start {
            profile,
            name,
            goal,
            budget,
            lasts,
            check_in,
            milestones,
            deliver,
            work_at_check_in,
        } => {
            crate::profiles::get(&profile)?;
            if !crate::profiles::valid_name(&name) {
                return Err(CliError::Usage(format!(
                    "{name:?} is not a project's name: 1 to 32 lowercase letters, digits and hyphens"
                )));
            }
            if goal.trim().is_empty() {
                return Err(CliError::Usage(
                    "say what the project is for: --goal".into(),
                ));
            }
            if let Some(channel) = &deliver {
                crate::schedules::check_channel(channel)?;
            }
            let budget = crate::agents::parse_amount(&budget)?;
            let lasts = sion_agent::schedule::parse_duration(&lasts).map_err(CliError::Usage)?;
            let every = sion_agent::schedule::parse_duration(&check_in).map_err(CliError::Usage)?;
            if every == 0 {
                return Err(CliError::Usage(
                    "a check-in comes every while: 1d, 12h".into(),
                ));
            }
            let at = now();
            let _locked = locked(&profile)?;
            let mut projects = all(&profile)?;
            if projects.contains_key(&name) {
                return Err(CliError::Usage(format!(
                    "profile {profile} has a project {name} already"
                )));
            }
            let mut project = Project::new(
                &name,
                &profile,
                goal.trim(),
                budget,
                When::Every {
                    seconds: every,
                    from: at,
                },
                at.saturating_add(lasts),
                at,
            );
            project.deliver = deliver;
            project.returns = work_at_check_in;
            for milestone in milestones {
                project.milestones.push(Milestone {
                    what: milestone,
                    reached: None,
                });
            }
            let next = project.next_check_in();
            projects.insert(name.clone(), project);
            keep(&profile, &projects)?;
            println!(
                "project {name} of profile {profile}: {budget} USD to spend, until {}; its first report is due at {}",
                sion_agent::schedule::rfc3339(at.saturating_add(lasts)),
                next.map(sion_agent::schedule::rfc3339).unwrap_or_default()
            );
            Ok(())
        }
        ProjectCommand::List { profile } => {
            let projects = all(&profile)?;
            if projects.is_empty() {
                println!("profile {profile} has no project (sion agent project start)");
            }
            let at = now();
            for project in projects.values() {
                let (reached, all) = project.progress();
                let standing = match project.may_work(at) {
                    Ok(()) => "running".to_string(),
                    Err(closed) => closed.describe(),
                };
                println!(
                    "{}: {standing}; {reached} of {all} milestones, {} of {} USD spent, until {}; {}",
                    project.name,
                    project.spent,
                    project.budget,
                    sion_agent::schedule::rfc3339(project.until),
                    project.goal
                );
            }
            Ok(())
        }
        ProjectCommand::Show { profile, name } => {
            println!("{}", get(&link(&profile, &name))?.report(now()));
            Ok(())
        }
        ProjectCommand::Note {
            profile,
            name,
            what,
            spent,
        } => {
            let spent = spent
                .as_deref()
                .map(crate::agents::parse_amount)
                .transpose()?;
            change(&link(&profile, &name), |project| {
                project.note(now(), what.trim(), spent)
            })?;
            println!("noted in project {name}");
            Ok(())
        }
        ProjectCommand::Reached {
            profile,
            name,
            milestone,
        } => {
            let (reached, all) = change(&link(&profile, &name), |project| {
                project.reached(now(), milestone.trim());
                Ok(project.progress())
            })?;
            println!("project {name}: {reached} of {all} milestones reached");
            Ok(())
        }
        ProjectCommand::Report { profile, name } => {
            for line in report(&link(&profile, &name), now())? {
                println!("{line}");
            }
            Ok(())
        }
        ProjectCommand::Pause { profile, name } => {
            set_state(&link(&profile, &name), State::Paused, &[State::Running])
        }
        ProjectCommand::Resume { profile, name } => {
            set_state(&link(&profile, &name), State::Running, &[State::Paused])
        }
        ProjectCommand::Done { profile, name } => set_state(
            &link(&profile, &name),
            State::Done,
            &[State::Running, State::Paused],
        ),
        ProjectCommand::Stop { profile, name } => set_state(
            &link(&profile, &name),
            State::Stopped,
            &[State::Running, State::Paused],
        ),
        ProjectCommand::Work { .. } => Err(CliError::Usage(
            "an agent works on a project against a node: sion agent project work runs with --node"
                .into(),
        )),
    }
}

/// Runs `sion agent project …`.
pub fn run(ledger: SharedLedger, command: ProjectCommand) -> Result<(), CliError> {
    match command {
        ProjectCommand::Work {
            profile,
            name,
            steer,
        } => {
            let found = crate::profiles::get(&profile)?;
            let link = ProjectLink { profile, name };
            for line in work(ledger, &found, &link, steer.as_deref())? {
                println!("{line}");
            }
            println!("{}", get(&link)?.report(now()));
            Ok(())
        }
        other => run_local(other),
    }
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

    #[test]
    fn a_report_that_could_not_be_sent_waits_for_the_next_check_in() {
        let week = 7 * 24 * 3_600;
        let mut p = project(100, 100 * week);
        p.note(2_000, "parsed the grammar", None).unwrap();
        let first = 1_000 + week;
        assert!(p.due(first));
        // Made at the check-in and not sent: not due again until the next
        // one, and the next report still says what the first could not.
        p.tried_at = Some(first + 10);
        assert!(!p.due(first + 60));
        assert_eq!(p.next_check_in(), Some(1_000 + 2 * week));
        assert!(p.due(1_000 + 2 * week));
        assert!(p.report(1_000 + 2 * week).contains("parsed the grammar"));
    }

    #[test]
    fn a_payment_is_held_once_for_its_call_and_given_back_once() {
        let mut p = project(10, 100_000);
        p.hold(2_000, Amount::units(4), "4 USD to bob: a review", "call-1")
            .unwrap();
        // The same call made again is not held twice.
        p.hold(2_001, Amount::units(4), "4 USD to bob: a review", "call-1")
            .unwrap();
        assert_eq!(p.spent, Amount::units(4));
        // Another that would pass the budget is refused before it is sent.
        p.hold(2_002, Amount::units(4), "4 USD to bob", "call-2")
            .unwrap();
        let refused = p
            .hold(2_003, Amount::units(4), "4 USD to carol", "call-3")
            .unwrap_err();
        assert!(
            refused.to_string().contains("has 2 USD left, not 4"),
            "{refused}"
        );
        // One made stays spent; one not made is given back, once.
        p.settle("call-1");
        p.release(2_004, "call-2", "refused by the ledger").unwrap();
        p.release(2_005, "call-2", "refused by the ledger").unwrap();
        p.release(2_006, "call-1", "too late").unwrap();
        assert_eq!(p.spent, Amount::units(4));
        assert!(p.holds.is_empty());
        assert!(p
            .log
            .iter()
            .any(|e| e.what == "4 USD not paid after all: refused by the ledger"));
        // A paused project holds nothing.
        p.state = State::Paused;
        assert!(p.hold(2_007, Amount::units(1), "1 USD", "call-4").is_err());
    }

    #[test]
    fn a_project_session_is_told_its_goal_budget_and_milestones() {
        let mut p = project(10, 1_000 + 30 * 24 * 3_600);
        p.milestones.push(Milestone {
            what: "Parser rewritten".into(),
            reached: None,
        });
        p.reached(3_000, "Grammar written");
        p.note(3_000, "wrote the grammar", Some(Amount::units(2)))
            .unwrap();
        let task = task_for(&p, 4_000, Some("the error messages"));
        assert!(task.contains("You own a project, rewrite."), "{task}");
        assert!(
            task.contains("It has 8 USD left of its budget of 10 USD"),
            "{task}"
        );
        assert!(
            task.contains("Milestones still to reach:\n- Parser rewritten"),
            "{task}"
        );
        assert!(
            task.contains("Milestones reached:\n- Grammar written"),
            "{task}"
        );
        assert!(task.contains("wrote the grammar"), "{task}");
        assert!(
            task.contains("What to work on now: the error messages"),
            "{task}"
        );
        assert!(task.contains("project_note"), "{task}");
    }
}
