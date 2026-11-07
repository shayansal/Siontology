//! Tasks from the agent's own work, and a part of them sealed before any
//! change is tried, to confirm changes on.
//!
//! - **Marking.** `sion agent improve mark <session> --good` (or `--bad`)
//!   says how one of the agent's sessions went. A session run with a check
//!   (`--verify`) that finished has passed it, and needs no mark.
//! - **Harvesting.** The profile's sessions whose outcome is known become a
//!   set of tasks ([`sion_agent::harvest`]): the task each was asked, its
//!   tools as stand-ins that answer what they answered, and checks that a
//!   run does what the good ones did and none of what the bad ones did.
//! - **Sealing.** A set of tasks is split, by a draw from its own digest
//!   and a seed, into the tasks changes may be chosen on and the tasks
//!   they are confirmed on, which no proposer is shown. Canary cases
//!   ([`sion_agent::guard::canary`]) are added to the sealed part. A seal
//!   allows so many confirmations, each spending one, and the level each
//!   is held to is corrected for all of them (Bonferroni): the chance that
//!   any change it confirms is no better is at most 5%, for the changes it
//!   confirms. When its confirmations are spent, fresh tasks are sealed.
//! - **On the ledger.** With a wallet, a seal's digests are published as a
//!   claim before anything is tried on it ([`SEAL_SCHEMA`]), and each
//!   change it confirms as a claim that names the seal and the change it
//!   follows ([`STEP_SCHEMA`]). Anyone later shown the sealed tasks can
//!   check that they are the ones sealed, and that the seal came first;
//!   [`check_lineage`] does.
//!
//! What a seal does not do: keep the tasks secret from whoever holds this
//! machine's files, or stop the adaptivity of many confirmations on the
//! same tasks beyond the correction for their number — answers that steer
//! later changes leak a little of the sealed tasks each time, which is why
//! a seal's confirmations are few and fresh tasks are sealed after them.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use sion_agent::compare::TaskSet;
use sion_agent::evals::Case;
use sion_agent::harvest::{self, Verdict};
use sion_assurance::Formula;
use sion_core::ids::{ClaimId, ObjectId};
use sion_exec::claim::{formulas, ClaimBody, Payload};

use crate::error::CliError;
use crate::ledger::Ledger;
use crate::session::Session;

/// The schema of a claim that seals tasks for an agent's improvement.
pub const SEAL_SCHEMA: &str = "agent.seal/v1";

/// The schema of a claim that an agent kept a change a seal confirmed.
pub const STEP_SCHEMA: &str = "agent.improvement/v1";

/// The Pulses a seal's or a step's claim stands for.
pub const VALID_FOR: u64 = 1_000_000;

/// The confirmations a seal allows by default.
pub const BUDGET: u32 = 8;

/// The share of tasks sealed by default, in percent.
pub const SHARE: u32 = 30;

/// Canary cases added to a seal by default.
pub const CANARIES: usize = 2;

/// The chance, over all the confirmations a seal allows, of confirming a
/// change that is no better.
pub const ALPHA: f64 = 0.05;

fn io(path: &std::path::Path) -> impl Fn(std::io::Error) -> CliError + '_ {
    move |e| CliError::Usage(format!("{}: {e}", path.display()))
}

fn dir() -> PathBuf {
    crate::model_routes::dir()
}

/// Where the marks of sessions are kept.
pub fn marks_path() -> PathBuf {
    dir().join("marks.json")
}

/// How a person said a session went.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mark {
    /// It went well.
    pub good: bool,
    /// What they said of it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    /// When (seconds since the epoch).
    pub at: u64,
}

