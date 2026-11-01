//! Skills that get better, with others checking (part 3 of the agents
//! plan), on today's primitives.
//!
//! - **An evaluation set** ([`sion_agent::evals`]) measures a skill: cases
//!   with stand-in tools and checks. `sion skill eval` runs it and can
//!   export each run as a trajectory labelled with its result.
//! - **Evolution** ([`sion_agent::evolve`]): `sion skill evolve` shows a
//!   model the cases a version fails and scores the variants it proposes,
//!   under gates, keeping a Pareto front. The best is kept as a draft,
//!   which nothing installs.
//! - **Vouching** (`skill.evals/v1`): whoever wrote an evaluation set can
//!   claim, on a bond, that it has no known way to be passed without doing
//!   what the skill describes, as an environment vendor does for a grader.
//! - **A bounty** is a Task Charter for a better version of a release. Its
//!   spec names the release, its bundle, the evaluation set's digest and
//!   how many cases must pass; its verifiers, its adjudicator and the
//!   claims every result must rest on (the set's vouching) are fixed when
//!   it is posted, before any version exists.
//! - **Verifying**: a verifier named by the task runs the set on the new
//!   version itself and attests, on its bond, only if enough cases pass.
//!   The task's worker cannot verify its own version.
//! - **Submitting** settles the task with the worker's result claim, which
//!   rests on the attestations and the set's vouching; the ledger checks
//!   the formula and the poster's decision rule before paying.
//! - **Publishing** the version with `--evidence <result>` makes the
//!   release rest on that result too, and record it, so that a skill's
//!   lineage shows the evidence for each step (`sion skill lineage`).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use sion_agent::evals::{EvalSet, Report};
use sion_agent::evolve::{Evolution, Settings};
use sion_agent::{Limits, Model};
use sion_assurance::{AtomClass, Policy, Ratio};
use sion_core::hash::Hash32;
use sion_core::ids::{AtomId, ClaimId, ObjectId, Pulse};
use sion_exec::claim::{claim_id, formulas, ClaimBody, Payload};
use sion_exec::execute::{bond_atom, Event};
use sion_exec::task::{Acceptance, TaskResult, TaskState, RESULT_SCHEMA};
use sion_exec::{ExecStatus, TxKind};

use crate::error::CliError;
use crate::ledger::{Ledger, LedgerExt};
use crate::session::{wallet_dir, Session};

/// The schema of a claim vouching for an evaluation set.
pub const VOUCH: &str = "skill.evals/v1";
/// The kind of a bounty's spec.
pub const SPEC_KIND: &str = "skill-improvement/v1";

fn skills_home() -> PathBuf {
    wallet_dir()
        .parent()
        .map(|home| home.join("skills"))
        .unwrap_or_else(|| PathBuf::from(".sion/skills"))
}

/// Where evaluation sets are kept by digest: `$SION_HOME/skills/evals`.
pub fn evals_dir() -> PathBuf {
    skills_home().join("evals")
}

/// Where bounties' specs are kept by commitment: `$SION_HOME/skills/specs`.
pub fn specs_dir() -> PathBuf {
    skills_home().join("specs")
}

/// Where evolved versions are kept: `$SION_HOME/skills/drafts`.
pub fn drafts_dir() -> PathBuf {
    skills_home().join("drafts")
}

fn work_dir() -> PathBuf {
    skills_home().join("work")
}

fn io(path: &Path) -> impl Fn(std::io::Error) -> CliError + '_ {
    move |e| CliError::Usage(format!("{}: {e}", path.display()))
}

fn parse_hash(text: &str, what: &str) -> Result<Hash32, CliError> {
    Hash32::from_hex(text.trim())
        .map_err(|_| CliError::Usage(format!("{what} must be 64 hex digits")))
}

/// Reads an evaluation set: the set, its bytes and its digest. It is kept
/// in the local store too, under its digest.
pub fn load_evals(path: &Path) -> Result<(EvalSet, Vec<u8>, Hash32), CliError> {
    let bytes = std::fs::read(path).map_err(io(path))?;
    let set = EvalSet::parse(&bytes).map_err(CliError::Usage)?;
    let digest = sion_agent::evals::digest(&bytes);
    let dir = evals_dir();
    std::fs::create_dir_all(&dir).map_err(io(&dir))?;
    let kept = dir.join(format!("{}.json", digest.to_hex()));
    std::fs::write(&kept, &bytes).map_err(io(&kept))?;
    Ok((set, bytes, digest))
}

