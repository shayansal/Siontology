//! Sealed exams: what an agent can do, shown on tasks nobody could have
//! prepared it for.
//!
//! A score on a public benchmark says little: the tasks may have been
//! seen, trained on or fitted to. A sealed exam fixes the order of things
//! on the ledger so that they cannot have been:
//!
//! 1. **The exam is sealed.** Its author encrypts the tasks and publishes
//!    a claim with their digest ([`Exam`]). The key is split among
//!    trustees, any `threshold` of whom can open it; the claim holds the
//!    digest of each trustee's share. Until enough shares are released,
//!    nobody but the author can read the tasks.
//! 2. **Agents enter.** An entry is a claim that fixes an agent: its
//!    models, what its operator tells it and what it has learned, in full
//!    ([`Entry`]). An entry counts only if it was issued before the exam
//!    opened.
//! 3. **The exam opens.** Trustees release their shares in claims
//!    ([`Released`]). With `threshold` of them the tasks can be read by
//!    anyone, and the Pulse of the share that completed them is when the
//!    exam opened.
//! 4. **Anyone runs it.** A runner runs an entry's agent on the tasks and
//!    publishes what it found ([`Outcome`]). The entrant's own run is one
//!    result among others: a reader looks at what runners who are not the
//!    entrant found.
//!
//! Afterwards the tasks are public, so every result can be run again by
//! anyone, and the exam cannot be used again.
//!
//! An exam's author has read its tasks. Where no single party should
//! have, several authors each seal an exam and an agent is entered in all
//! of them: the same agent is recognised across exams by its digest
//! ([`Entry::agent`]), and [`across`] sets its results side by side, so
//! that an agent that does well only on one author's tasks shows.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chacha20poly1305::aead::{Aead, KeyInit, Payload as Sealing};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use clap::Subcommand;
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sion_agent::compare::TaskSet;
use sion_core::hash::Hash32;
use sion_core::ids::{AccountId, ClaimId};
use sion_crypto::shamir::{self, Share};
use sion_exec::claim::{formulas, ClaimBody, Payload};

use crate::error::CliError;
use crate::feeds::{self, Feed};
use crate::improve_loop::Variant;
use crate::ledger::Ledger;
use crate::session::Session;

/// The schema of a claim sealing an exam.
pub const EXAM_SCHEMA: &str = "notary.exam/v1";

/// The schema of a claim entering an agent.
pub const ENTRY_SCHEMA: &str = "notary.entry/v1";

/// The schema of a claim releasing a trustee's share.
pub const SHARE_SCHEMA: &str = "notary.share/v1";

/// The schema of a claim saying what a run of an entry found.
pub const RESULT_SCHEMA: &str = "notary.result/v1";

/// An exam, as its claim holds it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Exam {
    /// Its name: lowercase letters, digits and hyphens.
    pub name: String,
    /// What it examines, in a line.
    pub about: String,
    /// The digest of the tasks, as they will be read once it opens, with
    /// the salt sealed beside them: it tells nothing of tasks one could
    /// guess.
    pub tasks: String,
    /// The digest of the tasks as they are sealed.
    pub sealed: String,
    /// How many tasks there are.
    pub cases: u64,
    /// The trustees who hold the shares of its key.
    pub trustees: Vec<AccountId>,
    /// The digest of each trustee's share, in the trustees' order.
    pub shares: Vec<String>,
    /// How many shares open it.
    pub threshold: u8,
    /// How many times each task is run.
    pub runs: u32,
    /// The tokens each run of a task may use.
    pub budget_tokens: u64,
}

/// An exam as a file anyone may hold: what its claim says, and the sealed
/// tasks.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExamFile {
    /// The exam's claim.
    pub claim: String,
    /// What the claim holds.
    pub exam: Exam,
    /// The nonce the tasks were sealed with, in hex.
    pub nonce: String,
    /// The sealed tasks, in hex.
    pub sealed: String,
}

/// A trustee's share, as its trustee is handed it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShareFile {
    /// The exam's name.
    pub exam: String,
    /// The share.
    pub share: Share,
}

/// An entry: the agent examined, in full, so that anyone with its models
/// can run it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// The exam's claim.
    pub exam: String,
    /// What the entrant calls it.
    pub name: String,
    /// The models it runs on, best first.
    pub models: Vec<String>,
    /// What its operator tells it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub instructions: String,
    /// What it has learned.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub learned: String,
    /// Whether it says what it expects of its calls.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub expect: bool,
}