/// Every session's mark, by session id.
pub fn marks() -> Result<BTreeMap<String, Mark>, CliError> {
    let path = marks_path();
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text)
            .map_err(|e| CliError::Usage(format!("{}: {e}", path.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(e) => Err(io(&path)(e)),
    }
}

/// Marks session `id` (or a unique prefix of it) as having gone well or
/// badly. Returns the session's full id.
pub fn mark(id: &str, good: bool, note: &str, now: u64) -> Result<String, CliError> {
    let saved = crate::runtime::Saved::load(id)?;
    let id = saved.state.id.clone();
    let path = marks_path();
    let _held = crate::pairing::Held::take(&path.with_extension("lock"))?;
    let mut all = marks()?;
    all.insert(
        id.clone(),
        Mark {
            good,
            note: note.to_string(),
            at: now,
        },
    );
    crate::profiles::write_json(&path, &all)?;
    Ok(id)
}

/// A set of tasks from `profile`'s sessions whose outcome is known: a mark
/// says how it went, or a check it was run with passed. Returns the set
/// (none if no session gave a case) and, for each session passed over,
/// why.
pub fn harvest(
    profile: &crate::profiles::Profile,
) -> Result<(Option<TaskSet>, Vec<String>), CliError> {
    let marks = marks()?;
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
    let mut cases: Vec<Case> = Vec::new();
    let mut passed_over = Vec::new();
    for id in ids {
        let Ok(saved) = crate::runtime::Saved::load(&id) else {
            continue;
        };
        if saved.wallet != profile.wallet || saved.agent != profile.agent {
            continue;
        }
        let checked =
            saved.verify.is_some() && matches!(saved.ended, Some(sion_agent::Ended::Done { .. }));
        let verdict = match marks.get(&saved.state.id) {
            Some(mark) if mark.good => Verdict::Good,
            Some(_) => Verdict::Bad,
            None if checked => Verdict::Good,
            None => continue,
        };
        let short: String = saved.state.id.chars().take(12).collect();
        let task = match harvest::task_of(&saved.state) {
            Ok(task) => task,
            Err(why) => {
                passed_over.push(format!("session {short}: {why}"));
                continue;
            }
        };
        let calls = harvest::recorded(&saved.state);
        let name = format!(
            "{}-{short}",
            if verdict == Verdict::Good {
                "good"
            } else {
                "bad"
            }
        );
        match harvest::case_of(&name, &task, &calls, verdict) {
            Ok(case) => cases.push(case),
            Err(why) => passed_over.push(format!("session {short}: {why}")),
        }
    }
    if cases.is_empty() {
        return Ok((None, passed_over));
    }
    cases.truncate(sion_agent::evals::MAX_CASES);
    let set = TaskSet {
        name: format!("{}-work", set_name(&profile.name)),
        about: format!(
            "tasks from {}'s own sessions whose outcome is known",
            profile.name
        ),
        system: None,
        cases,
    };
    Ok((Some(set), passed_over))
}

/// `name` as a set's name: lowercase letters, digits and hyphens.
fn set_name(name: &str) -> String {
    let mut out: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    out.truncate(48);
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        "agent".into()
    } else {
        out
    }
}

/// A seal: tasks split into those changes may be chosen on and those they
/// are confirmed on, and how many confirmations it allows.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Seal {
    /// The profile it is for.
    pub profile: String,
    /// The profile's agent.
    pub agent: ObjectId,
    /// The digest of the set of tasks as given, which the split was drawn
    /// from.
    pub source: String,
    /// The seed mixed into the split.
    pub seed: u64,
    /// The digest of the tasks changes may be chosen on (the file's bytes).
    pub train: String,
    /// The digest of the sealed tasks, canaries among them.
    pub sealed: String,
    /// Tasks to choose changes on.
    pub train_cases: usize,
    /// Sealed tasks, canaries apart.
    pub sealed_cases: usize,
    /// Canaries among the sealed tasks.
    pub canaries: usize,
    /// The confirmations it allows.
    pub budget: u32,
    /// The confirmations spent.
    #[serde(default)]
    pub used: u32,
    /// When it was made (seconds since the epoch).
    pub at: u64,
    /// The claim that published it, if one did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim: Option<String>,
    /// If it is a reusable holdout: its state. Its sealed tasks then answer
    /// questions through Thresholdout ([`sion_agent::holdout`]), its budget
    /// spent only when they disagree with the tasks to choose on, instead
    /// of a fixed number of confirmations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reusable: Option<sion_agent::holdout::Thresholdout>,
}

impl Seal {
    /// The file of the tasks changes may be chosen on.
    pub fn train_path(&self) -> PathBuf {
        dir().join(format!(
            "{}-train-{}.json",
            self.profile,
            short(&self.train, 16)
        ))
    }