/// The evaluation set with `digest`: from `file`, or the local store.
fn evals_by_digest(digest: &Hash32, file: Option<&Path>) -> Result<EvalSet, CliError> {
    let path = match file {
        Some(path) => path.to_path_buf(),
        None => evals_dir().join(format!("{}.json", digest.to_hex())),
    };
    let bytes = std::fs::read(&path).map_err(|_| {
        CliError::Usage(format!(
            "no copy of evaluation set {}: give --evals",
            digest.short()
        ))
    })?;
    if sion_agent::evals::digest(&bytes) != *digest {
        return Err(CliError::Verification(format!(
            "{} is not evaluation set {}",
            path.display(),
            digest.short()
        )));
    }
    EvalSet::parse(&bytes).map_err(CliError::Usage)
}

/// Fresh models from their descriptions, for each run of a case.
pub fn models_of(specs: &[String]) -> impl Fn() -> Result<Vec<Box<dyn Model>>, String> + '_ {
    move || {
        if specs.is_empty() {
            return Err("give at least one --model".into());
        }
        specs
            .iter()
            .map(|spec| crate::runtime::model(spec, 0).map_err(|e| e.to_string()))
            .collect()
    }
}

/// Puts a bundle's files in a skills directory of their own, to run cases
/// with: `work/<root>/<name>`. Returns the directory of skills.
fn stage(bytes: &[u8], root: &Hash32, name: &str) -> Result<PathBuf, CliError> {
    let files = sion_agent::skills::unbundle(bytes).map_err(CliError::Usage)?;
    let skills = work_dir().join(root.to_hex());
    if skills.exists() {
        std::fs::remove_dir_all(&skills).map_err(io(&skills))?;
    }
    for (relative, content) in files {
        let path = skills.join(name).join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(io(parent))?;
        }
        std::fs::write(&path, content).map_err(io(&path))?;
    }
    Ok(skills)
}

/// The lines that summarise a report.
fn report_lines(report: &Report) -> Vec<String> {
    let mut lines = report.lines();
    lines.push(format!(
        "{} of {} cases passed",
        report.passed(),
        report.runs.len()
    ));
    lines
}

/// Each run as a trajectory (chat format), labelled with how it scored.
fn write_trajectories(
    path: &Path,
    report: &Report,
    digest: &Hash32,
    root: &Hash32,
) -> Result<(), CliError> {
    let mut text = String::new();
    for run in &report.runs {
        let line = sion_agent::trace::chat_line(&run.state, &run.system, Some(&run.ended));
        let mut value: Value =
            serde_json::from_str(&line).map_err(|e| CliError::Protocol(e.to_string()))?;
        value["metadata"]["case"] = json!(run.case);
        value["metadata"]["passed"] = json!(run.passed);
        value["metadata"]["failures"] = json!(run.failures);
        value["metadata"]["evals"] = json!(digest.to_hex());
        value["metadata"]["bundle"] = json!(root.to_hex());
        text.push_str(&value.to_string());
        text.push('\n');
    }
    std::fs::write(path, text).map_err(io(path))
}

/// Runs an evaluation set on the skill in `dir`; returns the report and
/// what to print.
pub fn eval(
    dir: &Path,
    evals: &Path,
    models: &[String],
    trajectories: Option<&Path>,
) -> Result<(Report, Vec<String>), CliError> {
    let (set, _, digest) = load_evals(evals)?;
    let (bytes, root, name, _) = crate::skills::pack(dir)?;
    if name != set.skill {
        return Err(CliError::Usage(format!(
            "the evaluation set is for skill {}, not {name}",
            set.skill
        )));
    }
    let skills = stage(&bytes, &root, &name)?;
    let report = sion_agent::evals::evaluate(&set, &skills, &models_of(models), &Limits::default())
        .map_err(CliError::Usage)?;
    let mut lines = vec![format!(
        "skill {name}, bundle {}; evaluation set {} ({} cases){}",
        root.short(),
        digest.short(),
        set.cases.len(),
        if set.about.is_empty() {
            String::new()
        } else {
            format!(": {}", set.about)
        }
    )];
    lines.extend(report_lines(&report));
    if let Some(path) = trajectories {
        write_trajectories(path, &report, &digest, &root)?;
        lines.push(format!(
            "{} trajectories, labelled with their results, in {}",
            report.runs.len(),
            path.display()
        ));
    }
    std::fs::remove_dir_all(&skills).ok();
    Ok((report, lines))
}