impl Entry {
    /// The digest of the agent it fixes, the same in every exam the agent
    /// is entered in: its models, what its operator tells it, what it has
    /// learned and whether it says what it expects.
    pub fn agent(&self) -> Hash32 {
        let fixed = serde_json::json!({
            "models": self.models,
            "instructions": self.instructions,
            "learned": self.learned,
            "expect": self.expect,
        });
        sion_core::hash::hash_domain("sion-cli/exam/agent/v1", fixed.to_string().as_bytes())
    }
}

/// The most bytes an entry may be written in: what a claim holds, with
/// room to spare.
pub const MAX_ENTRY: usize = 60_000;

/// A trustee's share, released.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Released {
    /// The exam's claim.
    pub exam: String,
    /// The share.
    pub share: Share,
}

/// What a run of an entry found.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Outcome {
    /// The exam's claim.
    pub exam: String,
    /// The entry's claim.
    pub entry: String,
    /// Runs of tasks that passed.
    pub passed: u64,
    /// Runs of tasks made.
    pub total: u64,
    /// Whether each task passed every run, in the exam's order.
    pub tasks: Vec<bool>,
    /// The version of `sion` that ran it.
    pub sion: String,
}

fn exam_key(name: &str) -> String {
    format!("exam:{name}")
}

fn entry_key(exam: &ClaimId) -> String {
    format!("exam-entry:{}", feeds::short_key(exam.as_hash()))
}

fn share_key(exam: &ClaimId) -> String {
    format!("exam-share:{}", feeds::short_key(exam.as_hash()))
}

fn result_key(entry: &ClaimId) -> String {
    format!("exam-result:{}", feeds::short_key(entry.as_hash()))
}

fn share_digest(share: &Share) -> String {
    let mut bytes = vec![share.x];
    bytes.extend_from_slice(&share.y);
    sion_core::hash::hash_domain("sion-cli/exam/share/v1", &bytes).to_hex()
}

/// The bytes of the salt sealed before the tasks.
const SALT: usize = 32;

/// The digest of what is sealed: the salt, then the tasks.
fn tasks_digest(salted: &[u8]) -> String {
    sion_core::hash::hash_domain("sion-cli/exam/tasks/v1", salted).to_hex()
}

/// The first characters of a digest, as lines show it.
fn short(digest: &Hash32) -> String {
    digest.to_hex()[..8].to_string()
}

fn claim_of(hex: &str) -> Result<ClaimId, CliError> {
    Hash32::from_hex(hex.trim())
        .map(ClaimId::from_hash)
        .map_err(|e| CliError::Usage(format!("{hex:?} is not a claim's id: {e}")))
}

/// A claim that rests on its issuer's signature alone.
fn signed(schema: &str, subject: String, payload: Vec<u8>) -> ClaimBody {
    ClaimBody {
        schema: schema.into(),
        subject,
        payload: Payload::Public(payload),
        formula: sion_assurance::Formula::and(vec![formulas::dual_signature()]),
        adjudicator: None,
        detection_permille: 0,
        valid_for: 1_000_000,
        challenge_period: 0,
    }
}

fn written<T: Serialize>(value: &T) -> Result<Vec<u8>, CliError> {
    serde_json::to_vec(value).map_err(|e| CliError::Usage(format!("it could not be written: {e}")))
}

/// An exam just sealed: what its claim will hold, the sealed tasks, and
/// the share of each trustee.
#[derive(Clone, Debug)]
pub struct Sealed {
    /// What the claim holds.
    pub exam: Exam,
    /// The nonce the tasks were sealed with.
    pub nonce: [u8; 12],
    /// The sealed tasks.
    pub sealed: Vec<u8>,
    /// A share for each trustee, in the trustees' order.
    pub shares: Vec<Share>,
}

