//! Datasets of checked work, contributed by those whose work it was.
//!
//! Models are trained on whatever could be gathered, and nobody can say
//! of an example whether the work in it was right, whose it was, or
//! whether its maker agreed. A dataset here is built the other way round:
//!
//! - **A card** ([`Card`]) is a curator's claim: what the dataset is for,
//!   the terms a contributor agrees to, and what counts as checked.
//! - **A contribution** ([`Contribution`]) is its contributor's claim. It
//!   commits to one example (a session, as a line to train on), says what
//!   checked the work in it ([`Evidence`]), and agrees to the card's
//!   terms. The example itself is a file the contributor gives the
//!   curator; nothing of it is on the ledger.
//! - **A version** ([`Version`]) is the curator's claim over the examples
//!   it took: how many, the root of their digests, and how many each
//!   contributor gave. A trainer checks a file of examples against it
//!   ([`check`]).
//! - **A withdrawal** is its contributor's claim: later versions leave
//!   the example out.
//!
//! Everything is found by name in its maker's own index, so none of it
//! depends on how long a node keeps its events.
//!
//! What checked the work is of three strengths. A Task Charter that was
//! settled is the ledger's: the result was accepted under the rule fixed
//! when the task was posted. A check that passed, and a session its
//! operator marked good, are the operator's word.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use clap::{Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sion_core::hash::Hash32;
use sion_core::ids::{AccountId, ClaimId, ObjectId};
use sion_core::merkle;
use sion_exec::claim::{formulas, ClaimBody, Payload};
use sion_exec::task::{TaskResult, TaskState};

use crate::error::CliError;
use crate::feeds;
use crate::ledger::Ledger;
use crate::runtime::Saved;
use crate::session::Session;

/// The schema of a dataset's card.
pub const CARD_SCHEMA: &str = "dataset.card/v1";

/// The schema of a contribution.
pub const CONTRIBUTION_SCHEMA: &str = "dataset.contribution/v1";

/// The schema of a withdrawal.
pub const WITHDRAWN_SCHEMA: &str = "dataset.withdrawn/v1";

/// The schema of a version.
pub const VERSION_SCHEMA: &str = "dataset.version/v1";

/// The most bytes a card's terms may be.
pub const MAX_TERMS: usize = 8_000;

/// What stands in an example for text its contributor took out.
pub const REDACTED: &str = "[redacted]";

/// What can have checked the work in an example.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// A Task Charter settled on the ledger: the result was accepted under
    /// the rule its poster fixed.
    Task,
    /// The session's own check passed (`--verify`): its operator's word.
    Check,
    /// Its operator marked the session good: its operator's word.
    Marked,
}

impl Kind {
    /// A word for it.
    pub fn word(self) -> &'static str {
        match self {
            Kind::Task => "task",
            Kind::Check => "check",
            Kind::Marked => "marked",
        }
    }
}

/// What checked the work in an example.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Evidence {
    /// A settled Task Charter.
    Task {
        /// The task.
        task: ObjectId,
        /// The result it was settled with.
        result: ClaimId,
        /// Whether the output the task was settled with is in the
        /// session, word for word.
        output_in_session: bool,
        /// The contributor's agent that was paid for it, where the
        /// contributor's own account was not.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent: Option<ObjectId>,
    },
    /// The session's check passed.
    Check {
        /// The digest of the check's command.
        command: String,
    },
    /// Its operator marked it good.
    Marked,
}

impl Evidence {
    /// Its kind.
    pub fn kind(&self) -> Kind {
        match self {
            Evidence::Task { .. } => Kind::Task,
            Evidence::Check { .. } => Kind::Check,
            Evidence::Marked => Kind::Marked,
        }
    }
}

/// A dataset's card.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Card {
    /// Its name: lowercase letters, digits and hyphens.
    pub name: String,
    /// What it is for, in a line.
    pub about: String,
    /// The terms a contributor agrees to, in full.
    pub terms: String,
    /// What counts as checked.
    pub takes: Vec<Kind>,
    /// Whether it takes sessions that read content from outside (a web
    /// page, a file someone sent): content that may be someone else's.
    #[serde(default)]
    pub outside: bool,
}

impl Card {
    /// The digest of its terms, which a contribution agrees to.
    pub fn terms_digest(&self) -> String {
        sion_core::hash::hash_domain("sion-cli/dataset/terms/v1", self.terms.as_bytes()).to_hex()
    }
}

/// A contribution, as its claim holds it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Contribution {
    /// The dataset's card.
    pub dataset: String,
    /// The digest of the example.
    pub example: String,
    /// The digest of the terms agreed to.
    pub terms: String,
    /// What checked the work.
    pub evidence: Evidence,
    /// The models that answered in the session, as its receipts name
    /// them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<String>,
    /// Whether the session read content from outside.
    #[serde(default)]
    pub read_outside: bool,
    /// How many pieces of text its contributor took out.
    #[serde(default)]
    pub redactions: u64,
}

/// An example as its contributor hands it to a curator.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExampleFile {
    /// The format's version.
    pub version: u32,
    /// Who contributes it.
    pub contributor: AccountId,
    /// The contribution's claim.
    pub claim: String,
    /// What the claim holds.
    pub contribution: Contribution,
    /// The output a task was settled with, where a task is the evidence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    /// The example: a line in chat format with tool calls.
    pub line: String,
}