/// Evolves the skill in `dir` against an evaluation set and keeps the best
/// version found as a draft, if one did better than the version given and,
/// with `held`, an evaluation set whose cases the evolution never sees,
/// holds up on those ([`sion_agent::evolve::held_out`]); returns the
/// evolution, the draft's directory and what to print.
pub fn evolve(
    dir: &Path,
    evals: &Path,
    held: Option<&Path>,
    models: &[String],
    mutator: &str,
    settings: &Settings,
) -> Result<(Evolution, Option<PathBuf>, Vec<String>), CliError> {
    let (set, _, digest) = load_evals(evals)?;
    let held = held.map(load_evals).transpose()?;
    let (_, parent_root, name, _) = crate::skills::pack(dir)?;
    let mutator = crate::runtime::model(mutator, 0)?;
    let work = work_dir().join(format!("evolve-{}", parent_root.to_hex()));
    if work.exists() {
        std::fs::remove_dir_all(&work).map_err(io(&work))?;
    }
    let evolution = sion_agent::evolve::evolve(
        dir,
        &set,
        &models_of(models),
        mutator.as_ref(),
        settings,
        &work,
    )
    .map_err(CliError::Usage)?;
    let cases = set.cases.len();
    let mut lines = vec![format!(
        "skill {name}, bundle {}; evaluation set {} ({cases} cases)",
        parent_root.short(),
        digest.short()
    )];
    for candidate in &evolution.candidates {
        let from = match candidate.parent {
            Some(parent) => format!("from version {parent}, generation {}", candidate.generation),
            None => "the version given".to_string(),
        };
        lines.push(format!(
            "version {} ({from}): {} of {cases} cases, {} bytes{}",
            candidate.index,
            candidate.passed(),
            candidate.size,
            if candidate.parent.is_some() {
                format!(": {}", candidate.why)
            } else {
                String::new()
            }
        ));
    }
    for rejected in &evolution.rejected {
        lines.push(format!(
            "turned down (generation {}, from version {}): {}",
            rejected.generation, rejected.parent, rejected.reason
        ));
    }
    lines.push(format!(
        "Pareto front: {}",
        evolution
            .front
            .iter()
            .map(|i| format!("version {i}"))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    let best = &evolution.candidates[evolution.best];
    if evolution.best == 0 {
        lines.push("no version did better than the one given: no draft".into());
        std::fs::remove_dir_all(&work).ok();
        return Ok((evolution, None, lines));
    }
    // On cases it never saw, the best must pass whatever the version given
    // passes.
    let mut held_record = serde_json::Value::Null;
    if let Some((held_set, _, held_digest)) = &held {
        let out = sion_agent::evolve::held_out(
            &evolution,
            &set,
            held_set,
            &models_of(models),
            &settings.limits,
        )
        .map_err(CliError::Usage)?;
        let passed = |passes: &[bool]| passes.iter().filter(|p| **p).count();
        lines.push(format!(
            "held out (evaluation set {}, {} cases the evolution never saw): the version given passes {}, version {} passes {}",
            held_digest.short(),
            held_set.cases.len(),
            passed(&out.given),
            evolution.best,
            passed(&out.best)
        ));
        if let Some(case) = &out.regressed {
            lines.push(format!(
                "version {} fails held-out case {case}, which the version given passes: no draft",
                evolution.best
            ));
            std::fs::remove_dir_all(&work).ok();
            return Ok((evolution, None, lines));
        }
        held_record = json!({
            "evals": held_digest.to_hex(),
            "given": out.given,
            "best": out.best,
        });
    }
    let best_dir = best.dir.join(&name);
    let (bytes, root, _, _) = crate::skills::pack(&best_dir)?;
    let draft = drafts_dir().join(format!("{name}-{}", &root.to_hex()[..8]));
    if draft.exists() {
        std::fs::remove_dir_all(&draft).map_err(io(&draft))?;
    }
    for (relative, content) in sion_agent::skills::unbundle(&bytes).map_err(CliError::Usage)? {
        let path = draft.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(io(parent))?;
        }
        std::fs::write(&path, content).map_err(io(&path))?;
    }
    let record = draft.with_extension("json");
    let summary = json!({
        "skill": name,
        "parent_bundle": parent_root.to_hex(),
        "bundle": root.to_hex(),
        "evals": digest.to_hex(),
        "passed": best.passed(),
        "cases": cases,
        "candidates": evolution.candidates.iter().map(|c| json!({
            "index": c.index,
            "parent": c.parent,
            "generation": c.generation,
            "passes": c.passes,
            "size": c.size,
            "why": c.why,
        })).collect::<Vec<_>>(),
        "rejected": evolution.rejected,
        "front": evolution.front,
        "best": evolution.best,
        "held_out": held_record,
    });
    std::fs::write(
        &record,
        serde_json::to_string_pretty(&summary).unwrap_or_default(),
    )
    .map_err(io(&record))?;
    lines.push(format!(
        "best: version {}, {} of {cases} cases, bundle {}: a draft in {}",
        evolution.best,
        best.passed(),
        root.short(),
        draft.display()
    ));
    std::fs::remove_dir_all(&work).ok();
    Ok((evolution, Some(draft), lines))
}

/// Vouches, on a bond, for an evaluation set: that it has no known way to
/// be passed without doing what the skill describes. Returns the claim.
pub fn vouch(
    session: &mut Session<'_>,
    wallet: &str,
    evals: &Path,
    bond: &str,
    adjudicator: AtomId,
    valid_for: u64,
) -> Result<(ClaimId, sion_node::TxView), CliError> {
    let (set, _, digest) = load_evals(evals)?;
    let who = session.id(wallet);
    let body = ClaimBody {
        schema: VOUCH.into(),
        subject: format!(
            "Evaluation set {} for skill {} has no known way to be passed without doing what the skill describes",
            digest.short(),
            set.skill
        ),
        payload: Payload::Public(
            json!({"evals": digest.to_hex(), "skill": set.skill, "about": set.about, "cases": set.cases.len()})
                .to_string()
                .into_bytes(),
        ),
        formula: formulas::bonded_statement(bond_atom(AtomClass::Economic, &who, bond).id),
        adjudicator: Some(adjudicator),
        detection_permille: 1000,
        valid_for,
        challenge_period: 100,
    };
    let id = claim_id(&who, &body);
    let view = session.send(wallet, TxKind::IssueClaim { claim: body }, Vec::new())?;
    joined(session.ledger, id, wallet, bond)?;
    Ok((id, view))
}

/// Checks that a claim just issued joined the knowledge log: a claim on a
/// bond that was never posted is submitted, then rejected.
fn joined(ledger: &dyn Ledger, id: ClaimId, wallet: &str, bond: &str) -> Result<(), CliError> {
    match crate::tasks::stored_claim(ledger, &id.to_string()) {
        Ok(_) => Ok(()),
        Err(_) => Err(CliError::Transaction(format!(
            "claim {} did not join the knowledge log: it rests on bond:{wallet}:{bond}; post it first (`sion bond {wallet} {bond} <amount>`), and see `sion events`",
            id.short()
        ))),
    }
}

/// A bounty's spec: what a better version of a release must do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Spec {
    /// The skill.
    pub skill: String,
    /// The release to improve.
    pub release: ClaimId,
    /// Its bundle.
    pub bundle: Hash32,
    /// The evaluation set's digest.
    pub evals: Hash32,
    /// The cases a version must pass.
    pub pass: usize,
    /// The cases in the set.
    pub cases: usize,
}