/// Seals `tasks` as an exam: the tasks encrypted under a fresh key, the
/// key split among `trustees`, any `threshold` of whom open it.
pub fn seal(
    name: &str,
    about: &str,
    tasks: &[u8],
    trustees: &[AccountId],
    threshold: u8,
    runs: u32,
    budget_tokens: u64,
) -> Result<Sealed, CliError> {
    let named = !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if !named {
        return Err(CliError::Usage(format!(
            "{name:?} is not an exam's name: lowercase letters, digits and hyphens"
        )));
    }
    let set = TaskSet::parse(tasks).map_err(CliError::Usage)?;
    let distinct: std::collections::BTreeSet<&AccountId> = trustees.iter().collect();
    if distinct.len() != trustees.len() || trustees.is_empty() || trustees.len() > 255 {
        return Err(CliError::Usage(
            "an exam has 1 to 255 trustees, each named once".into(),
        ));
    }
    if threshold == 0 || usize::from(threshold) > trustees.len() {
        return Err(CliError::Usage(format!(
            "{threshold} of {} trustees cannot open an exam",
            trustees.len()
        )));
    }
    let mut key = [0u8; 32];
    let mut nonce = [0u8; 12];
    let mut salted = vec![0u8; SALT];
    OsRng.fill_bytes(&mut key);
    OsRng.fill_bytes(&mut nonce);
    OsRng.fill_bytes(&mut salted);
    salted.extend_from_slice(tasks);
    let sealed = ChaCha20Poly1305::new(Key::from_slice(&key))
        .encrypt(
            Nonce::from_slice(&nonce),
            Sealing {
                msg: &salted,
                aad: name.as_bytes(),
            },
        )
        .map_err(|_| CliError::Usage("the tasks could not be sealed".into()))?;
    let shares = shamir::split(&key, threshold, trustees.len() as u8, &mut OsRng)
        .map_err(|e| CliError::Usage(format!("the key could not be split: {e}")))?;
    let exam = Exam {
        name: name.to_string(),
        about: about.split_whitespace().collect::<Vec<_>>().join(" "),
        tasks: tasks_digest(&salted),
        sealed: sion_core::hash::hash_domain("sion-cli/exam/sealed/v1", &sealed).to_hex(),
        cases: set.cases.len() as u64,
        trustees: trustees.to_vec(),
        shares: shares.iter().map(share_digest).collect(),
        threshold,
        runs: runs.max(1),
        budget_tokens,
    };
    Ok(Sealed {
        exam,
        nonce,
        sealed,
        shares,
    })
}

/// Publishes `exam` as `wallet`'s claim, named under the exam's name.
pub fn publish(session: &mut Session<'_>, wallet: &str, exam: &Exam) -> Result<ClaimId, CliError> {
    let body = signed(
        EXAM_SCHEMA,
        format!("exam {}: {} sealed task(s)", exam.name, exam.cases),
        written(exam)?,
    );
    crate::vouching::publish(session, wallet, body, Some(exam_key(&exam.name)))
}

/// The exam `file` holds, as the ledger has it: the claim it names is an
/// exam, and holds what the file says it does.
pub fn on_ledger(
    ledger: &dyn Ledger,
    file: &ExamFile,
) -> Result<(ClaimId, AccountId, u64), CliError> {
    let claim = claim_of(&file.claim)?;
    let read = feeds::read::<Exam>(ledger, claim, EXAM_SCHEMA)?.ok_or_else(|| {
        CliError::Usage(format!(
            "claim {} is not an exam on this ledger",
            claim.short()
        ))
    })?;
    if read.says != file.exam {
        return Err(CliError::Usage(
            "the file is not the exam its claim holds".into(),
        ));
    }
    let sealed = hex::decode(&file.sealed).unwrap_or_default();
    if sion_core::hash::hash_domain("sion-cli/exam/sealed/v1", &sealed).to_hex() != file.exam.sealed
    {
        return Err(CliError::Usage(
            "the file's sealed tasks are not those the exam's claim names".into(),
        ));
    }
    Ok((claim, read.issuer, read.issued_at))
}

/// Enters `entry` as `wallet`'s claim.
pub fn enter(session: &mut Session<'_>, wallet: &str, entry: &Entry) -> Result<ClaimId, CliError> {
    let exam = claim_of(&entry.exam)?;
    if entry.models.is_empty() {
        return Err(CliError::Usage(
            "an entry names the models its agent runs on".into(),
        ));
    }
    let payload = written(entry)?;
    if payload.len() > MAX_ENTRY {
        return Err(CliError::Usage(format!(
            "what the agent is told and has learned takes {} bytes: an entry holds at most \
             {MAX_ENTRY}",
            payload.len()
        )));
    }
    let body = signed(
        ENTRY_SCHEMA,
        format!(
            "enters {} in exam {}",
            entry.name.chars().take(80).collect::<String>(),
            exam.short()
        ),
        payload,
    );
    crate::vouching::publish(session, wallet, body, Some(entry_key(&exam)))
}

/// Releases `share` of the exam `claim` as `wallet`, a trustee of it whose
/// share it is.
pub fn release(
    session: &mut Session<'_>,
    wallet: &str,
    claim: ClaimId,
    exam: &Exam,
    share: &Share,
) -> Result<ClaimId, CliError> {
    let me = session.id(wallet);
    let Some(place) = exam.trustees.iter().position(|t| *t == me) else {
        return Err(CliError::Usage(format!(
            "{wallet} is not a trustee of exam {}",
            exam.name
        )));
    };
    if exam.shares.get(place) != Some(&share_digest(share)) {
        return Err(CliError::Usage(
            "that is not the share the exam names for this trustee".into(),
        ));
    }
    let body = signed(
        SHARE_SCHEMA,
        format!("releases a share of exam {}", exam.name),
        written(&Released {
            exam: claim.as_hash().to_hex(),
            share: share.clone(),
        })?,
    );
    crate::vouching::publish(session, wallet, body, Some(share_key(&claim)))
}