/// A version of a dataset, as its claim holds it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Version {
    /// The dataset's card.
    pub dataset: String,
    /// Its number, from 1.
    pub number: u64,
    /// How many examples it holds.
    pub count: u64,
    /// The root of their digests, in the order they are listed.
    pub root: String,
    /// How many each contributor gave, by account.
    pub credit: BTreeMap<String, u64>,
    /// How many were checked by each kind of evidence.
    pub by_kind: BTreeMap<String, u64>,
}

/// An example as a version lists it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Listed {
    /// Its digest.
    pub example: String,
    /// Who contributed it.
    pub contributor: AccountId,
    /// The contribution's claim.
    pub contribution: String,
}

/// What a trainer holds beside a version's examples.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// The version's claim.
    pub claim: String,
    /// What it holds.
    pub version: Version,
    /// The examples, in the order of the file's lines.
    pub examples: Vec<Listed>,
}

/// The digest of an example: of its line, byte for byte.
pub fn example_digest(line: &str) -> String {
    sion_core::hash::hash_domain("sion-cli/dataset/example/v1", line.as_bytes()).to_hex()
}

/// The root of examples' digests, in order.
pub fn root_of(digests: &[String]) -> String {
    let leaves: Vec<Hash32> = digests
        .iter()
        .map(|digest| merkle::leaf_hash(digest.as_bytes()))
        .collect();
    merkle::root(&leaves).to_hex()
}

fn short(hex: &str, chars: usize) -> &str {
    &hex[..hex.len().min(chars)]
}

fn card_key(name: &str) -> String {
    format!("dataset:{name}")
}

fn in_key(dataset: &str, example: &str) -> String {
    format!("dataset-in:{}:{}", short(dataset, 16), short(example, 32))
}

fn out_key(dataset: &str, example: &str) -> String {
    format!("dataset-out:{}:{}", short(dataset, 16), short(example, 32))
}

fn version_key(dataset: &str) -> String {
    format!("dataset-version:{}", short(dataset, 32))
}

fn claim_of(hex: &str) -> Result<ClaimId, CliError> {
    Hash32::from_hex(hex.trim())
        .map(ClaimId::from_hash)
        .map_err(|e| CliError::Usage(format!("{hex:?} is not a claim's id: {e}")))
}

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

/// Opens a dataset: `card` as `wallet`'s claim.
pub fn open(session: &mut Session<'_>, wallet: &str, card: &Card) -> Result<ClaimId, CliError> {
    let named = !card.name.is_empty()
        && card.name.len() <= 40
        && card
            .name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if !named {
        return Err(CliError::Usage(format!(
            "{:?} is not a dataset's name: at most 40 lowercase letters, digits and hyphens",
            card.name
        )));
    }
    if card.terms.trim().is_empty() || card.terms.len() > MAX_TERMS {
        return Err(CliError::Usage(format!(
            "a dataset states the terms a contributor agrees to, in at most {MAX_TERMS} bytes"
        )));
    }
    if card.takes.is_empty() {
        return Err(CliError::Usage(
            "a dataset says what counts as checked".into(),
        ));
    }
    let body = signed(
        CARD_SCHEMA,
        format!("a dataset of checked work: {}", card.name),
        written(card)?,
    );
    crate::vouching::publish(session, wallet, body, Some(card_key(&card.name)))
}

/// The card the claim `id` holds, and its curator.
pub fn card(ledger: &dyn Ledger, id: &str) -> Result<(ClaimId, Card, AccountId), CliError> {
    let claim = claim_of(id)?;
    let read = feeds::read::<Card>(ledger, claim, CARD_SCHEMA)?
        .ok_or_else(|| CliError::Usage(format!("claim {} is not a dataset", claim.short())))?;
    Ok((claim, read.says, read.issuer))
}

/// The words a session's conversation holds: what a person and the tools
/// said, and what the agent said and called with.
fn words(saved: &Saved) -> String {
    let mut all = String::new();
    for message in &saved.state.messages {
        for block in &message.content {
            match block {
                sion_agent::model::Block::Text { text } => all.push_str(text),
                sion_agent::model::Block::ToolResult { content, .. } => all.push_str(content),
                sion_agent::model::Block::ToolUse { input, .. } => {
                    all.push_str(&input.to_string());
                    // As a tool was given it, too: a file's text written
                    // by a call.
                    if let Some(object) = input.as_object() {
                        for value in object.values().filter_map(|v| v.as_str()) {
                            all.push('\n');
                            all.push_str(value);
                        }
                    }
                }
                _ => {}
            }
            all.push('\n');
        }
    }
    all
}

/// What a contributor asks of an example before it leaves its machine.
#[derive(Clone, Debug, Default)]
pub struct Asked {
    /// Text to take out wherever it stands.
    pub redact: Vec<String>,
    /// Whether the system prompt is left out.
    pub withhold_system: bool,
    /// The task whose settlement checked the work, if one did.
    pub task: Option<ObjectId>,
    /// The output the task was settled with (the session's answer, if not
    /// given).
    pub output: Option<String>,
}

/// An example made of a session, with what its contribution says of it.
#[derive(Clone, Debug)]
pub struct Made {
    /// The line.
    pub line: String,
    /// What the contribution says.
    pub contribution: Contribution,
    /// The output a task was settled with.
    pub output: Option<String>,
}