    /// The file of the sealed tasks.
    pub fn sealed_path(&self) -> PathBuf {
        dir().join(format!(
            "{}-sealed-{}.json",
            self.profile,
            short(&self.sealed, 16)
        ))
    }

    /// The confirmations it has left.
    pub fn left(&self) -> u32 {
        self.budget.saturating_sub(self.used)
    }

    /// The z each confirmation is held to: a 95% chance, over all the
    /// confirmations it allows, of confirming no change that is no better.
    pub fn z(&self) -> f64 {
        sion_agent::compare::z_for(ALPHA / f64::from(self.budget.max(1)))
    }

    /// The tasks changes may be chosen on, read and checked against their
    /// digest.
    pub fn train_set(&self) -> Result<(TaskSet, Vec<u8>), CliError> {
        read_set(&self.train_path(), &self.train)
    }

    /// The sealed tasks, read and checked against their digest.
    pub fn sealed_set(&self) -> Result<(TaskSet, Vec<u8>), CliError> {
        read_set(&self.sealed_path(), &self.sealed)
    }
}

/// The first `n` characters of `text`, or all of it if it is shorter.
pub fn short(text: &str, n: usize) -> &str {
    match text.char_indices().nth(n) {
        Some((at, _)) => &text[..at],
        None => text,
    }
}

fn read_set(path: &std::path::Path, digest: &str) -> Result<(TaskSet, Vec<u8>), CliError> {
    let bytes = std::fs::read(path).map_err(io(path))?;
    let found = sion_agent::evals::digest(&bytes).to_hex();
    if found != digest {
        return Err(CliError::Usage(format!(
            "{}: its digest is {found}, not the {digest} sealed: the tasks were changed after they were sealed",
            path.display()
        )));
    }
    let set = TaskSet::parse(&bytes).map_err(CliError::Usage)?;
    Ok((set, bytes))
}

/// Where a profile's current seal is kept.
pub fn seal_path(profile: &str) -> PathBuf {
    dir().join(format!("{profile}-seal.json"))
}

/// A profile's current seal.
pub fn current(profile: &str) -> Result<Seal, CliError> {
    let path = seal_path(profile);
    let text = std::fs::read_to_string(&path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            CliError::Usage(format!(
                "profile {profile} has no sealed tasks (sion agent improve seal)"
            ))
        } else {
            io(&path)(e)
        }
    })?;
    serde_json::from_str(&text).map_err(|e| CliError::Usage(format!("{}: {e}", path.display())))
}