/// What the ledger holds about an exam.
#[derive(Clone, Debug, Default)]
pub struct Found {
    /// The shares released by trustees, each trustee's once: the share and
    /// the Pulse it was released in, in the order released.
    pub shares: Vec<(Share, u64)>,
    /// The entries: the claim, who entered, the Pulse, the entry.
    pub entries: Vec<(ClaimId, AccountId, u64, Entry)>,
    /// What runs found: the claim, the runner, what it found.
    pub results: Vec<(ClaimId, AccountId, Outcome)>,
}

impl Found {
    /// The Pulse the exam opened in: that of the share that made
    /// `threshold` of them, if there are that many.
    pub fn opened_at(&self, threshold: u8) -> Option<u64> {
        self.shares
            .get(usize::from(threshold).checked_sub(1)?)
            .map(|(_, at)| *at)
    }
}

/// Reads what the ledger holds about the exam `claim`: the shares its
/// trustees released, the entries, and the results of runs.
pub fn found(ledger: &dyn Ledger, claim: ClaimId, exam: &Exam) -> Result<Found, CliError> {
    let hex = claim.as_hash().to_hex();
    // Read in part, an exam would seem sealed when it is open, or an
    // entry made in time would be missing.
    let sealed_at =
        feeds::read::<Exam>(ledger, claim, EXAM_SCHEMA)?.map_or(0, |read| read.issued_at);
    feeds::need_since(ledger, sealed_at, "the exam's shares, entries and results")?;
    let mut feed = Feed {
        next: sealed_at,
        ..Feed::default()
    };
    feed.scan(ledger, &["exam-share:", "exam-entry:", "exam-result:"])?;
    let mut all = Found::default();
    let mut released: Vec<AccountId> = Vec::new();
    for entry in feed.under(&share_key(&claim)) {
        let Some(read) = feeds::read::<Released>(ledger, entry.claim, SHARE_SCHEMA)? else {
            continue;
        };
        let place = exam.trustees.iter().position(|t| *t == read.issuer);
        let fits = place.is_some_and(|at| exam.shares[at] == share_digest(&read.says.share));
        if read.issuer != entry.owner
            || read.says.exam != hex
            || !fits
            || released.contains(&read.issuer)
        {
            continue;
        }
        released.push(read.issuer);
        all.shares.push((read.says.share, read.issued_at));
    }
    all.shares.sort_by_key(|(_, at)| *at);
    for entry in feed.under(&entry_key(&claim)) {
        let Some(read) = feeds::read::<Entry>(ledger, entry.claim, ENTRY_SCHEMA)? else {
            continue;
        };
        if read.issuer != entry.owner || read.says.exam != hex {
            continue;
        }
        if !all.entries.iter().any(|(c, ..)| *c == entry.claim) {
            all.entries
                .push((entry.claim, read.issuer, read.issued_at, read.says));
        }
    }
    let entries: Vec<ClaimId> = all.entries.iter().map(|(c, ..)| *c).collect();
    for entry in feed.under("exam-result:") {
        let Some(read) = feeds::read::<Outcome>(ledger, entry.claim, RESULT_SCHEMA)? else {
            continue;
        };
        let Ok(of) = claim_of(&read.says.entry) else {
            continue;
        };
        if read.issuer != entry.owner
            || read.says.exam != hex
            || !entries.contains(&of)
            || result_key(&of) != entry.key
        {
            continue;
        }
        // A runner's latest word on an entry replaces its earlier one.
        all.results
            .retain(|(_, by, had)| !(*by == read.issuer && had.entry == read.says.entry));
        all.results.push((entry.claim, read.issuer, read.says));
    }
    Ok(all)
}