impl Spec {
    /// Its exact bytes (keys in order): what the task commits to.
    pub fn bytes(&self) -> Vec<u8> {
        json!({
            "kind": SPEC_KIND,
            "skill": self.skill,
            "release": self.release.to_string(),
            "bundle": self.bundle.to_hex(),
            "evals": self.evals.to_hex(),
            "pass": self.pass,
            "cases": self.cases,
        })
        .to_string()
        .into_bytes()
    }

    /// Reads a spec.
    pub fn parse(bytes: &[u8]) -> Result<Spec, CliError> {
        let value: Value = serde_json::from_slice(bytes)
            .map_err(|e| CliError::Usage(format!("not a bounty's spec: {e}")))?;
        if value["kind"] != SPEC_KIND {
            return Err(CliError::Usage(format!("not a {SPEC_KIND} spec")));
        }
        let text = |key: &str| value[key].as_str().unwrap_or_default().to_string();
        let count = |key: &str| value[key].as_u64().unwrap_or(0) as usize;
        Ok(Spec {
            skill: text("skill"),
            release: ClaimId::from_hash(parse_hash(&text("release"), "the spec's release")?),
            bundle: parse_hash(&text("bundle"), "the spec's bundle")?,
            evals: parse_hash(&text("evals"), "the spec's evaluation set")?,
            pass: count("pass"),
            cases: count("cases"),
        })
    }
}