/// Splits `tasks` (a set's exact bytes) for `profile`: `share` percent of
/// its cases (at least one, and at least one left) sealed, each case's side
/// drawn from the set's digest, `seed` and its name; `canaries` canary
/// cases, each with a code word from the system's randomness, added to the
/// sealed part. A task exported for training ([`crate::training`]) is
/// never sealed. The files are written beside the profile and the seal made
/// current, its confirmations `budget`. The seal is not yet on any ledger.
pub fn seal(
    profile: &crate::profiles::Profile,
    tasks: &[u8],
    share: u32,
    budget: u32,
    canaries: usize,
    seed: u64,
    now: u64,
) -> Result<Seal, CliError> {
    let set = TaskSet::parse(tasks).map_err(CliError::Usage)?;
    if set.cases.iter().any(sion_agent::guard::is_canary) {
        return Err(CliError::Usage(format!(
            "a case's name starts with {:?}, which canaries' names do",
            sion_agent::guard::CANARY
        )));
    }
    if set.cases.len() < 2 {
        return Err(CliError::Usage(
            "a seal needs at least two tasks: one to choose changes on, one to confirm them on"
                .into(),
        ));
    }
    if !(1..=90).contains(&share) {
        return Err(CliError::Usage(
            "the share sealed is 1 to 90 percent".into(),
        ));
    }
    if !(1..=64).contains(&budget) {
        return Err(CliError::Usage(
            "a seal allows 1 to 64 confirmations".into(),
        ));
    }
    if canaries > 8 {
        return Err(CliError::Usage("at most 8 canaries".into()));
    }
    let source = sion_agent::evals::digest(tasks);
    let n = set.cases.len();
    // A task exported for training is never sealed: a model trained on it
    // would pass it by memory ([`crate::training`]).
    let trained = crate::training::trained(&profile.name)?;
    let eligible = set
        .cases
        .iter()
        .filter(|case| !trained.contains(&crate::training::task_digest(&case.task)))
        .count();
    if eligible == 0 {
        return Err(CliError::Usage(
            "every one of these tasks was exported for training (sion agent improve export): \
             none can be sealed"
                .into(),
        ));
    }
    let sealed_n = ((n * share as usize).div_ceil(100))
        .clamp(1, n - 1)
        .min(eligible);
    let mut order: Vec<(sion_core::hash::Hash32, usize)> = set
        .cases
        .iter()
        .enumerate()
        .filter(|(_, case)| !trained.contains(&crate::training::task_digest(&case.task)))
        .map(|(i, case)| {
            (
                sion_core::hash::hash_parts(
                    "sion-cli/improve/split/v1",
                    &[&source.0, &seed.to_be_bytes(), case.id.as_bytes()],
                ),
                i,
            )
        })
        .collect();
    order.sort();
    let sealed_at: std::collections::BTreeSet<usize> =
        order.iter().take(sealed_n).map(|(_, i)| *i).collect();
    let mut train = Vec::new();
    let mut sealed = Vec::new();
    for (i, case) in set.cases.iter().enumerate() {
        if sealed_at.contains(&i) {
            sealed.push(case.clone());
        } else {
            train.push(case.clone());
        }
    }
    for index in 0..canaries {
        let mut bytes = [0u8; 8];
        rand_core::RngCore::fill_bytes(&mut rand_core::OsRng, &mut bytes);
        sealed.push(sion_agent::guard::canary(index, &hex::encode(bytes)));
    }
    let train_set = TaskSet {
        name: trimmed(&set.name, "-train"),
        about: set.about.clone(),
        system: set.system.clone(),
        cases: train,
    };
    let sealed_set = TaskSet {
        name: trimmed(&set.name, "-sealed"),
        about: set.about.clone(),
        system: set.system.clone(),
        cases: sealed,
    };
    let as_bytes = |set: &TaskSet| {
        serde_json::to_vec_pretty(set)
            .map_err(|e| CliError::Usage(format!("the tasks could not be written: {e}")))
    };
    let train_bytes = as_bytes(&train_set)?;
    let sealed_bytes = as_bytes(&sealed_set)?;
    let made = Seal {
        profile: profile.name.clone(),
        agent: profile.agent,
        source: source.to_hex(),
        seed,
        train: sion_agent::evals::digest(&train_bytes).to_hex(),
        sealed: sion_agent::evals::digest(&sealed_bytes).to_hex(),
        train_cases: train_set.cases.len(),
        sealed_cases: sealed_n,
        canaries,
        budget,
        used: 0,
        at: now,
        claim: None,
        reusable: None,
    };
    std::fs::create_dir_all(dir()).map_err(io(&dir()))?;
    for (path, bytes) in [
        (made.train_path(), &train_bytes),
        (made.sealed_path(), &sealed_bytes),
    ] {
        std::fs::write(&path, bytes).map_err(io(&path))?;
    }
    save(&made)?;
    Ok(made)
}

/// `name` with `suffix`, within a set name's 64 bytes.
fn trimmed(name: &str, suffix: &str) -> String {
    let mut base = name.to_string();
    base.truncate(64 - suffix.len());
    format!("{}{suffix}", base.trim_end_matches('-'))
}

fn save(seal: &Seal) -> Result<(), CliError> {
    crate::profiles::write_json(&seal_path(&seal.profile), seal)
}