/// The exam's tasks, read with the shares released: `None` while fewer
/// than its threshold are.
pub fn opened(file: &ExamFile, found: &Found) -> Result<Option<(TaskSet, Vec<u8>)>, CliError> {
    let exam = &file.exam;
    if found.shares.len() < usize::from(exam.threshold) {
        return Ok(None);
    }
    let shares: Vec<Share> = found
        .shares
        .iter()
        .map(|(share, _)| share.clone())
        .collect();
    let key = shamir::combine(&shares, exam.threshold)
        .map_err(|e| CliError::Usage(format!("the shares do not make a key: {e}")))?;
    let nonce = hex::decode(&file.nonce).unwrap_or_default();
    let sealed = hex::decode(&file.sealed).unwrap_or_default();
    if key.len() != 32 || nonce.len() != 12 {
        return Err(CliError::Usage("the exam's file is not whole".into()));
    }
    let salted = ChaCha20Poly1305::new(Key::from_slice(&key))
        .decrypt(
            Nonce::from_slice(&nonce),
            Sealing {
                msg: &sealed,
                aad: exam.name.as_bytes(),
            },
        )
        .map_err(|_| CliError::Usage("the shares released do not open the sealed tasks".into()))?;
    if salted.len() < SALT || tasks_digest(&salted) != exam.tasks {
        return Err(CliError::Usage(
            "the tasks that opened are not those the exam's claim names".into(),
        ));
    }
    let tasks = salted[SALT..].to_vec();
    let set = TaskSet::parse(&tasks).map_err(CliError::Usage)?;
    Ok(Some((set, tasks)))
}

/// Runs `entry`'s agent on the opened tasks of the exam and returns what
/// it found.
pub fn run(
    exam_claim: ClaimId,
    exam: &Exam,
    entry_claim: ClaimId,
    entry: &Entry,
    set: &TaskSet,
) -> Result<Outcome, CliError> {
    let variant = Variant {
        learned: entry.learned.clone(),
        models: entry.models.clone(),
    };
    let digest = Hash32::from_hex(&exam.tasks)
        .map_err(|e| CliError::Usage(format!("the exam's digest: {e}")))?;
    let operator = Some(entry.instructions.as_str()).filter(|text| !text.trim().is_empty());
    let scored = crate::improve_loop::score_as(
        operator,
        entry.expect,
        &variant,
        set,
        digest,
        exam.runs,
        exam.budget_tokens,
    )?;
    Ok(Outcome {
        exam: exam_claim.as_hash().to_hex(),
        entry: entry_claim.as_hash().to_hex(),
        passed: scored.outcome.passed as u64,
        total: scored.outcome.total as u64,
        tasks: scored.cases,
        sion: env!("CARGO_PKG_VERSION").to_string(),
    })
}

/// Publishes `outcome` as `wallet`'s claim.
pub fn report(
    session: &mut Session<'_>,
    wallet: &str,
    outcome: &Outcome,
) -> Result<ClaimId, CliError> {
    let entry = claim_of(&outcome.entry)?;
    let body = signed(
        RESULT_SCHEMA,
        format!(
            "ran entry {}: {}/{} runs passed",
            entry.short(),
            outcome.passed,
            outcome.total
        ),
        written(outcome)?,
    );
    crate::vouching::publish(session, wallet, body, Some(result_key(&entry)))
}

/// Where an exam stands, in lines: whether it is open, each entry and
/// whether it counts, and what each run of it found.
pub fn lines(exam: &Exam, author: &AccountId, found: &Found) -> Vec<String> {
    let opened = found.opened_at(exam.threshold);
    let mut lines = vec![format!(
        "exam {} by {}: {} task(s), {} run(s) each; {}",
        exam.name,
        author.short(),
        exam.cases,
        exam.runs,
        exam.about
    )];
    lines.push(match opened {
        Some(at) => format!(
            "  opened at Pulse {at}: {} of its {} trustees released their shares, {} open it",
            found.shares.len(),
            exam.trustees.len(),
            exam.threshold
        ),
        None => format!(
            "  sealed: {} of the {} shares that open it are released",
            found.shares.len(),
            exam.threshold
        ),
    });
    if found.entries.is_empty() {
        lines.push("  no agent is entered".to_string());
    }
    for (claim, entrant, at, entry) in &found.entries {
        let counts = opened.map_or(true, |opened| *at < opened);
        lines.push(format!(
            "  entry {} by {}: {} on {} (agent {}, entered at Pulse {at}{})",
            claim.short(),
            entrant.short(),
            entry.name,
            entry.models.join(", "),
            short(&entry.agent()),
            if counts {
                ""
            } else {
                ", after the exam opened: it does not count"
            }
        ));
        let mut by_runner: BTreeMap<bool, Vec<String>> = BTreeMap::new();
        for (_, runner, outcome) in found
            .results
            .iter()
            .filter(|(_, _, o)| o.entry == claim.as_hash().to_hex())
        {
            by_runner
                .entry(runner == entrant)
                .or_default()
                .push(format!(
                    "{}/{} by {}",
                    outcome.passed,
                    outcome.total,
                    runner.short()
                ));
        }
        match (by_runner.get(&false), by_runner.get(&true)) {
            (None, None) => lines.push("    not run yet".to_string()),
            (others, own) => {
                if let Some(others) = others {
                    lines.push(format!("    run by others: {}", others.join("; ")));
                }
                if let Some(own) = own {
                    lines.push(format!("    run by its entrant: {}", own.join("; ")));
                }
                if others.is_none() {
                    lines.push("    no one but its entrant has run it".to_string());
                }
            }
        }
    }
    lines
}