/// The spec of `task`: from `file`, or the local store, checked against the
/// task's commitment.
pub fn spec_of(ledger: &dyn Ledger, task: ObjectId, file: Option<&Path>) -> Result<Spec, CliError> {
    let state = crate::tasks::load_task(ledger, task)?;
    let path = match file {
        Some(path) => path.to_path_buf(),
        None => specs_dir().join(format!("{}.json", state.spec.to_hex())),
    };
    let bytes = std::fs::read(&path).map_err(|_| {
        CliError::Usage(format!(
            "no copy of task {}'s spec: give --spec",
            task.0.short()
        ))
    })?;
    if crate::tasks::spec_commitment(&bytes) != state.spec {
        return Err(CliError::Verification(format!(
            "{} is not the spec task {} commits to",
            path.display(),
            task.0.short()
        )));
    }
    Spec::parse(&bytes)
}

/// Who must attest a better version (spec v1.15): verifiers who each must,
/// and how many attestations it needs at least.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attesting {
    /// Verifiers who must each attest.
    pub must: Vec<String>,
    /// How many attestations a version needs at least.
    pub at_least: u8,
}

impl Default for Attesting {
    /// One attestation, by any of the verifiers: a better version is
    /// verified by others.
    fn default() -> Self {
        Attesting {
            must: Vec::new(),
            at_least: 1,
        }
    }
}