/// Spends one of `profile`'s seal's confirmations, under its lock: the
/// seal as it now stands, or an error if none is left or the seal changed
/// (`sealed` is the digest the caller confirms on).
pub fn spend(profile: &str, sealed: &str) -> Result<Seal, CliError> {
    let path = seal_path(profile);
    let _held = crate::pairing::Held::take(&path.with_extension("lock"))?;
    let mut seal = current(profile)?;
    if seal.sealed != sealed {
        return Err(CliError::Usage(
            "the profile's tasks were sealed anew while this ran".into(),
        ));
    }
    if seal.left() == 0 {
        return Err(CliError::Usage(format!(
            "the seal's {} confirmations are spent: seal fresh tasks (sion agent improve seal)",
            seal.budget
        )));
    }
    seal.used += 1;
    save(&seal)?;
    Ok(seal)
}

/// A uniform draw in (0, 1) from the system's randomness.
fn uniform() -> f64 {
    let mut bytes = [0u8; 8];
    rand_core::RngCore::fill_bytes(&mut rand_core::OsRng, &mut bytes);
    ((u64::from_be_bytes(bytes) >> 11) as f64 + 0.5) / (1u64 << 53) as f64
}

/// Makes `profile`'s seal, before anything is tried on it or it is
/// published, a reusable holdout ([`sion_agent::holdout`]): its sealed tasks
/// may disagree with the tasks to choose on as many times as the seal
/// allowed confirmations.
pub fn make_reusable(profile: &str) -> Result<Seal, CliError> {
    let path = seal_path(profile);
    let _held = crate::pairing::Held::take(&path.with_extension("lock"))?;
    let mut seal = current(profile)?;
    if seal.reusable.is_some() {
        // Its noisy threshold is drawn once: drawn again, a questioner
        // could keep the most lenient.
        return Err(CliError::Usage(
            "the seal is a reusable holdout already".into(),
        ));
    }
    if seal.used > 0 || seal.claim.is_some() {
        return Err(CliError::Usage(
            "a seal becomes a reusable holdout before anything is tried on it or it is published"
                .into(),
        ));
    }
    seal.reusable = Some(sion_agent::holdout::Thresholdout::for_holdout(
        seal.sealed_cases,
        seal.budget,
        &mut uniform,
    ));
    save(&seal)?;
    Ok(seal)
}

/// Asks `profile`'s reusable holdout, under its lock, about a statistic
/// measured as `train` on the tasks to choose on and as `holdout` on the
/// sealed tasks whose digest is `sealed`: the answer, and the holdout as it
/// now stands. An error once its budget is spent.
pub fn ask(
    profile: &str,
    sealed: &str,
    train: f64,
    holdout: f64,
) -> Result<
    (
        sion_agent::holdout::Answer,
        sion_agent::holdout::Thresholdout,
    ),
    CliError,
> {
    let path = seal_path(profile);
    let _held = crate::pairing::Held::take(&path.with_extension("lock"))?;
    let mut seal = current(profile)?;
    if seal.sealed != sealed {
        return Err(CliError::Usage(
            "the profile's tasks were sealed anew while this ran".into(),
        ));
    }
    let mut state = seal
        .reusable
        .clone()
        .ok_or_else(|| CliError::Usage("the profile's seal is not a reusable holdout".into()))?;
    let answer = state.answer(train, holdout, &mut uniform).ok_or_else(|| {
        CliError::Usage(format!(
            "the reusable holdout disagreed {} times, all its budget: seal fresh tasks (sion agent improve seal)",
            state.budget
        ))
    })?;
    seal.reusable = Some(state.clone());
    save(&seal)?;
    Ok((answer, state))
}

/// What a seal's claim says.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SealClaim {
    /// The agent.
    pub agent: ObjectId,
    /// The profile's name.
    pub profile: String,
    /// The digest of the tasks as given.
    pub source: String,
    /// The seed of the split.
    pub seed: u64,
    /// The digest of the tasks changes may be chosen on.
    pub train: String,
    /// The digest of the sealed tasks.
    pub sealed: String,
    /// Their counts.
    pub train_cases: usize,
    /// Sealed tasks, canaries apart.
    pub sealed_cases: usize,
    /// Canaries among them.
    pub canaries: usize,
    /// The confirmations it allows.
    pub budget: u32,
    /// If it is a reusable holdout: its threshold and noise, in millionths.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reusable: Option<(u64, u64)>,
}