/// The runs of tasks that runners other than the entrant found passed
/// and made, over the entries of the agent `agent` that count in an exam:
/// `None` where the agent is not entered in time, `Some((0, 0))` where no
/// one but its entrant has run it.
pub fn by_others(exam: &Exam, found: &Found, agent: &Hash32) -> Option<(u64, u64)> {
    let opened = found.opened_at(exam.threshold);
    let mut entered = false;
    let (mut passed, mut total) = (0, 0);
    for (claim, entrant, at, entry) in &found.entries {
        if entry.agent() != *agent || opened.is_some_and(|opened| *at >= opened) {
            continue;
        }
        entered = true;
        let hex = claim.as_hash().to_hex();
        for (_, runner, outcome) in &found.results {
            if outcome.entry == hex && runner != entrant {
                passed += outcome.passed;
                total += outcome.total;
            }
        }
    }
    entered.then_some((passed, total))
}

/// The agents entered in several exams, each with what runners other than
/// its entrant found in every one of them, in lines: an agent that does
/// well only on one author's tasks shows here.
pub fn across(exams: &[(Exam, Found)]) -> Vec<String> {
    let mut agents: BTreeMap<Hash32, String> = BTreeMap::new();
    for (_, found) in exams {
        for (_, _, _, entry) in &found.entries {
            agents
                .entry(entry.agent())
                .or_insert_with(|| entry.name.clone());
        }
    }
    let mut lines = Vec::new();
    for (agent, name) in &agents {
        let parts: Vec<String> = exams
            .iter()
            .map(|(exam, found)| match by_others(exam, found, agent) {
                None if found.entries.iter().any(|(.., e)| e.agent() == *agent) => {
                    format!("{}: entered after it opened", exam.name)
                }
                None => format!("{}: not entered", exam.name),
                Some((_, 0)) => format!("{}: not run by others", exam.name),
                Some((passed, total)) => format!("{}: {passed}/{total}", exam.name),
            })
            .collect();
        lines.push(format!(
            "  agent {} ({name}), as others ran it: {}",
            short(agent),
            parts.join("; ")
        ));
    }
    lines
}

/// `sion agent exam`.
#[derive(Clone, Debug, Subcommand)]
pub enum ExamCommand {
    /// Seal a set of tasks as an exam, as you: the tasks are encrypted,
    /// their digest published, and the key split among trustees. Writes the
    /// exam's file, which anyone may hold, and a file for each trustee with
    /// its share, which you hand to that trustee and no one else.
    Seal {
        /// The exam's name (lowercase letters, digits and hyphens).
        name: String,
        /// The tasks (a set, as `sion agent improve harvest` writes).
        #[arg(long, value_name = "FILE")]
        tasks: PathBuf,
        /// What it examines, in a line.
        #[arg(long, default_value = "")]
        about: String,
        /// A trustee (a wallet's name or an account's id; repeat for more).
        #[arg(long = "trustee", value_name = "ACCOUNT", required = true)]
        trustees: Vec<String>,
        /// How many trustees' shares open it.
        #[arg(long)]
        threshold: u8,
        /// How many times each task is run.
        #[arg(long, default_value_t = 3)]
        runs: u32,
        /// The tokens each run of a task may use.
        #[arg(long = "budget-tokens", default_value_t = 20_000)]
        budget_tokens: u64,
        /// Your wallet.
        #[arg(long)]
        wallet: String,
        /// The directory to write the files in.
        #[arg(long, value_name = "DIR", default_value = ".")]
        out: PathBuf,
    },
    /// Enter a profile's agent in an exam, as you: a claim that fixes its
    /// models, what you tell it and what it has learned, in full. An entry
    /// counts only if it is made before the exam opens.
    Enter {
        /// The exam's file.
        exam: PathBuf,
        /// The profile.
        profile: String,
        /// Your wallet.
        #[arg(long)]
        wallet: String,
    },
    /// Release your share of an exam's key, as one of its trustees. When
    /// enough trustees have, the tasks can be read by anyone and no further
    /// entry counts.
    Release {
        /// The exam's file.
        exam: PathBuf,
        /// Your share's file.
        share: PathBuf,
        /// Your wallet.
        #[arg(long)]
        wallet: String,
    },
    /// Run an entry's agent on an opened exam's tasks, with your own access
    /// to its models; with --wallet, publish what you found.
    Run {
        /// The exam's file.
        exam: PathBuf,
        /// The entry (its claim's id, or a unique prefix of it).
        entry: String,
        /// Publish what the run found as this wallet's claim.
        #[arg(long)]
        wallet: Option<String>,
    },
    /// Where an exam stands: whether it is open, the entries and whether
    /// each counts, and what runs of each found, by whom. With several
    /// exams, by different authors, also each agent's results side by side:
    /// no author has read another's tasks.
    Results {
        /// The exam's file (repeat for several exams).
        #[arg(required = true)]
        exams: Vec<PathBuf>,
    },
    /// Write an opened exam's tasks to a file, for anyone to read and run.
    Tasks {
        /// The exam's file.
        exam: PathBuf,
        /// Where to write them.
        #[arg(long, value_name = "FILE")]
        out: PathBuf,
    },
}