/// Posts a bounty for a better version of `release`: a Task Charter whose
/// spec names the release, the evaluation set and the cases to pass.
/// Returns the task, the spec's local copy and the receipt.
#[allow(clippy::too_many_arguments)]
pub fn bounty(
    session: &mut Session<'_>,
    ledger: &dyn Ledger,
    wallet: &str,
    release: &str,
    evals: &Path,
    amount: &str,
    verifiers: &[String],
    attesting: &Attesting,
    adjudicator: AtomId,
    requires: &[ClaimId],
    worker: Option<&str>,
    pass: Option<usize>,
    deadline: u64,
    kappa: f64,
    reliance: u64,
) -> Result<(ObjectId, PathBuf, sion_node::TxView), CliError> {
    let parent = crate::skills::release(ledger, release)?;
    let (set, _, digest) = load_evals(evals)?;
    if set.skill != parent.name {
        return Err(CliError::Usage(format!(
            "the evaluation set is for skill {}, the release is of {}",
            set.skill, parent.name
        )));
    }
    let cases = set.cases.len();
    let pass = pass.unwrap_or(cases);
    if pass == 0 || pass > cases {
        return Err(CliError::Usage(format!(
            "--pass is 1 to {cases}, the cases in the set"
        )));
    }
    if verifiers.is_empty() {
        return Err(CliError::Usage(
            "a better version is verified by others: give --verifiers".into(),
        ));
    }
    let verifier_ids: BTreeSet<_> = verifiers.iter().map(|v| session.id(v)).collect();
    let must_attest: BTreeSet<_> = attesting.must.iter().map(|v| session.id(v)).collect();
    if !must_attest.is_subset(&verifier_ids) {
        return Err(CliError::Usage(
            "--must-attest names only verifiers given with --verifiers".into(),
        ));
    }
    let min_attestations = attesting.at_least;
    if usize::from(min_attestations) > verifier_ids.len() {
        return Err(CliError::Usage(format!(
            "{} verifiers cannot give {min_attestations} attestations",
            verifier_ids.len()
        )));
    }
    let worker_id = worker.map(|w| session.id(w));
    if let Some(worker) = &worker_id {
        if verifier_ids.contains(worker) {
            return Err(CliError::Usage(
                "the worker cannot be one of its verifiers".into(),
            ));
        }
    }
    let spec = Spec {
        skill: parent.name.clone(),
        release: parent.claim,
        bundle: parent.bundle,
        evals: digest,
        pass,
        cases,
    };
    let bytes = spec.bytes();
    let commitment = crate::tasks::spec_commitment(&bytes);
    let dir = specs_dir();
    std::fs::create_dir_all(&dir).map_err(io(&dir))?;
    let path = dir.join(format!("{}.json", commitment.to_hex()));
    std::fs::write(&path, &bytes).map_err(io(&path))?;
    let acceptance = Acceptance {
        policy: Policy::standard().with_kappa(Ratio::new(
            (kappa * 1000.0).round().max(1_000.0) as u64,
            1000,
        )),
        requires: requires.iter().copied().collect(),
        verifiers: verifier_ids,
        must_attest,
        min_attestations,
        adjudicator: Some(adjudicator),
        detection_permille: 1000,
        reliance,
    };
    let view = session.send(
        wallet,
        TxKind::PostTask {
            spec: commitment,
            acceptance,
            worker: worker_id,
            amount: crate::agents::parse_amount(amount)?,
            deadline: Pulse(ledger.pulse()?.0 + deadline),
            charter: None,
        },
        Vec::new(),
    )?;
    let task = view
        .effects
        .events
        .iter()
        .find_map(|e| match e {
            Event::TaskPosted { task, .. } => Some(*task),
            _ => None,
        })
        .ok_or_else(|| CliError::Transaction("the bounty was not posted".into()))?;
    Ok((task, path, view))
}

/// The name and description a bundle's `SKILL.md` gives.
fn purpose(bytes: &[u8]) -> Result<(String, String), CliError> {
    let files = sion_agent::skills::unbundle(bytes).map_err(CliError::Usage)?;
    let text = files
        .iter()
        .find(|(p, _)| p == "SKILL.md")
        .map(|(_, c)| String::from_utf8_lossy(c).into_owned())
        .unwrap_or_default();
    let (name, description, _) = sion_agent::skills::parse(&text).map_err(CliError::Usage)?;
    Ok((name, description))
}