/// Whether the account `paid` for a task is `contributor`'s: its own, or
/// that of `agent`, an agent it registered.
fn paid_to(
    ledger: &dyn Ledger,
    contributor: &AccountId,
    paid: &AccountId,
    agent: Option<ObjectId>,
) -> bool {
    match agent {
        None => paid == contributor,
        Some(agent) => crate::agents::identity(ledger, agent)
            .is_ok_and(|charter| charter.agent == *paid && charter.principal == *contributor),
    }
}

/// Makes an example of `saved` for the dataset `card` (whose claim is
/// `dataset`), as `contributor` asks. An error says why the session is
/// not one to contribute.
pub fn make(
    ledger: &dyn Ledger,
    dataset: &ClaimId,
    card: &Card,
    saved: &Saved,
    contributor: &AccountId,
    asked: &Asked,
) -> Result<Made, CliError> {
    let no = |why: &str| CliError::Usage(format!("session {}: {why}", saved.state.id));
    let answer = match &saved.ended {
        Some(sion_agent::Ended::Done { text }) => text.clone(),
        _ => return Err(no("it did not finish")),
    };
    if saved.state.compactions > 0 {
        return Err(no(
            "its conversation was compacted: it is not whole to learn from",
        ));
    }
    if saved.state.standing.confidential {
        return Err(no("it held confidential content"));
    }
    let read_outside = saved.state.standing.tainted_until.is_some();
    if read_outside && !card.outside {
        return Err(no(
            "it read content from outside, which may be someone else's, and this dataset does \
             not take such sessions",
        ));
    }
    // What checked the work, the strongest there is.
    let (evidence, output) =
        match asked.task {
            Some(task) => {
                let state = crate::tasks::load_task(ledger, task)?.state;
                let TaskState::Settled {
                    worker: paid,
                    result,
                    ..
                } = state
                else {
                    return Err(no("that task is not settled: no result of it was accepted"));
                };
                let agent = (paid != *contributor).then_some(saved.agent);
                if !paid_to(ledger, contributor, &paid, agent) {
                    return Err(no(
                        "that task paid an account that is neither yours nor that of an agent \
                         you registered",
                    ));
                }
                let output = asked.output.clone().unwrap_or(answer);
                if !settled_with(ledger, task, result, &output)? {
                    return Err(no(
                        "the task was settled with another output than this (give it with \
                         --output)",
                    ));
                }
                let output_in_session = words(saved).contains(output.trim());
                (
                    Evidence::Task {
                        task,
                        result,
                        output_in_session,
                        agent,
                    },
                    Some(output),
                )
            }
            None if saved.verify.is_some() => (
                Evidence::Check {
                    command: sion_core::hash::hash_domain(
                        "sion-cli/dataset/check/v1",
                        saved.verify.as_deref().unwrap_or_default().as_bytes(),
                    )
                    .to_hex(),
                },
                None,
            ),
            None if crate::sealed::marks()?
                .get(&saved.state.id)
                .is_some_and(|mark| mark.good) =>
            {
                (Evidence::Marked, None)
            }
            None => return Err(no(
                "nothing checked it: it has no check that passed, no settled task (--task) and \
                 no mark that it went well",
            )),
        };
    if !card.takes.contains(&evidence.kind()) {
        return Err(no(&format!(
            "this dataset does not take work checked only by {}",
            match evidence.kind() {
                Kind::Task => "a settled task",
                Kind::Check => "the session's own check",
                Kind::Marked => "its operator's mark",
            }
        )));
    }
    let system = if asked.withhold_system {
        "[withheld]"
    } else if saved.prompt.is_empty() {
        &saved.system
    } else {
        &saved.prompt
    };
    let mut line = json!({
        "messages": sion_agent::providers::openai_messages(
            system,
            &saved.state.messages,
            &saved.state.images,
            "",
        ),
        "tools": sion_agent::providers::openai_tools(&crate::training::tools_of(saved)),
        "metadata": {
            "checked_by": evidence.kind().word(),
            "turns": saved.state.turns_taken,
            "read_outside": read_outside,
        },
    })
    .to_string();
    // Taken out as the line holds it: text is escaped there as JSON
    // escapes it.
    let mut redactions = 0;
    for text in asked.redact.iter().filter(|text| !text.is_empty()) {
        let escaped = serde_json::to_string(text).unwrap_or_default();
        let inner = &escaped[1..escaped.len().saturating_sub(1)];
        redactions += line.matches(inner).count() as u64;
        line = line.replace(inner, REDACTED);
    }
    if crate::training::holds_secret(&line, &crate::training::kept_secrets()) {
        return Err(no(
            "it holds a secret this machine keeps (a credential or a login's password)",
        ));
    }
    let mut models: Vec<String> = saved
        .state
        .receipts
        .tallies
        .iter()
        .map(|tally| tally.answered.clone())
        .collect();
    models.sort();
    models.dedup();
    let contribution = Contribution {
        dataset: dataset.as_hash().to_hex(),
        example: example_digest(&line),
        terms: card.terms_digest(),
        evidence,
        models,
        read_outside,
        redactions,
    };
    Ok(Made {
        line,
        contribution,
        output,
    })
}