fn read<T: serde::de::DeserializeOwned>(path: &Path, what: &str) -> Result<T, CliError> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| CliError::Usage(format!("{}: {e}", path.display())))?;
    serde_json::from_str(&text)
        .map_err(|e| CliError::Usage(format!("{} is not {what}: {e}", path.display())))
}

/// Runs an exam command.
pub fn command(
    session: &mut Session<'_>,
    ledger: &dyn Ledger,
    command: ExamCommand,
) -> Result<(), CliError> {
    match command {
        ExamCommand::Seal {
            name,
            tasks,
            about,
            trustees,
            threshold,
            runs,
            budget_tokens,
            wallet,
            out,
        } => {
            let bytes = std::fs::read(&tasks)
                .map_err(|e| CliError::Usage(format!("{}: {e}", tasks.display())))?;
            let accounts: Vec<AccountId> = trustees.iter().map(|t| session.id(t)).collect();
            let Sealed {
                exam,
                nonce,
                sealed,
                shares,
            } = seal(
                &name,
                &about,
                &bytes,
                &accounts,
                threshold,
                runs,
                budget_tokens,
            )?;
            let claim = publish(session, &wallet, &exam)?;
            let file = ExamFile {
                claim: claim.as_hash().to_hex(),
                exam: exam.clone(),
                nonce: hex::encode(nonce),
                sealed: hex::encode(sealed),
            };
            let path = out.join(format!("{name}.exam.json"));
            crate::profiles::write_json(&path, &file)?;
            println!(
                "sealed exam {name}: {} task(s), claim {}",
                exam.cases,
                claim.as_hash().to_hex()
            );
            println!("  the exam, for anyone: {}", path.display());
            for ((trustee, account), share) in trustees.iter().zip(&accounts).zip(shares) {
                let path = out.join(format!("{name}.share-{}.json", share.x));
                crate::profiles::write_json(
                    &path,
                    &ShareFile {
                        exam: name.clone(),
                        share,
                    },
                )?;
                println!(
                    "  the share of {trustee} ({}), for that trustee alone: {}",
                    account.short(),
                    path.display()
                );
            }
            println!(
                "  {threshold} of {} trustees open it (sion agent exam release)",
                accounts.len()
            );
            Ok(())
        }
        ExamCommand::Enter {
            exam,
            profile,
            wallet,
        } => {
            let file: ExamFile = read(&exam, "an exam")?;
            let (claim, _, _) = on_ledger(ledger, &file)?;
            let all = found(ledger, claim, &file.exam)?;
            if let Some(at) = all.opened_at(file.exam.threshold) {
                return Err(CliError::Usage(format!(
                    "exam {} opened at Pulse {at}: an entry made now does not count",
                    file.exam.name
                )));
            }
            let profile = crate::profiles::get(&profile)?;
            let variant = Variant::of(&profile);
            let entry = Entry {
                exam: file.claim.clone(),
                name: profile.name.clone(),
                models: variant.models,
                instructions: profile.instructions.clone().unwrap_or_default(),
                learned: variant.learned,
                expect: profile.expect,
            };
            let me = session.id(&wallet);
            if let Some((had, ..)) = all
                .entries
                .iter()
                .find(|(_, by, _, was)| *by == me && was.agent() == entry.agent())
            {
                return Err(CliError::Usage(format!(
                    "this agent is already entered in exam {}, as claim {}",
                    file.exam.name,
                    had.as_hash().to_hex()
                )));
            }
            let entered = enter(session, &wallet, &entry)?;
            println!(
                "entered {} in exam {}: claim {}",
                entry.name,
                file.exam.name,
                entered.as_hash().to_hex()
            );
            println!("  fixed: its models, what you tell it and what it has learned, in full");
            Ok(())
        }
        ExamCommand::Release {
            exam,
            share,
            wallet,
        } => {
            let file: ExamFile = read(&exam, "an exam")?;
            let share: ShareFile = read(&share, "a share")?;
            let (claim, _, _) = on_ledger(ledger, &file)?;
            let before = found(ledger, claim, &file.exam)?;
            if before.shares.iter().any(|(had, _)| *had == share.share) {
                println!("your share of exam {} is already released", file.exam.name);
            } else {
                let released = release(session, &wallet, claim, &file.exam, &share.share)?;
                println!(
                    "released your share of exam {}: claim {}",
                    file.exam.name,
                    released.short()
                );
            }
            let all = found(ledger, claim, &file.exam)?;
            match all.opened_at(file.exam.threshold) {
                Some(at) => {
                    println!("  the exam is open, as of Pulse {at}: anyone can read its tasks")
                }
                None => println!(
                    "  {} of the {} shares that open it are released",
                    all.shares.len(),
                    file.exam.threshold
                ),
            }
            Ok(())
        }
        ExamCommand::Run {
            exam,
            entry,
            wallet,
        } => {
            let file: ExamFile = read(&exam, "an exam")?;
            let (claim, _, _) = on_ledger(ledger, &file)?;
            let all = found(ledger, claim, &file.exam)?;
            let Some((set, _)) = opened(&file, &all)? else {
                return Err(CliError::Usage(format!(
                    "exam {} is sealed: {} of the {} shares that open it are released",
                    file.exam.name,
                    all.shares.len(),
                    file.exam.threshold
                )));
            };
            let fit: Vec<_> = all
                .entries
                .iter()
                .filter(|(c, ..)| c.as_hash().to_hex().starts_with(entry.trim()))
                .collect();
            let (entry_claim, _, at, entered) = match fit.as_slice() {
                [one] => *one,
                [] => return Err(CliError::Usage(format!("no entry {entry} in this exam"))),
                _ => {
                    return Err(CliError::Usage(format!(
                        "{entry} names more than one entry"
                    )))
                }
            };
            if all
                .opened_at(file.exam.threshold)
                .is_some_and(|opened| *at >= opened)
            {
                return Err(CliError::Usage(
                    "that entry was made after the exam opened: it does not count".into(),
                ));
            }
            let outcome = run(claim, &file.exam, *entry_claim, entered, &set)?;
            println!(
                "ran {} on exam {}: {}/{} runs passed, {} of {} tasks every time",
                entered.name,
                file.exam.name,
                outcome.passed,
                outcome.total,
                outcome.tasks.iter().filter(|p| **p).count(),
                outcome.tasks.len()
            );
            if let Some(wallet) = wallet {
                let me = session.id(&wallet);
                match all
                    .results
                    .iter()
                    .find(|(_, by, had)| *by == me && *had == outcome)
                {
                    Some((had, ..)) => println!(
                        "you already published this result, in claim {}",
                        had.as_hash().to_hex()
                    ),
                    None => {
                        let said = report(session, &wallet, &outcome)?;
                        println!("published in claim {}", said.as_hash().to_hex());
                    }
                }
            }
            Ok(())
        }
        ExamCommand::Results { exams } => {
            let mut all = Vec::new();
            for exam in &exams {
                let file: ExamFile = read(exam, "an exam")?;
                let (claim, author, _) = on_ledger(ledger, &file)?;
                let found = found(ledger, claim, &file.exam)?;
                for line in lines(&file.exam, &author, &found) {
                    println!("{line}");
                }
                all.push((file.exam, found));
            }
            if all.len() > 1 {
                println!("across the {} exams:", all.len());
                for line in across(&all) {
                    println!("{line}");
                }
            }
            Ok(())
        }
        ExamCommand::Tasks { exam, out } => {
            let file: ExamFile = read(&exam, "an exam")?;
            let (claim, _, _) = on_ledger(ledger, &file)?;
            let all = found(ledger, claim, &file.exam)?;
            let Some((set, tasks)) = opened(&file, &all)? else {
                return Err(CliError::Usage(format!(
                    "exam {} is sealed: its tasks cannot be read yet",
                    file.exam.name
                )));
            };
            std::fs::write(&out, tasks)
                .map_err(|e| CliError::Usage(format!("{}: {e}", out.display())))?;
            println!("{} task(s) written to {}", set.cases.len(), out.display());
            Ok(())
        }
    }
}