/// Verifies a version for a bounty, as one of its verifiers: checks that
/// it keeps the skill's purpose, runs the evaluation set on it (printing
/// the results before anything is signed), and attests on `bond` only if
/// enough cases pass. Returns the attestation, if any, and the report.
#[allow(clippy::too_many_arguments)]
pub fn verify(
    session: &mut Session<'_>,
    ledger: &dyn Ledger,
    wallet: &str,
    task: ObjectId,
    dir: &Path,
    evals: Option<&Path>,
    spec_file: Option<&Path>,
    models: &[String],
    bond: &str,
) -> Result<(Option<ClaimId>, Report), CliError> {
    let state = crate::tasks::load_task(ledger, task)?;
    let spec = spec_of(ledger, task, spec_file)?;
    let who = session.id(wallet);
    if !state.acceptance.verifiers.contains(&who) {
        return Err(CliError::Usage(format!(
            "{wallet} is not a verifier of task {}",
            task.0.short()
        )));
    }
    if state.worker == Some(who) {
        return Err(CliError::Usage(
            "the task's worker cannot verify its own version".into(),
        ));
    }
    if state.state != TaskState::Open {
        return Err(CliError::Usage(format!(
            "task {} is no longer open",
            task.0.short()
        )));
    }
    let set = evals_by_digest(&spec.evals, evals)?;
    let (bytes, root, name, description) = crate::skills::pack(dir)?;
    if root == spec.bundle {
        return Err(CliError::Usage(
            "this is the release the task asks to improve, not a new version".into(),
        ));
    }
    let parent = crate::skills::release(ledger, &spec.release.to_string())?;
    let parent_bytes = crate::skills::fetch_bundle(&parent, None)?;
    let (parent_name, parent_description) = purpose(&parent_bytes)?;
    if (name.as_str(), description.as_str()) != (parent_name.as_str(), parent_description.as_str())
    {
        return Err(CliError::Verification(
            "a version must keep the skill's name and description".into(),
        ));
    }
    let skills = stage(&bytes, &root, &name)?;
    let report = sion_agent::evals::evaluate(&set, &skills, &models_of(models), &Limits::default())
        .map_err(CliError::Usage)?;
    std::fs::remove_dir_all(&skills).ok();
    // Shown now, before the attestation is signed.
    println!(
        "bundle {} for task {}: evaluation set {}, {} of {} cases needed",
        root.short(),
        task.0.short(),
        spec.evals.short(),
        spec.pass,
        spec.cases
    );
    for line in report_lines(&report) {
        println!("{line}");
    }
    if report.passed() < spec.pass {
        println!("not attested");
        return Ok((None, report));
    }
    let body = crate::tasks::attestation_body(ledger, task, &who, &bytes, bond)?;
    let id = claim_id(&who, &body);
    let view = session.send(wallet, TxKind::IssueClaim { claim: body }, Vec::new())?;
    crate::agents::print_receipt(session, &view);
    if view.status != ExecStatus::Success {
        return Err(CliError::Transaction("the attestation was refused".into()));
    }
    joined(ledger, id, wallet, bond)?;
    println!("attestation {id}");
    Ok((Some(id), report))
}

/// Submits a version for a bounty, as its worker, and settles the task:
/// the result claim rests on the attestations given, the worker's task
/// bond and the claims the task requires. Returns the result claim.
pub fn submit(
    session: &mut Session<'_>,
    ledger: &dyn Ledger,
    wallet: &str,
    task: ObjectId,
    dir: &Path,
    attestations: &[ClaimId],
) -> Result<ClaimId, CliError> {
    let spec = spec_of(ledger, task, None).ok();
    let (bytes, root, name, _) = crate::skills::pack(dir)?;
    if let Some(spec) = &spec {
        if spec.skill != name {
            return Err(CliError::Usage(format!(
                "task {} is for skill {}, not {name}",
                task.0.short(),
                spec.skill
            )));
        }
    }
    crate::skills::store_bundle(&bytes, &root)?;
    let who = session.id(wallet);
    // Verified by others: the ledger refuses an attestation of one's own
    // version (spec v1.15); saying so here saves the fee.
    for attestation in attestations {
        let stored = crate::tasks::stored_claim(ledger, &attestation.to_string())?;
        if stored.issuer == who {
            return Err(CliError::Usage(format!(
                "attestation {} is your own: a version is verified by others",
                attestation.short()
            )));
        }
    }
    let body = crate::tasks::result_body(ledger, task, &who, &bytes, attestations)?;
    let id = claim_id(&who, &body);
    let view = session.send(wallet, TxKind::IssueClaim { claim: body }, Vec::new())?;
    crate::agents::print_receipt(session, &view);
    if view.status != ExecStatus::Success {
        return Err(CliError::Transaction("the result was refused".into()));
    }
    joined(ledger, id, wallet, sion_exec::task::WORKER_BOND_LABEL)?;
    println!("result {id}");
    let result = crate::tasks::stored_claim(ledger, &id.to_string())?;
    let attested = attestations
        .iter()
        .map(|a| crate::tasks::stored_claim(ledger, &a.to_string()).map(|c| (c.issuer, c.body)))
        .collect::<Result<Vec<_>, _>>()?;
    let view = session.send(
        wallet,
        TxKind::SettleTask {
            task,
            result: result.body,
            attestations: attested,
        },
        Vec::new(),
    )?;
    crate::agents::print_receipt(session, &view);
    if view.status != ExecStatus::Success {
        return Err(CliError::Transaction(format!(
            "task {} was not settled",
            task.0.short()
        )));
    }
    Ok(id)
}