/// Whether `task` was settled with the result `result`, and that result
/// commits to `output`.
fn settled_with(
    ledger: &dyn Ledger,
    task: ObjectId,
    result: ClaimId,
    output: &str,
) -> Result<bool, CliError> {
    let claim = crate::tasks::stored_claim(ledger, &result.as_hash().to_hex())?;
    let Payload::Public(bytes) = &claim.body.payload else {
        return Ok(false);
    };
    let Ok(said) = sion_core::encode::decode::<TaskResult>(bytes) else {
        return Ok(false);
    };
    Ok(said.task == task && said.output == crate::tasks::output_commitment(output.as_bytes()))
}

/// Publishes `contribution` as `wallet`'s claim, named in its index by
/// the dataset and the example.
pub fn contribute(
    session: &mut Session<'_>,
    wallet: &str,
    contribution: &Contribution,
) -> Result<ClaimId, CliError> {
    let body = signed(
        CONTRIBUTION_SCHEMA,
        format!(
            "contributes example {} to dataset {}",
            short(&contribution.example, 8),
            short(&contribution.dataset, 8)
        ),
        written(contribution)?,
    );
    crate::vouching::publish(
        session,
        wallet,
        body,
        Some(in_key(&contribution.dataset, &contribution.example)),
    )
}

/// Withdraws `wallet`'s contribution of `example` to `dataset`: later
/// versions leave it out.
pub fn withdraw(
    session: &mut Session<'_>,
    wallet: &str,
    dataset: &str,
    example: &str,
) -> Result<ClaimId, CliError> {
    let me = session.id(wallet);
    if feeds::indexed(session.ledger, &me, &in_key(dataset, example))?.is_none() {
        return Err(CliError::Usage(format!(
            "{wallet} contributed no example {} to that dataset",
            short(example, 8)
        )));
    }
    let body = signed(
        WITHDRAWN_SCHEMA,
        format!(
            "withdraws example {} from dataset {}",
            short(example, 8),
            short(dataset, 8)
        ),
        written(&json!({"dataset": dataset, "example": example}))?,
    );
    crate::vouching::publish(session, wallet, body, Some(out_key(dataset, example)))
}

/// Whether `contributor` has withdrawn `example` from `dataset`.
pub fn withdrawn(
    ledger: &dyn Ledger,
    contributor: &AccountId,
    dataset: &str,
    example: &str,
) -> Result<bool, CliError> {
    Ok(feeds::indexed(ledger, contributor, &out_key(dataset, example))?.is_some())
}

/// The Pulse the contribution of the example `file` was made in, where
/// the file is what its contributor committed to on this ledger; why it
/// is not, otherwise.
pub fn made_at(ledger: &dyn Ledger, file: &ExampleFile) -> Result<Result<u64, String>, CliError> {
    let no = |why: &str| Ok(Err(why.to_string()));
    let said = &file.contribution;
    if example_digest(&file.line) != said.example {
        return no("the example is not the one its contribution commits to");
    }
    let Ok(claim) = claim_of(&file.claim) else {
        return no("it names no contribution");
    };
    match feeds::read::<Contribution>(ledger, claim, CONTRIBUTION_SCHEMA)?
        .filter(|read| read.issuer == file.contributor && read.says == *said)
    {
        Some(held) => Ok(Ok(held.issued_at)),
        None => no("its contributor made no such contribution on this ledger"),
    }
}

/// Why the curator does not take the example `file`, a contribution its
/// contributor made ([`made_at`]), into the dataset `card` (whose claim
/// is `dataset`), if it does not: `None` where it was contributed to this
/// dataset on its terms, was checked as it says as far as the ledger can
/// tell, and is not withdrawn.
pub fn refused(
    ledger: &dyn Ledger,
    dataset: &ClaimId,
    card: &Card,
    file: &ExampleFile,
) -> Result<Option<String>, CliError> {
    let no = |why: &str| Ok(Some(why.to_string()));
    let hex = dataset.as_hash().to_hex();
    let said = &file.contribution;
    if said.dataset != hex {
        return no("it was contributed to another dataset");
    }
    if said.terms != card.terms_digest() {
        return no("its contributor agreed to other terms than this dataset's");
    }
    if !card.takes.contains(&said.evidence.kind()) {
        return no(&format!(
            "this dataset does not take work checked by {}",
            said.evidence.kind().word()
        ));
    }
    if said.read_outside && !card.outside {
        return no("its session read content from outside");
    }
    if let Evidence::Task {
        task,
        result,
        agent,
        ..
    } = &said.evidence
    {
        let paid = match crate::tasks::load_task(ledger, *task)?.state {
            TaskState::Settled {
                worker,
                result: with,
                ..
            } if with == *result => worker,
            _ => return no("the task it names was not settled with the result it names"),
        };
        if !paid_to(ledger, &file.contributor, &paid, *agent) {
            return no(
                "the task it names paid an account that is neither its contributor's nor that of \
                 an agent its contributor registered",
            );
        }
        let output = file.output.as_deref().unwrap_or_default();
        if !settled_with(ledger, *task, *result, output)? {
            return no("the task it names was not settled with the output it gives");
        }
    }
    if withdrawn(ledger, &file.contributor, &hex, &said.example)? {
        return no("its contributor withdrew it");
    }
    Ok(None)
}