impl SealClaim {
    /// What `seal` publishes.
    pub fn of(seal: &Seal) -> SealClaim {
        SealClaim {
            agent: seal.agent,
            profile: seal.profile.clone(),
            source: seal.source.clone(),
            seed: seal.seed,
            train: seal.train.clone(),
            sealed: seal.sealed.clone(),
            train_cases: seal.train_cases,
            sealed_cases: seal.sealed_cases,
            canaries: seal.canaries,
            budget: seal.budget,
            reusable: seal.reusable.as_ref().map(|r| {
                (
                    (r.threshold * 1e6).round() as u64,
                    (r.sigma * 1e6).round() as u64,
                )
            }),
        }
    }
}

/// A claim that commits to `payload`: it rests on its issuer's signature
/// alone, since a commitment is not a statement that can be proven false.
fn commitment(schema: &str, subject: String, payload: Vec<u8>) -> ClaimBody {
    ClaimBody {
        schema: schema.into(),
        subject,
        payload: Payload::Public(payload),
        formula: Formula::and(vec![formulas::dual_signature()]),
        adjudicator: None,
        detection_permille: 0,
        valid_for: VALID_FOR,
        challenge_period: 0,
    }
}

/// The index key an agent's latest seal is named under.
pub fn seal_key(agent: &ObjectId) -> String {
    format!("agent-seal:{}", hex::encode(&agent.as_hash().0[..16]))
}

/// The index key an agent's latest kept change is named under.
pub fn step_key(agent: &ObjectId) -> String {
    format!(
        "agent-improvement:{}",
        hex::encode(&agent.as_hash().0[..16])
    )
}

/// Publishes `profile`'s current seal as `wallet`'s claim, and records the
/// claim in it. Returns the claim.
pub fn publish(
    session: &mut Session<'_>,
    wallet: &str,
    profile: &str,
) -> Result<ClaimId, CliError> {
    // Under the seal's lock throughout: two publishings at once would
    // otherwise both find it unpublished, and a confirmation spent
    // meanwhile would come before the claim that says it was sealed.
    let path = seal_path(profile);
    let _held = crate::pairing::Held::take(&path.with_extension("lock"))?;
    let mut seal = current(profile)?;
    if let Some(claim) = &seal.claim {
        return Err(CliError::Usage(format!(
            "the seal is already published, as claim {claim}"
        )));
    }
    if seal.used > 0 || seal.reusable.as_ref().is_some_and(|r| r.asked > 0) {
        return Err(CliError::Usage(format!(
            "a seal is published before anything is tried on it, and this one has spent {} of its confirmations or been asked: seal fresh tasks",
            seal.used
        )));
    }
    let payload = serde_json::to_vec(&SealClaim::of(&seal))
        .map_err(|e| CliError::Usage(format!("the seal could not be written: {e}")))?;
    let body = commitment(
        SEAL_SCHEMA,
        format!("seals tasks to confirm agent {}'s changes on", seal.agent),
        payload,
    );
    let id = crate::vouching::publish(session, wallet, body, Some(seal_key(&seal.agent)))?;
    seal.claim = Some(id.as_hash().to_hex());
    save(&seal)?;
    Ok(id)
}

/// Publishes `body`, a step's claim, as `wallet`'s; it is named the
/// agent's latest kept change.
pub fn publish_step(
    session: &mut Session<'_>,
    wallet: &str,
    agent: &ObjectId,
    payload: Vec<u8>,
    subject: String,
) -> Result<ClaimId, CliError> {
    let body = commitment(STEP_SCHEMA, subject, payload);
    crate::vouching::publish(session, wallet, body, Some(step_key(agent)))
}