/// Checks a task result offered as evidence for a release of the bundle
/// `bytes` following `previous`: a result of a settled bounty for a better
/// version of `previous`, for these very bytes. Returns what the release
/// records of it.
pub fn evidence(
    ledger: &dyn Ledger,
    result: ClaimId,
    previous: Option<ClaimId>,
    bytes: &[u8],
) -> Result<Value, CliError> {
    let stored = crate::tasks::stored_claim(ledger, &result.to_string())?;
    if stored.body.schema != RESULT_SCHEMA {
        return Err(CliError::Usage(format!(
            "claim {} is not a task's result",
            result.short()
        )));
    }
    let Payload::Public(payload) = &stored.body.payload else {
        return Err(CliError::Usage("a task result's payload is public".into()));
    };
    let committed: TaskResult = sion_core::encode::decode(payload)
        .map_err(|e| CliError::Protocol(format!("result {}: {e}", result.short())))?;
    if committed.output != crate::tasks::output_commitment(bytes) {
        return Err(CliError::Verification(format!(
            "result {} is for other bytes than this skill's bundle",
            result.short()
        )));
    }
    let task = crate::tasks::load_task(ledger, committed.task)?;
    match &task.state {
        TaskState::Settled { result: paid, .. } if *paid == result => {}
        _ => {
            return Err(CliError::Usage(format!(
                "task {} was not settled with result {}",
                committed.task.0.short(),
                result.short()
            )))
        }
    }
    let spec = spec_of(ledger, committed.task, None)?;
    if previous != Some(spec.release) {
        return Err(CliError::Usage(format!(
            "the task asks for a better version of release {}: give --previous {}",
            spec.release.short(),
            spec.release
        )));
    }
    Ok(json!({
        "result": result.to_string(),
        "task": committed.task.0.to_hex(),
        "evals": spec.evals.to_hex(),
        "pass": spec.pass,
        "cases": spec.cases,
    }))
}

/// A release and those before it, newest first: version, status, what it
/// rests on, and the evidence for each step.
pub fn lineage(ledger: &dyn Ledger, id: &str) -> Result<Vec<String>, CliError> {
    let mut lines = Vec::new();
    let mut next = Some(id.to_string());
    let mut seen = BTreeSet::new();
    while let Some(claim) = next.take() {
        let release = crate::skills::release(ledger, &claim)?;
        if !seen.insert(release.claim) {
            break;
        }
        let status = crate::skills::status_word(&ledger.claim(release.claim)?["status"]);
        lines.push(format!(
            "{} {}: release {}, bundle {}, {status}",
            release.name,
            release.version,
            release.claim.short(),
            release.bundle.short()
        ));
        if !release.audits.is_empty() {
            lines.push(format!(
                "  audits: {}",
                release
                    .audits
                    .iter()
                    .map(|a| a.short())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        for evidence in &release.evidence {
            let short = |key: &str| {
                evidence[key]
                    .as_str()
                    .map(|s| s.chars().take(8).collect::<String>())
                    .unwrap_or_default()
            };
            lines.push(format!(
                "  verified: task {} paid for result {}, which passed at least {} of {} cases of evaluation set {}",
                short("task"),
                short("result"),
                evidence["pass"],
                evidence["cases"],
                short("evals")
            ));
        }
        next = release.previous.map(|p| p.to_string());
    }
    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sion_core::hash::hash_domain;

    #[test]
    fn a_spec_reads_back_and_commits_to_its_bytes() {
        let spec = Spec {
            skill: "pay-invoices".into(),
            release: ClaimId::from_hash(hash_domain("t", b"release")),
            bundle: hash_domain("t", b"bundle"),
            evals: hash_domain("t", b"evals"),
            pass: 2,
            cases: 3,
        };
        let bytes = spec.bytes();
        assert_eq!(Spec::parse(&bytes).unwrap(), spec);
        let other = Spec { pass: 3, ..spec };
        assert_ne!(
            crate::tasks::spec_commitment(&bytes),
            crate::tasks::spec_commitment(&other.bytes())
        );
        assert!(Spec::parse(br#"{"kind": "something-else"}"#).is_err());
    }
}