/// The latest version of the dataset `dataset` its curator published.
pub fn latest(
    ledger: &dyn Ledger,
    curator: &AccountId,
    dataset: &ClaimId,
) -> Result<Option<(ClaimId, Version)>, CliError> {
    let hex = dataset.as_hash().to_hex();
    let Some(claim) = feeds::indexed(ledger, curator, &version_key(&hex))? else {
        return Ok(None);
    };
    Ok(feeds::read::<Version>(ledger, claim, VERSION_SCHEMA)?
        .filter(|read| read.issuer == *curator && read.says.dataset == hex)
        .map(|read| (claim, read.says)))
}

/// A version as it is built: what its claim will hold, the examples
/// taken with their lines, in the order of their digests, and why each
/// other one was left out.
#[derive(Clone, Debug)]
pub struct Built {
    /// What the version's claim holds.
    pub version: Version,
    /// The examples taken, each with its line.
    pub examples: Vec<(Listed, String)>,
    /// Why each other example was left out.
    pub left: Vec<String>,
}

/// Builds version `number` from `files`.
pub fn build(
    ledger: &dyn Ledger,
    dataset: &ClaimId,
    card: &Card,
    number: u64,
    files: &[ExampleFile],
) -> Result<Built, CliError> {
    // An example contributed twice is its first contributor's: the one
    // whose contribution the ledger issued first. Whoever comes after has
    // no claim to it, whatever becomes of the first.
    let mut first: BTreeMap<String, (u64, &ExampleFile)> = BTreeMap::new();
    let mut left = Vec::new();
    for file in files {
        let example = &file.contribution.example;
        let at = match made_at(ledger, file)? {
            Ok(at) => at,
            Err(why) => {
                left.push(format!("example {}: {why}", short(example, 8)));
                continue;
            }
        };
        let later = match first.get(example) {
            Some((had, held)) if (*had, &held.claim) <= (at, &file.claim) => file,
            Some((_, held)) => {
                let held = *held;
                first.insert(example.clone(), (at, file));
                held
            }
            None => {
                first.insert(example.clone(), (at, file));
                continue;
            }
        };
        left.push(format!(
            "example {}: {} contributed it after someone else had",
            short(example, 8),
            later.contributor.short()
        ));
    }
    let mut kept = BTreeMap::new();
    for (example, (_, file)) in first {
        match refused(ledger, dataset, card, file)? {
            Some(why) => left.push(format!("example {}: {why}", short(&example, 8))),
            None => {
                kept.insert(example, file);
            }
        }
    }
    let first = kept;
    let taken: BTreeMap<String, (Listed, String, Kind)> = first
        .into_iter()
        .map(|(example, file)| {
            (
                example.clone(),
                (
                    Listed {
                        example,
                        contributor: file.contributor,
                        contribution: file.claim.clone(),
                    },
                    file.line.clone(),
                    file.contribution.evidence.kind(),
                ),
            )
        })
        .collect();
    let mut credit: BTreeMap<String, u64> = BTreeMap::new();
    let mut by_kind: BTreeMap<String, u64> = BTreeMap::new();
    for (listed, _, kind) in taken.values() {
        *credit
            .entry(listed.contributor.as_hash().to_hex())
            .or_default() += 1;
        *by_kind.entry(kind.word().to_string()).or_default() += 1;
    }
    let digests: Vec<String> = taken.keys().cloned().collect();
    let version = Version {
        dataset: dataset.as_hash().to_hex(),
        number,
        count: digests.len() as u64,
        root: root_of(&digests),
        credit,
        by_kind,
    };
    let examples = taken
        .into_values()
        .map(|(listed, line, _)| (listed, line))
        .collect();
    Ok(Built {
        version,
        examples,
        left,
    })
}

/// Publishes `version` as `wallet`'s claim, the dataset's latest.
pub fn publish(
    session: &mut Session<'_>,
    wallet: &str,
    version: &Version,
) -> Result<ClaimId, CliError> {
    let body = signed(
        VERSION_SCHEMA,
        format!(
            "version {} of dataset {}: {} example(s)",
            version.number,
            short(&version.dataset, 8),
            version.count
        ),
        written(version)?,
    );
    crate::vouching::publish(session, wallet, body, Some(version_key(&version.dataset)))
}