/// A line for each check of `profile`'s lineage against the ledger: each
/// kept change's claim holds what was recorded here, names its seal, and
/// was issued after it; each seal's claim holds the digests of the tasks
/// kept here.
pub fn check_lineage(ledger: &dyn Ledger, profile: &str) -> Result<Vec<String>, CliError> {
    let steps = crate::improve_loop::lineage(profile)?;
    let mut lines = Vec::new();
    let mut seals: BTreeMap<String, (SealClaim, sion_core::ids::Pulse)> = BTreeMap::new();
    if steps.is_empty() {
        lines.push(format!(
            "profile {profile} has kept no change from its own work yet"
        ));
    }
    for step in &steps {
        let name = format!("step {}", step.id);
        let Some(claim) = &step.claim else {
            lines.push(format!(
                "{name}: kept on this machine only, not on the ledger"
            ));
            continue;
        };
        let stored = crate::tasks::stored_claim(ledger, claim)?;
        if stored.body.schema != STEP_SCHEMA {
            lines.push(format!(
                "{name}: claim {claim} is a {}, not a kept change",
                stored.body.schema
            ));
            continue;
        }
        let Payload::Public(bytes) = &stored.body.payload else {
            lines.push(format!("{name}: its claim's payload is not public"));
            continue;
        };
        let expected = step.claimed()?;
        if bytes != &expected {
            lines.push(format!(
                "{name}: its claim does not hold what was recorded here"
            ));
            continue;
        }
        let Some(seal_claim) = &step.seal_claim else {
            lines.push(format!(
                "{name}: on the ledger (Pulse {}), but its seal was kept on this machine only: nothing shows the tasks were sealed first",
                stored.issued_at.0
            ));
            continue;
        };
        let (sealed, sealed_at) = match seals.get(seal_claim) {
            Some(found) => found.clone(),
            None => {
                let found = crate::tasks::stored_claim(ledger, seal_claim)?;
                let Payload::Public(bytes) = &found.body.payload else {
                    lines.push(format!("{name}: its seal's payload is not public"));
                    continue;
                };
                if found.body.schema != SEAL_SCHEMA {
                    lines.push(format!("{name}: claim {seal_claim} is not a seal"));
                    continue;
                }
                let claim: SealClaim = serde_json::from_slice(bytes)
                    .map_err(|e| CliError::Protocol(format!("seal {seal_claim}: {e}")))?;
                seals.insert(seal_claim.clone(), (claim.clone(), found.issued_at));
                (claim, found.issued_at)
            }
        };
        if sealed.sealed != step.sealed {
            lines.push(format!(
                "{name}: its seal holds other tasks than the ones it was confirmed on"
            ));
            continue;
        }
        if sealed_at >= stored.issued_at {
            lines.push(format!(
                "{name}: its seal (Pulse {}) is not earlier than it (Pulse {})",
                sealed_at.0, stored.issued_at.0
            ));
            continue;
        }
        lines.push(format!(
            "{name}: on the ledger as claim {} (Pulse {}), confirmed on tasks sealed at Pulse {}",
            &claim[..16.min(claim.len())],
            stored.issued_at.0,
            sealed_at.0
        ));
    }
    if let Ok(seal) = current(profile) {
        let files = [
            ("tasks to choose on", seal.train_path(), &seal.train),
            ("sealed tasks", seal.sealed_path(), &seal.sealed),
        ];
        for (what, path, digest) in files {
            match std::fs::read(&path) {
                Ok(bytes) if sion_agent::evals::digest(&bytes).to_hex() == *digest => {}
                Ok(_) => lines.push(format!(
                    "the current seal's {what} were changed after they were sealed"
                )),
                Err(_) => lines.push(format!("the current seal's {what} are not on this machine")),
            }
        }
        if let Some(claim) = &seal.claim {
            let stored = crate::tasks::stored_claim(ledger, claim)?;
            let ok = matches!(&stored.body.payload, Payload::Public(bytes)
                if serde_json::from_slice::<SealClaim>(bytes).ok().as_ref() == Some(&SealClaim::of(&seal)));
            lines.push(if ok {
                format!(
                    "the current seal is on the ledger as claim {} (Pulse {}), {} of its {} confirmations spent",
                    &claim[..16.min(claim.len())],
                    stored.issued_at.0,
                    seal.used,
                    seal.budget
                )
            } else {
                format!("the current seal's claim {claim} does not hold its digests")
            });
        } else {
            lines.push("the current seal is kept on this machine only".into());
        }
    }
    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_set_name_keeps_to_letters_digits_and_hyphens() {
        assert_eq!(set_name("Lead Agent_2"), "lead-agent-2");
        assert_eq!(set_name("!!!"), "agent");
        assert_eq!(trimmed(&"x".repeat(64), "-sealed").len(), 64);
    }
}