/// Checks a file of examples (`lines`) against `manifest` and the ledger,
/// as someone about to train on it: a line for each thing looked at, and
/// whether everything holds.
pub fn check(
    ledger: &dyn Ledger,
    manifest: &Manifest,
    lines: &[&str],
) -> Result<(Vec<String>, bool), CliError> {
    let mut out = Vec::new();
    let mut holds = true;
    let version = &manifest.version;
    let (dataset, card, curator) = card(ledger, &version.dataset)?;
    // The examples are the ones listed, and the list is the version's.
    let digests: Vec<String> = lines.iter().map(|line| example_digest(line)).collect();
    let listed: Vec<String> = manifest
        .examples
        .iter()
        .map(|l| l.example.clone())
        .collect();
    if digests == listed
        && root_of(&digests) == version.root
        && version.count == digests.len() as u64
    {
        out.push(format!(
            "holds: {} example(s), each the one the version lists, in its order",
            digests.len()
        ));
    } else {
        holds = false;
        out.push(
            "differs: the examples are not the ones the version lists: one was changed, added, \
             left out or moved"
                .into(),
        );
    }
    let held = feeds::read::<Version>(ledger, claim_of(&manifest.claim)?, VERSION_SCHEMA)?;
    match held {
        Some(read) if read.issuer == curator && read.says == *version => out.push(format!(
            "holds: version {} of dataset {} is its curator's ({}), published at Pulse {}",
            version.number,
            card.name,
            curator.short(),
            read.issued_at
        )),
        _ => {
            holds = false;
            out.push("differs: the dataset's curator published no such version".into());
        }
    }
    // Each example: its contributor's, on these terms, and what checked it.
    let terms = card.terms_digest();
    let (mut agreed, mut settled, mut gone) = (0u64, 0u64, Vec::new());
    let mut by_kind: BTreeMap<Kind, u64> = BTreeMap::new();
    let mut credit: BTreeMap<String, u64> = BTreeMap::new();
    let hex = dataset.as_hash().to_hex();
    for listed in &manifest.examples {
        let read = match claim_of(&listed.contribution) {
            Ok(claim) => feeds::read::<Contribution>(ledger, claim, CONTRIBUTION_SCHEMA)?,
            Err(_) => None,
        };
        let Some(read) = read.filter(|read| {
            read.issuer == listed.contributor
                && read.says.example == listed.example
                && read.says.dataset == hex
                && read.says.terms == terms
        }) else {
            continue;
        };
        agreed += 1;
        *by_kind.entry(read.says.evidence.kind()).or_default() += 1;
        *credit
            .entry(listed.contributor.as_hash().to_hex())
            .or_default() += 1;
        if let Evidence::Task { task, result, .. } = &read.says.evidence {
            if matches!(
                crate::tasks::load_task(ledger, *task)?.state,
                TaskState::Settled { result: with, .. } if with == *result
            ) {
                settled += 1;
            }
        }
        if withdrawn(ledger, &listed.contributor, &hex, &listed.example)? {
            gone.push(short(&listed.example, 8).to_string());
        }
    }
    let total = manifest.examples.len() as u64;
    if agreed == total {
        out.push(format!(
            "holds: every one was contributed by the account credited with it, which agreed to \
             the dataset's terms ({}…)",
            short(&terms, 16)
        ));
    } else {
        holds = false;
        out.push(format!(
            "differs: {} of {total} example(s) have no contribution by the account credited, to \
             this dataset, on its terms",
            total - agreed
        ));
    }
    let tasks = by_kind.get(&Kind::Task).copied().unwrap_or(0);
    if tasks > 0 {
        if settled == tasks {
            out.push(format!(
                "holds: {tasks} checked by a task: each task is settled on the ledger with the \
                 result named"
            ));
        } else {
            holds = false;
            out.push(format!(
                "differs: {} of the {tasks} said to be checked by a task name a task that is \
                 not settled with that result",
                tasks - settled
            ));
        }
    }
    for (kind, word) in [
        (Kind::Check, "by the session's own check"),
        (Kind::Marked, "by its operator's mark"),
    ] {
        if let Some(count) = by_kind.get(&kind) {
            out.push(format!(
                "  {count} checked {word}: their contributors' word"
            ));
        }
    }
    if credit != version.credit {
        holds = false;
        out.push(
            "differs: the version's count of who gave what is not what the examples show".into(),
        );
    }
    for (account, count) in &version.credit {
        out.push(format!("  credit: {} gave {count}", short(account, 8)));
    }
    if !gone.is_empty() {
        out.push(format!(
            "note: {} example(s) were withdrawn after this version was built ({}): a later \
             version leaves them out",
            gone.len(),
            gone.join(", ")
        ));
    }
    Ok((out, holds))
}

/// `sion agent dataset`.
#[derive(Clone, Debug, Subcommand)]
pub enum DatasetCommand {
    /// Open a dataset, as its curator: what it is for, the terms a
    /// contributor agrees to, and what counts as checked.
    Open {
        /// The dataset's name (lowercase letters, digits and hyphens).
        name: String,
        /// What it is for, in a line.
        #[arg(long, default_value = "")]
        about: String,
        /// A file with the terms a contributor agrees to.
        #[arg(long, value_name = "FILE")]
        terms: PathBuf,
        /// What counts as checked (repeat for more): task (a settled Task
        /// Charter), check (the session's own check passed), marked (its
        /// operator marked it good).
        #[arg(long = "take", value_enum, required = true)]
        takes: Vec<Kind>,
        /// Take sessions that read content from outside too.
        #[arg(long)]
        outside: bool,
        /// Your wallet.
        #[arg(long)]
        wallet: String,
    },
    /// Contribute a session to a dataset, on its terms: an example is
    /// written to a file for the curator, and a claim of yours commits to
    /// it, says what checked the work and agrees to the terms. Nothing of
    /// the example goes on the ledger.
    Contribute {
        /// The dataset (its card's claim).
        dataset: String,
        /// The session.
        session: String,
        /// Say that you agree to the dataset's terms: without it the terms
        /// are shown and nothing is contributed.
        #[arg(long)]
        agree: bool,
        /// The Task Charter whose settlement checked this work.
        #[arg(long)]
        task: Option<String>,
        /// The file the task was settled with (the session's answer, if
        /// not given).
        #[arg(long, value_name = "FILE")]
        output: Option<PathBuf>,
        /// Text to take out of the example wherever it stands (repeat for
        /// more).
        #[arg(long, value_name = "TEXT")]
        redact: Vec<String>,
        /// Leave your system prompt out of the example.
        #[arg(long = "withhold-system")]
        withhold_system: bool,
        /// The directory to write the example in.
        #[arg(long, value_name = "DIR", default_value = ".")]
        out: PathBuf,
        /// Your wallet.
        #[arg(long)]
        wallet: String,
    },
    /// Withdraw an example you contributed: versions built from now on
    /// leave it out. A version already built cannot be called back.
    Withdraw {
        /// The example's file.
        example: PathBuf,
        /// Your wallet.
        #[arg(long)]
        wallet: String,
    },
    /// Build a version, as the dataset's curator, from the example files
    /// in a directory: each is checked against its contribution and what
    /// it says checked it, and those taken are written as a file to train
    /// on, with a manifest, and committed to in a claim.
    Build {
        /// The dataset (its card's claim).
        dataset: String,
        /// The directory of example files.
        #[arg(long, value_name = "DIR")]
        from: PathBuf,
        /// The directory to write the version in.
        #[arg(long, value_name = "DIR", default_value = ".")]
        out: PathBuf,
        /// Your wallet.
        #[arg(long)]
        wallet: String,
    },
    /// Check a file of examples against its manifest and the ledger, as
    /// someone about to train on it: that the examples are the version's,
    /// that each was contributed on the dataset's terms, and what checked
    /// each.
    Check {
        /// The manifest.
        manifest: PathBuf,
        /// The examples.
        examples: PathBuf,
    },
    /// A dataset's card and its latest version.
    Show {
        /// The dataset (its card's claim).
        dataset: String,
    },
}

fn read<T: serde::de::DeserializeOwned>(path: &Path, what: &str) -> Result<T, CliError> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| CliError::Usage(format!("{}: {e}", path.display())))?;
    serde_json::from_str(&text)
        .map_err(|e| CliError::Usage(format!("{} is not {what}: {e}", path.display())))
}

/// Runs a dataset command.
pub fn command(
    session: &mut Session<'_>,
    ledger: &dyn Ledger,
    command: DatasetCommand,
) -> Result<(), CliError> {
    match command {
        DatasetCommand::Open {
            name,
            about,
            terms,
            takes,
            outside,
            wallet,
        } => {
            let terms = std::fs::read_to_string(&terms)
                .map_err(|e| CliError::Usage(format!("{}: {e}", terms.display())))?;
            let mut takes = takes;
            takes.sort();
            takes.dedup();
            let card = Card {
                name,
                about: about.split_whitespace().collect::<Vec<_>>().join(" "),
                terms,
                takes,
                outside,
            };
            let claim = open(session, &wallet, &card)?;
            println!(
                "opened dataset {}: claim {}",
                card.name,
                claim.as_hash().to_hex()
            );
            println!(
                "  it takes work checked by: {}; its terms' digest is {}…",
                card.takes
                    .iter()
                    .map(|k| k.word())
                    .collect::<Vec<_>>()
                    .join(", "),
                short(&card.terms_digest(), 16)
            );
            Ok(())
        }
        DatasetCommand::Contribute {
            dataset,
            session: id,
            agree,
            task,
            output,
            redact,
            withhold_system,
            out,
            wallet,
        } => {
            let (claim, card, curator) = card(ledger, &dataset)?;
            if !agree {
                println!(
                    "dataset {} by {} asks its contributors to agree to these terms:",
                    card.name,
                    curator.short()
                );
                println!();
                println!("{}", card.terms.trim());
                println!();
                println!("nothing was contributed: say --agree to contribute on these terms");
                return Ok(());
            }
            let saved = Saved::load(&id)?;
            let asked = Asked {
                redact,
                withhold_system,
                task: task
                    .as_deref()
                    .map(|hex| {
                        Hash32::from_hex(hex.trim())
                            .map(ObjectId::from_hash)
                            .map_err(|e| CliError::Usage(format!("the task: {e}")))
                    })
                    .transpose()?,
                output: output
                    .as_deref()
                    .map(|path| {
                        std::fs::read_to_string(path)
                            .map_err(|e| CliError::Usage(format!("{}: {e}", path.display())))
                    })
                    .transpose()?,
            };
            let me = session.id(&wallet);
            let made = make(ledger, &claim, &card, &saved, &me, &asked)?;
            let published = contribute(session, &wallet, &made.contribution)?;
            let file = ExampleFile {
                version: 1,
                contributor: me,
                claim: published.as_hash().to_hex(),
                contribution: made.contribution.clone(),
                output: made.output,
                line: made.line,
            };
            let path = out.join(format!(
                "{}.example.json",
                short(&file.contribution.example, 16)
            ));
            crate::profiles::write_json(&path, &file)?;
            println!(
                "contributed example {} to dataset {}, on its terms: claim {}",
                short(&file.contribution.example, 8),
                card.name,
                published.short()
            );
            println!(
                "  checked by: {}",
                match &file.contribution.evidence {
                    Evidence::Task {
                        task,
                        output_in_session,
                        ..
                    } => format!(
                        "task {}, settled on the ledger{}",
                        task.0.short(),
                        if *output_in_session {
                            "; its output is in the session"
                        } else {
                            "; its output is not in the session word for word"
                        }
                    ),
                    Evidence::Check { .. } => "the session's own check, which passed".into(),
                    Evidence::Marked => "your mark that it went well".into(),
                }
            );
            if file.contribution.redactions > 0 {
                println!(
                    "  {} piece(s) of text taken out",
                    file.contribution.redactions
                );
            }
            println!(
                "  the example is in {}: give it to the curator; nothing of it is on the ledger",
                path.display()
            );
            Ok(())
        }
        DatasetCommand::Withdraw { example, wallet } => {
            let file: ExampleFile = read(&example, "an example")?;
            let said = &file.contribution;
            let claim = withdraw(session, &wallet, &said.dataset, &said.example)?;
            println!(
                "withdrew example {}: claim {}",
                short(&said.example, 8),
                claim.short()
            );
            println!(
                "  versions built from now on leave it out; one already built cannot be called \
                 back"
            );
            Ok(())
        }
        DatasetCommand::Build {
            dataset,
            from,
            out,
            wallet,
        } => {
            let (claim, card, curator) = card(ledger, &dataset)?;
            if curator != session.id(&wallet) {
                return Err(CliError::Usage(format!(
                    "{wallet} is not the curator of dataset {}",
                    card.name
                )));
            }
            let mut files = Vec::new();
            let mut paths: Vec<PathBuf> = std::fs::read_dir(&from)
                .map_err(|e| CliError::Usage(format!("{}: {e}", from.display())))?
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.to_string_lossy().ends_with(".example.json"))
                .collect();
            paths.sort();
            let mut left = Vec::new();
            for path in &paths {
                match read::<ExampleFile>(path, "an example") {
                    Ok(file) => files.push(file),
                    Err(why) => left.push(why.to_string()),
                }
            }
            let number = latest(ledger, &curator, &claim)?.map_or(1, |(_, v)| v.number + 1);
            let Built {
                version,
                examples,
                left: refused,
            } = build(ledger, &claim, &card, number, &files)?;
            left.extend(refused);
            if examples.is_empty() {
                for line in &left {
                    println!("left out: {line}");
                }
                return Err(CliError::Usage(
                    "no example was taken: there is no version to build".into(),
                ));
            }
            let published = publish(session, &wallet, &version)?;
            let stem = format!("{}-v{}", card.name, version.number);
            let lines: String = examples
                .iter()
                .map(|(_, line)| format!("{line}\n"))
                .collect();
            let data = out.join(format!("{stem}.jsonl"));
            std::fs::create_dir_all(&out)
                .and_then(|()| std::fs::write(&data, lines))
                .map_err(|e| CliError::Usage(format!("{}: {e}", data.display())))?;
            let manifest = Manifest {
                claim: published.as_hash().to_hex(),
                version: version.clone(),
                examples: examples.into_iter().map(|(listed, _)| listed).collect(),
            };
            let listing = out.join(format!("{stem}.manifest.json"));
            crate::profiles::write_json(&listing, &manifest)?;
            println!(
                "built version {} of dataset {}: {} example(s), root {}…, claim {}",
                version.number,
                card.name,
                version.count,
                short(&version.root, 16),
                published.short()
            );
            for (account, count) in &version.credit {
                println!("  credit: {} gave {count}", short(account, 8));
            }
            for line in &left {
                println!("  left out: {line}");
            }
            println!("  {} and {}", data.display(), listing.display());
            Ok(())
        }
        DatasetCommand::Check { manifest, examples } => {
            let manifest: Manifest = read(&manifest, "a manifest")?;
            let text = std::fs::read_to_string(&examples)
                .map_err(|e| CliError::Usage(format!("{}: {e}", examples.display())))?;
            let lines: Vec<&str> = text.lines().collect();
            let (out, holds) = check(ledger, &manifest, &lines)?;
            for line in out {
                println!("{line}");
            }
            if holds {
                Ok(())
            } else {
                Err(CliError::Verification(
                    "the dataset does not check out".into(),
                ))
            }
        }
        DatasetCommand::Show { dataset } => {
            let (claim, card, curator) = card(ledger, &dataset)?;
            println!(
                "dataset {} by {}: {}",
                card.name,
                curator.short(),
                card.about
            );
            println!(
                "  takes work checked by: {}{}",
                card.takes
                    .iter()
                    .map(|k| k.word())
                    .collect::<Vec<_>>()
                    .join(", "),
                if card.outside {
                    "; sessions that read content from outside too"
                } else {
                    "; no session that read content from outside"
                }
            );
            println!("  terms ({}…):", short(&card.terms_digest(), 16));
            for line in card.terms.trim().lines() {
                println!("    {line}");
            }
            match latest(ledger, &curator, &claim)? {
                Some((at, version)) => {
                    println!(
                        "  version {}: {} example(s), root {}…, claim {}",
                        version.number,
                        version.count,
                        short(&version.root, 16),
                        at.short()
                    );
                    for (kind, count) in &version.by_kind {
                        println!("    {count} checked by {kind}");
                    }
                    for (account, count) in &version.credit {
                        println!("    credit: {} gave {count}", short(account, 8));
                    }
                }
                None => println!("  no version is built yet"),
            }
            Ok(())
        }
    }
}
