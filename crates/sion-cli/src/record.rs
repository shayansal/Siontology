//! An agent's record, for whoever is asked to stand behind it: what it may
//! do, what it has done, and what went wrong, in a form a third party can
//! check.
//!
//! Someone asked to insure an agent, or to rely on it for something that
//! matters, needs three things an operator's assurances do not give: what
//! the agent is allowed to do, what it did, and how often that went badly,
//! from a source the operator cannot edit. The ledger holds much of that
//! already. [`write`] gathers it into one document in two parts that are
//! never mixed:
//!
//! - **What the ledger shows** ([`OnLedger`]): the agent's charter as it
//!   stands, and, over a span of Pulses, its payments, the times it was
//!   suspended or narrowed, the tasks it delivered, gave back or had proven
//!   false, the commits of its memory, the changes it kept on sealed tasks,
//!   and every challenge and slash against the bonds of its account and its
//!   owner. Anyone recomputes this part from the ledger's events
//!   ([`check`]).
//! - **What its operator says** ([`Reported`]): what only this machine's
//!   sessions know, such as how runs ended, how often a person was asked
//!   and what they decided, how the agent's stated expectations held, what
//!   was held back as an attack, and payments the ledger refused. This part
//!   is the operator's word. It is bound to the sessions it was counted
//!   from by a digest, so that sessions shown later can be held to it.
//!
//! [`warrant`] is the other half: a claim on the operator's bond
//! `agent-warranty`, under an adjudicator, saying what the operator stands
//! behind. A counterparty relies on it with `sion accept`, which reserves
//! part of the bond for it, and is compensated from the bond if the
//! adjudicator upholds a challenge. The record is what the price of that
//! reliance can be set by.
//!
//! Nothing here is insurance, and nothing here prices risk. It is the
//! evidence and the enforceable limits that pricing needs.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use clap::Subcommand;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sion_assurance::AtomClass;
use sion_core::hash::Hash32;
use sion_core::ids::{AccountId, AtomId, ClaimId, ObjectId, Pulse};
use sion_core::Amount;
use sion_exec::claim::{formulas, ClaimBody, Payload};
use sion_exec::execute::{bond_atom, Event};
use sion_exec::TxKind;

use crate::error::CliError;
use crate::feeds;
use crate::ledger::{Ledger, LedgerExt};
use crate::session::Session;

/// The schema of a claim committing to a record.
pub const RECORD_SCHEMA: &str = "agent.record/v1";

/// The schema of a warranty.
pub const WARRANTY_SCHEMA: &str = "agent.warranty/v1";

/// The bond an operator stands behind a warranty with.
pub const WARRANTY_BOND: &str = "agent-warranty";

/// As many events as a node answers at once.
const PAGE: usize = 5_000;

/// What the ledger shows of an agent over a span of Pulses: every figure
/// here is added up from the ledger's events, and anyone adds it up again.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OnLedger {
    /// The first Pulse counted.
    pub from: u64,
    /// The last Pulse counted.
    pub to: u64,
    /// The Pulse the agent was registered in, if within the span.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registered_at: Option<u64>,
    /// Payments it made under its charter.
    pub payments: u64,
    /// What they came to.
    pub paid: Amount,
    /// The largest of them.
    pub largest: Amount,
    /// How many accounts it paid.
    pub payees: u64,
    /// Subagents it launched.
    pub subagents: u64,
    /// Times it was suspended.
    pub suspended: u64,
    /// Times a suspension was lifted.
    pub resumed: u64,
    /// Times its charter was narrowed, or a narrowing lifted.
    pub narrowed: u64,
    /// Tasks it was paid for.
    pub delivered: u64,
    /// What those came to.
    pub earned: Amount,
    /// Delivered results later proven false.
    pub falsified: u64,
    /// Tasks it was named for and gave back.
    pub released: u64,
    /// Changes it kept on sealed tasks and published.
    pub improvements: u64,
    /// Seals of tasks it published.
    pub seals: u64,
    /// Bonds posted by its account or its owner.
    pub bonds: u64,
    /// Challenges opened against those bonds.
    pub challenges: u64,
    /// Of those, the ones an adjudicator upheld.
    pub upheld: u64,
    /// Times those bonds were slashed.
    pub slashes: u64,
    /// What was slashed from them.
    pub slashed: Amount,
}

/// What an operator says of an agent from its own machine's sessions.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reported {
    /// Sessions of the agent kept on this machine.
    pub sessions: u64,
    /// How their last runs ended, by kind.
    pub ended: BTreeMap<String, u64>,
    /// Sessions that finished and whose check passed.
    pub checked: u64,
    /// Tool calls made.
    pub calls: u64,
    /// Of those, the ones that failed.
    pub calls_failed: u64,
    /// Payments the ledger refused.
    pub payments_refused: u64,
    /// Times a person was asked to decide a call.
    pub asked: u64,
    /// Of those, approved.
    pub approved: u64,
    /// Of those, declined.
    pub declined: u64,
    /// Decisions signed with a wallet's keys whose signature holds.
    pub signed: u64,
    /// Calls the agent stated an expectation for.
    pub expected: u64,
    /// Of those, the ones that went as it said.
    pub expected_held: u64,
    /// Results from outside held back for matching a reported attack.
    pub attacks_held: u64,
    /// A digest of the sessions counted, each by its id and its state: a
    /// session shown later is one of these or it is not.
    pub sessions_digest: String,
}

/// An agent's record.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Record {
    /// The format's version.
    pub version: u32,
    /// The chain it is of, by its genesis digest.
    pub chain: String,
    /// The agent.
    pub agent: ObjectId,
    /// The account that signs for it.
    pub account: AccountId,
    /// The account that registered it and owns its charter, if the ledger
    /// says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<AccountId>,
    /// Its charter as the ledger had it when the record was written: what
    /// it may pay, whom, how fast, until when.
    pub charter: Value,
    /// What the ledger shows.
    pub ledger: OnLedger,
    /// What its operator says, if the record was written on a machine that
    /// holds its sessions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reported: Option<Reported>,
    /// What its operator says, sealed: the same figures committed to and
    /// not shown ([`Sealed`]), in place of [`Record::reported`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sealed: Option<Sealed>,
    /// Rules the operator proved its sealed figures keep, without showing
    /// them. Added after the record is written and committed to: they are
    /// not part of its digest.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub proofs: Vec<Proven>,
}

impl Record {
    /// The record's digest: what a claim commits to. The proofs added to a
    /// sealed record afterwards are not part of it.
    pub fn digest(&self) -> Result<Hash32, CliError> {
        let bare = Record {
            proofs: Vec::new(),
            ..self.clone()
        };
        let bytes = serde_json::to_vec(&bare)
            .map_err(|e| CliError::Usage(format!("the record could not be written: {e}")))?;
        Ok(sion_core::hash::hash_domain(
            "sion-cli/agent-record/v1",
            &bytes,
        ))
    }
}

/// What an operator says of its sessions, committed to and not shown: a
/// commitment to each figure of [`Reported`], by the figure's name
/// ([`sion_zk`]). The operator keeps what opens them, and proves rules
/// about them to whoever asks ([`prove_rules`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sealed {
    /// A commitment to each figure.
    pub figures: BTreeMap<String, sion_zk::Commitment>,
    /// The digest of the sessions counted, as in [`Reported`].
    pub sessions_digest: String,
}

/// A rule the operator proved its sealed figures keep.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Proven {
    /// The rule, as written: figures' names, whole numbers, `*` and `+`,
    /// compared with `<=`, `>=` or `==`.
    pub rule: String,
    /// The proof.
    pub proof: sion_zk::Proof,
}

/// The figures of `said`, by name: what a sealed record commits to and a
/// rule names.
pub fn figures(said: &Reported) -> BTreeMap<String, u64> {
    let mut figures: BTreeMap<String, u64> = [
        ("sessions", said.sessions),
        ("checked", said.checked),
        ("calls", said.calls),
        ("calls_failed", said.calls_failed),
        ("payments_refused", said.payments_refused),
        ("asked", said.asked),
        ("approved", said.approved),
        ("declined", said.declined),
        ("signed", said.signed),
        ("expected", said.expected),
        ("expected_held", said.expected_held),
        ("attacks_held", said.attacks_held),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_string(), value))
    .collect();
    figures.insert(
        "done".to_string(),
        said.ended.get("done").copied().unwrap_or(0),
    );
    figures
}

/// Seals `said`: the commitments to show, and what opens them, to keep.
pub fn seal(said: &Reported) -> (Sealed, BTreeMap<String, sion_zk::Opening>) {
    let mut commitments = BTreeMap::new();
    let mut openings = BTreeMap::new();
    for (name, value) in figures(said) {
        let (commitment, opening) = sion_zk::commit(value);
        commitments.insert(name.clone(), commitment);
        openings.insert(name, opening);
    }
    (
        Sealed {
            figures: commitments,
            sessions_digest: said.sessions_digest.clone(),
        },
        openings,
    )
}

/// One side of a rule: the figures it names with what each is multiplied
/// by, and the whole number added.
fn side(text: &str, names: &[&String]) -> Result<(BTreeMap<usize, i64>, i64), String> {
    let mut terms: BTreeMap<usize, i64> = BTreeMap::new();
    let mut constant: i64 = 0;
    let place = |name: &str| {
        names
            .iter()
            .position(|n| n.as_str() == name)
            .ok_or_else(|| {
                format!(
                    "{name:?} is no figure of a record: they are {}",
                    names
                        .iter()
                        .map(|n| n.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })
    };
    for term in text.split('+').map(str::trim) {
        if term.is_empty() {
            return Err("a rule's side is terms joined by +".into());
        }
        if let Ok(number) = term.parse::<i64>() {
            constant = constant
                .checked_add(number)
                .ok_or("a number is too large")?;
            continue;
        }
        let (factor, name) = match term.split_once('*') {
            Some((a, b)) => match (a.trim().parse::<i64>(), b.trim().parse::<i64>()) {
                (Ok(factor), Err(_)) => (factor, b.trim()),
                (Err(_), Ok(factor)) => (factor, a.trim()),
                _ => return Err(format!("{term:?} is not a whole number times a figure")),
            },
            None => (1, term),
        };
        let entry = terms.entry(place(name)?).or_insert(0);
        *entry = entry.checked_add(factor).ok_or("a number is too large")?;
    }
    Ok((terms, constant))
}

/// The statements a rule makes about the figures named `names`, in their
/// order: `left <= right`, `left >= right` or `left == right`, each side
/// figures' names, whole numbers and products of the two, joined by `+`.
pub fn statements_of(rule: &str, names: &[&String]) -> Result<Vec<sion_zk::Statement>, String> {
    let (left, how, right) = ["<=", ">=", "=="]
        .iter()
        .find_map(|how| rule.split_once(how).map(|(l, r)| (l, *how, r)))
        .ok_or("a rule compares two sides with <=, >= or ==")?;
    let ((left, left_constant), (right, right_constant)) =
        (side(left, names)?, side(right, names)?);
    // Σ (left − right) · figure ≤ right's number − left's number.
    let mut terms = left;
    for (at, factor) in right {
        let entry = terms.entry(at).or_insert(0);
        *entry = entry.checked_sub(factor).ok_or("a number is too large")?;
    }
    let bound = right_constant
        .checked_sub(left_constant)
        .ok_or("a number is too large")?;
    let at_most = sion_zk::Statement {
        terms: terms.iter().map(|(at, factor)| (*at, *factor)).collect(),
        bound,
    };
    let negated = |statement: &sion_zk::Statement| -> Result<sion_zk::Statement, String> {
        Ok(sion_zk::Statement {
            terms: statement
                .terms
                .iter()
                .map(|(at, factor)| factor.checked_neg().map(|f| (*at, f)))
                .collect::<Option<_>>()
                .ok_or("a number is too large")?,
            bound: statement
                .bound
                .checked_neg()
                .ok_or("a number is too large")?,
        })
    };
    Ok(match how {
        "<=" => vec![at_most],
        ">=" => vec![negated(&at_most)?],
        _ => vec![negated(&at_most)?, at_most],
    })
}

/// What a proof about a sealed record's figures is bound to: the record,
/// and the rule.
fn proof_context(record: &Record, rule: &str) -> Result<Vec<u8>, CliError> {
    let mut context = record.digest()?.0.to_vec();
    context.extend_from_slice(rule.trim().as_bytes());
    Ok(context)
}

/// Proves that the sealed figures of `record`, which `openings` open, keep
/// each of `rules`, and adds the proofs to the record. A rule that does
/// not hold has no proof: the error says which.
pub fn prove_rules(
    record: &mut Record,
    openings: &BTreeMap<String, sion_zk::Opening>,
    rules: &[String],
) -> Result<(), CliError> {
    let Some(sealed) = &record.sealed else {
        return Err(CliError::Usage(
            "the record is not sealed: its operator's figures are in it, in the clear".into(),
        ));
    };
    let names: Vec<&String> = sealed.figures.keys().collect();
    let mut ordered = Vec::new();
    for name in &names {
        let opening = openings
            .get(*name)
            .filter(|opening| Some(&opening.commitment()) == sealed.figures.get(*name))
            .ok_or_else(|| {
                CliError::Usage(format!(
                    "the openings are not this record's: {name} does not open its commitment"
                ))
            })?;
        ordered.push(opening.clone());
    }
    let mut proven = Vec::new();
    for rule in rules {
        let statements = statements_of(rule, &names).map_err(CliError::Usage)?;
        let proof =
            sion_zk::prove(&proof_context(record, rule)?, &ordered, &statements).map_err(|e| {
                match e {
                    sion_zk::Error::False(_) => CliError::Usage(format!(
                    "{rule:?} does not hold of this record's figures: there is no proof to make"
                )),
                    other => CliError::Usage(format!("the proof could not be made: {other}")),
                }
            })?;
        proven.push(Proven {
            rule: rule.trim().to_string(),
            proof,
        });
    }
    record
        .proofs
        .retain(|had| !proven.iter().any(|p| p.rule == had.rule));
    record.proofs.extend(proven);
    Ok(())
}

/// Checks each rule a sealed record's operator proved: a line for each,
/// and whether all hold.
fn check_proofs(record: &Record) -> Result<(Vec<String>, bool), CliError> {
    let mut lines = Vec::new();
    let mut holds = true;
    let Some(sealed) = &record.sealed else {
        if !record.proofs.is_empty() {
            return Ok((
                vec!["differs: the record holds proofs about figures it does not seal".into()],
                false,
            ));
        }
        return Ok((lines, holds));
    };
    let names: Vec<&String> = sealed.figures.keys().collect();
    let commitments: Vec<sion_zk::Commitment> = sealed.figures.values().copied().collect();
    for proven in &record.proofs {
        let verdict = statements_of(&proven.rule, &names).and_then(|statements| {
            let context = proof_context(record, &proven.rule).map_err(|e| e.to_string())?;
            sion_zk::verify(&context, &commitments, &statements, &proven.proof)
                .map_err(|e| e.to_string())
        });
        match verdict {
            Ok(()) => lines.push(format!(
                "holds, proven without the figures: {}",
                proven.rule
            )),
            Err(why) => {
                holds = false;
                lines.push(format!("differs: the proof of {:?}: {why}", proven.rule));
            }
        }
    }
    Ok((lines, holds))
}

/// What a record's claim holds: which agent, which span, and the record's
/// digest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Committed {
    /// The agent.
    pub agent: ObjectId,
    /// The first Pulse counted.
    pub from: u64,
    /// The last.
    pub to: u64,
    /// The record's digest.
    pub digest: String,
}

/// The key an agent's latest record is named under.
pub fn record_key(agent: &ObjectId) -> String {
    format!("agent-record:{}", feeds::short_key(agent.as_hash()))
}

/// The key an agent's warranty is named under.
pub fn warranty_key(agent: &ObjectId) -> String {
    format!("agent-warranty:{}", feeds::short_key(agent.as_hash()))
}

/// The account and owner of `agent`, and what its charter lets it do as
/// the ledger has it: the terms, not the balances, so that it reads the
/// same until someone changes it.
fn identity(
    ledger: &dyn Ledger,
    agent: ObjectId,
) -> Result<(AccountId, Option<AccountId>, Value), CliError> {
    let charter = crate::agents::identity(ledger, agent)?;
    let described = crate::agents::describe(ledger, agent)?;
    let caveats = &charter.caveats;
    let terms = serde_json::json!({
        "account": charter.agent,
        "principal": charter.principal,
        "payees": caveats.payees,
        "max_per_payment_usd": caveats.max_per_payment.to_string(),
        "burst_usd": caveats.burst.to_string(),
        "refill_usd_per_pulse": caveats.rate.to_string(),
        "expires_at": caveats.expires_at,
        "receipts": caveats.receipts,
        "may_spawn": caveats.may_delegate,
        "depth": described["depth"],
        "status": described["status"],
        "narrowing": described["narrowing"],
    });
    Ok((charter.agent, Some(charter.principal), terms))
}

/// Adds up what the ledger's events from Pulse `from` to `to` show of
/// `agent`, which `account` signs for and `owner` registered.
pub fn on_ledger(
    ledger: &dyn Ledger,
    agent: ObjectId,
    account: AccountId,
    owner: Option<AccountId>,
    from: u64,
    to: u64,
) -> Result<OnLedger, CliError> {
    let mut shown = OnLedger {
        from,
        to,
        ..OnLedger::default()
    };
    let mut payees: BTreeSet<AccountId> = BTreeSet::new();
    let mut atoms: BTreeSet<AtomId> = BTreeSet::new();
    let mut open: BTreeSet<Hash32> = BTreeSet::new();
    let mut events: Vec<Event> = Vec::new();
    let step_key = crate::sealed::step_key(&agent);
    let seal_key = crate::sealed::seal_key(&agent);
    let mut next = from;
    'pages: loop {
        let mut page = ledger.events(Pulse(next), PAGE)?;
        let full = page.len() >= PAGE;
        let (Some(first), Some(last)) = (
            page.first().map(|view| view.pulse),
            page.last().map(|view| view.pulse),
        ) else {
            break;
        };
        // A full page may end inside a Pulse: that Pulse is read again,
        // whole, with the next page.
        let whole = !(full && first != last);
        if !whole {
            page.retain(|view| view.pulse != last);
        }
        for view in page {
            if view.pulse.0 > to {
                break 'pages;
            }
            match &view.event {
                Event::AgentRegistered { agent: id, .. } if *id == agent => {
                    shown.registered_at = Some(view.pulse.0);
                }
                Event::AgentPaid {
                    charter,
                    payee,
                    amount,
                    ..
                } if *charter == agent => {
                    shown.payments += 1;
                    shown.paid = shown.paid.checked_add(*amount).unwrap_or(shown.paid);
                    shown.largest = shown.largest.max(*amount);
                    payees.insert(*payee);
                }
                Event::AgentSpawned { parent, .. } if *parent == agent => shown.subagents += 1,
                Event::AgentSuspended { agent: id, .. } if *id == agent => shown.suspended += 1,
                Event::AgentResumed { agent: id } if *id == agent => shown.resumed += 1,
                Event::AgentNarrowed { agent: id, .. } if *id == agent => shown.narrowed += 1,
                Event::ClaimIndexed { owner: by, key, .. }
                    if *by == account || Some(*by) == owner =>
                {
                    if *key == step_key {
                        shown.improvements += 1;
                    } else if *key == seal_key {
                        shown.seals += 1;
                    }
                }
                Event::BondPosted {
                    atom, owner: by, ..
                } if *by == account || Some(*by) == owner => {
                    if atoms.insert(*atom) {
                        shown.bonds += 1;
                    }
                }
                Event::ChallengeOpened {
                    challenge, target, ..
                } if atoms.contains(target) => {
                    shown.challenges += 1;
                    open.insert(*challenge);
                }
                Event::ChallengeRuled { challenge, upheld } if open.remove(challenge) => {
                    if *upheld {
                        shown.upheld += 1;
                    }
                }
                Event::Slashed { atom, amount } if atoms.contains(atom) => {
                    shown.slashes += 1;
                    shown.slashed = shown.slashed.checked_add(*amount).unwrap_or(shown.slashed);
                }
                _ => {}
            }
            events.push(view.event);
        }
        if !whole {
            next = last.0;
            continue;
        }
        next = last.0.saturating_add(1);
        if !full || next > to {
            break;
        }
    }
    shown.payees = payees.len() as u64;
    // Its work for others: what the ledger's own account of every agent's
    // tasks says of this one.
    if let Some(work) = crate::reputation::records_from(events.iter()).get(&account) {
        shown.delivered = work.delivered;
        shown.earned = work.earned;
        shown.falsified = work.failed;
        shown.released = work.released;
    }
    Ok(shown)
}

/// What this machine's sessions of `agent` show, as its operator's word.
pub fn reported(agent: &ObjectId) -> Reported {
    let mut said = Reported::default();
    let mut digests: Vec<(String, Hash32)> = Vec::new();
    let Ok(entries) = std::fs::read_dir(crate::runtime::sessions_dir()) else {
        return said;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(id) = name.strip_suffix(".json") else {
            continue;
        };
        let Ok(saved) = crate::runtime::Saved::load(id) else {
            continue;
        };
        if saved.agent != *agent {
            continue;
        }
        said.sessions += 1;
        let ended = match &saved.ended {
            Some(sion_agent::Ended::Done { .. }) => "done",
            Some(sion_agent::Ended::NeedsApproval { .. }) => "waiting for a person",
            Some(sion_agent::Ended::Unverified { .. }) => "unverified",
            Some(sion_agent::Ended::Stuck { .. }) => "stuck",
            Some(sion_agent::Ended::CannotComplete { .. }) => "gave up",
            Some(sion_agent::Ended::OutOfBudget { .. }) => "out of budget",
            Some(_) => "other",
            None => "not run",
        };
        *said.ended.entry(ended.to_string()).or_default() += 1;
        if matches!(saved.ended, Some(sion_agent::Ended::Done { .. }))
            && saved.state.attempts.checked > 0
        {
            said.checked += 1;
        }
        let mut names: BTreeMap<&str, &str> = BTreeMap::new();
        for message in &saved.state.messages {
            for block in &message.content {
                match block {
                    sion_agent::Block::ToolUse { id, name, .. } => {
                        names.insert(id.as_str(), name.as_str());
                    }
                    sion_agent::Block::ToolResult { id, is_error, .. } => {
                        said.calls += 1;
                        if *is_error {
                            said.calls_failed += 1;
                            if names.get(id.as_str()) == Some(&"ledger_pay") {
                                said.payments_refused += 1;
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        for decided in &saved.state.decisions {
            said.asked += 1;
            if decided.approved {
                said.approved += 1;
            } else {
                said.declined += 1;
            }
            if matches!(
                crate::runtime::checked_signature(&saved, decided),
                Some(Ok(_))
            ) {
                said.signed += 1;
            }
        }
        said.expected += saved.state.expected.stated;
        said.expected_held += saved.state.expected.held;
        said.attacks_held += saved.state.screened.count;
        let state = serde_json::to_vec(&saved.state).unwrap_or_default();
        digests.push((
            saved.state.id.clone(),
            sion_core::hash::hash_domain("sion-cli/agent-record/session/v1", &state),
        ));
    }
    digests.sort();
    let mut all = Vec::new();
    for (id, digest) in &digests {
        all.extend_from_slice(id.as_bytes());
        all.push(0);
        all.extend_from_slice(&digest.0);
    }
    said.sessions_digest =
        sion_core::hash::hash_domain("sion-cli/agent-record/sessions/v1", &all).to_hex();
    said
}

/// Writes the record of `agent` over Pulses `from` to `to` (the latest, if
/// not given); with `sessions`, with what this machine's sessions show.
pub fn write(
    ledger: &dyn Ledger,
    agent: ObjectId,
    from: u64,
    to: Option<u64>,
    sessions: bool,
) -> Result<Record, CliError> {
    let (account, owner, charter) = identity(ledger, agent)?;
    let to = to.unwrap_or(ledger.pulse()?.0);
    if from > to {
        return Err(CliError::Usage(format!(
            "the span runs from Pulse {from} to {to}: nothing is in it"
        )));
    }
    Ok(Record {
        version: 1,
        chain: ledger.genesis_digest()?.to_hex(),
        agent,
        account,
        owner,
        charter,
        ledger: on_ledger(ledger, agent, account, owner, from, to)?,
        reported: sessions.then(|| reported(&agent)),
        sealed: None,
        proofs: Vec::new(),
    })
}

/// Seals `record`'s operator's part: its figures are committed to in place
/// of being shown. Returns what opens them, for the operator to keep.
pub fn sealed(record: &mut Record) -> Result<BTreeMap<String, sion_zk::Opening>, CliError> {
    let Some(said) = record.reported.take() else {
        return Err(CliError::Usage(
            "the record holds nothing of its operator's to seal".into(),
        ));
    };
    let (commitments, openings) = seal(&said);
    record.sealed = Some(commitments);
    Ok(openings)
}

/// Publishes a commitment to `record` as `wallet`'s claim, named under the
/// agent's record key: when it was written, and that it has not changed
/// since.
pub fn commit(
    session: &mut Session<'_>,
    wallet: &str,
    record: &Record,
) -> Result<ClaimId, CliError> {
    let committed = Committed {
        agent: record.agent,
        from: record.ledger.from,
        to: record.ledger.to,
        digest: record.digest()?.to_hex(),
    };
    let payload = serde_json::to_vec(&committed)
        .map_err(|e| CliError::Usage(format!("the commitment could not be written: {e}")))?;
    let body = ClaimBody {
        schema: RECORD_SCHEMA.into(),
        subject: format!(
            "the record of agent {} from Pulse {} to {}",
            record.agent.short(),
            committed.from,
            committed.to
        ),
        payload: Payload::Public(payload),
        formula: sion_assurance::Formula::and(vec![formulas::dual_signature()]),
        adjudicator: None,
        detection_permille: 0,
        valid_for: 1_000_000,
        challenge_period: 0,
    };
    crate::vouching::publish(session, wallet, body, Some(record_key(&record.agent)))
}

/// Checks `record` against the ledger: a line for each thing looked at,
/// and whether everything the ledger can say of it holds.
pub fn check(ledger: &dyn Ledger, record: &Record) -> Result<(Vec<String>, bool), CliError> {
    let mut lines = Vec::new();
    let mut holds = true;
    let chain = ledger.genesis_digest()?.to_hex();
    if record.chain != chain {
        return Ok((
            vec![format!(
                "the record is of another chain ({}…), not this one ({}…): nothing in it can be \
                 checked here",
                &record.chain[..record.chain.len().min(8)],
                &chain[..8]
            )],
            false,
        ));
    }
    let (account, owner, charter) = identity(ledger, record.agent)?;
    if account != record.account || owner != record.owner {
        holds = false;
        lines.push("differs: the agent's account or owner is not the one the record names".into());
    }
    let now = ledger.pulse()?.0;
    if record.ledger.to > now {
        holds = false;
        lines.push(format!(
            "differs: the record counts to Pulse {}, and the ledger is at {now}",
            record.ledger.to
        ));
    }
    let again = on_ledger(
        ledger,
        record.agent,
        account,
        owner,
        record.ledger.from,
        record.ledger.to,
    )?;
    let (said, shown) = (
        serde_json::to_value(&record.ledger).unwrap_or_default(),
        serde_json::to_value(&again).unwrap_or_default(),
    );
    let mut differing = 0;
    if let (Some(said), Some(shown)) = (said.as_object(), shown.as_object()) {
        let keys: BTreeSet<&String> = said.keys().chain(shown.keys()).collect();
        for key in keys {
            let (a, b) = (said.get(key), shown.get(key));
            if a != b {
                differing += 1;
                holds = false;
                let text = |v: Option<&Value>| v.map_or("nothing".to_string(), Value::to_string);
                lines.push(format!(
                    "differs: {key}: the record says {}, the ledger shows {}",
                    text(a),
                    text(b)
                ));
            }
        }
    }
    if differing == 0 {
        lines.push(format!(
            "holds: what the record says the ledger shows from Pulse {} to {} is what the ledger \
             shows",
            record.ledger.from, record.ledger.to
        ));
    }
    lines.push(if charter == record.charter {
        "holds: the charter is as the record has it".to_string()
    } else {
        "changed since: the charter is no longer as the record has it (sion agent show)".to_string()
    });
    // Whether its owner or its account committed to this very record.
    let digest = record.digest()?.to_hex();
    let mut feed = feeds::Feed {
        next: record.ledger.to,
        ..feeds::Feed::default()
    };
    feed.scan(ledger, &[&record_key(&record.agent)])?;
    let mut committed = None;
    for entry in &feed.entries {
        let Some(read) = feeds::read::<Committed>(ledger, entry.claim, RECORD_SCHEMA)? else {
            continue;
        };
        if read.issuer == entry.owner
            && (read.issuer == account || Some(read.issuer) == owner)
            && read.says.digest == digest
        {
            committed = Some((entry.claim, read.issued_at));
            break;
        }
    }
    lines.push(match committed {
        Some((claim, at)) => format!(
            "holds: its owner committed to this record in claim {} at Pulse {at}",
            claim.short()
        ),
        None => "not on the ledger: no claim of its owner's commits to this record".to_string(),
    });
    lines.push(match (&record.reported, &record.sealed) {
        (Some(said), _) => format!(
            "its operator's word, which the ledger cannot check: what {} sessions on its machine \
             show (their digest {}…)",
            said.sessions,
            &said.sessions_digest[..said.sessions_digest.len().min(16)]
        ),
        (None, Some(sealed)) => format!(
            "its operator's word, sealed: {} figures of its machine's sessions committed to and \
             not shown (their digest {}…)",
            sealed.figures.len(),
            &sealed.sessions_digest[..sealed.sessions_digest.len().min(16)]
        ),
        (None, None) => "the record holds nothing of its operator's own account".to_string(),
    });
    let (proven, proofs_hold) = check_proofs(record)?;
    lines.extend(proven);
    Ok((lines, holds && proofs_hold))
}

/// The record in lines, for a person.
pub fn lines(record: &Record) -> Vec<String> {
    let shown = &record.ledger;
    let mut lines = vec![
        format!(
            "agent {}, signed for by {}{}",
            record.agent.short(),
            record.account.short(),
            match &record.owner {
                Some(owner) => format!(", owned by {}", owner.short()),
                None => String::new(),
            }
        ),
        format!(
            "what the ledger shows, Pulse {} to {}:",
            shown.from, shown.to
        ),
        format!(
            "  {} payment(s) to {} account(s), {} USD in all, the largest {} USD",
            shown.payments, shown.payees, shown.paid, shown.largest
        ),
        format!(
            "  tasks: {} delivered ({} USD), {} proven false, {} given back",
            shown.delivered, shown.earned, shown.falsified, shown.released
        ),
        format!(
            "  suspended {} time(s), narrowed {}, {} subagent(s) launched",
            shown.suspended, shown.narrowed, shown.subagents
        ),
        format!(
            "  {} change(s) kept on sealed tasks, {} seal(s) published",
            shown.improvements, shown.seals
        ),
        format!(
            "  bonds of its account and owner: {} posted, {} challenge(s), {} upheld, {} USD \
             slashed",
            shown.bonds, shown.challenges, shown.upheld, shown.slashed
        ),
    ];
    if let Some(said) = &record.reported {
        lines.push(format!(
            "what its operator says, from {} session(s) on its machine:",
            said.sessions
        ));
        let ended: Vec<String> = said
            .ended
            .iter()
            .map(|(how, n)| format!("{n} {how}"))
            .collect();
        lines.push(format!(
            "  runs ended: {}; {} finished with their check passed",
            if ended.is_empty() {
                "none".to_string()
            } else {
                ended.join(", ")
            },
            said.checked
        ));
        lines.push(format!(
            "  {} call(s), {} failed; {} payment(s) the ledger refused",
            said.calls, said.calls_failed, said.payments_refused
        ));
        lines.push(format!(
            "  a person was asked {} time(s): {} approved, {} declined, {} signed",
            said.asked, said.approved, said.declined, said.signed
        ));
        lines.push(format!(
            "  {} of {} stated expectation(s) held; {} result(s) held back as reported attacks",
            said.expected_held, said.expected, said.attacks_held
        ));
    }
    if let Some(sealed) = &record.sealed {
        lines.push(format!(
            "what its operator says, sealed: committed to and not shown ({})",
            sealed
                .figures
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        ));
        for proven in &record.proofs {
            lines.push(format!("  proven of them: {}", proven.rule));
        }
    }
    lines
}

/// What an operator stands behind, as a warranty says it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Warranty {
    /// The agent.
    pub agent: ObjectId,
    /// What the operator warrants of it, in its own words.
    pub covers: String,
    /// The latest record its owner committed to, if any: what the warranty
    /// was offered on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub record: Option<String>,
}

/// Publishes `warranty` as `wallet`'s claim on its bond
/// [`WARRANTY_BOND`], which `adjudicator` can rule against, named under
/// the agent's warranty key.
pub fn warrant(
    session: &mut Session<'_>,
    wallet: &str,
    warranty: &Warranty,
    adjudicator: AtomId,
    valid_for: u64,
) -> Result<ClaimId, CliError> {
    let covers = warranty
        .covers
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if covers.is_empty() || covers.chars().count() > 600 {
        return Err(CliError::Usage(
            "say what the warranty covers, in at most 600 characters".into(),
        ));
    }
    let issuer = session.id(wallet);
    let payload = serde_json::to_vec(&Warranty {
        covers: covers.clone(),
        ..warranty.clone()
    })
    .map_err(|e| CliError::Usage(format!("the warranty could not be written: {e}")))?;
    let body = ClaimBody {
        schema: WARRANTY_SCHEMA.into(),
        subject: format!(
            "warranty of agent {}: {}",
            warranty.agent.short(),
            covers.chars().take(100).collect::<String>()
        ),
        payload: Payload::Public(payload),
        formula: formulas::bonded_statement(
            bond_atom(AtomClass::Economic, &issuer, WARRANTY_BOND).id,
        ),
        adjudicator: Some(adjudicator),
        detection_permille: 1_000,
        valid_for,
        challenge_period: 100,
    };
    crate::vouching::publish(session, wallet, body, Some(warranty_key(&warranty.agent)))
}

/// `sion agent record`.
#[derive(Clone, Debug, Subcommand)]
pub enum RecordCommand {
    /// Write an agent's record: what the ledger shows of it over a span of
    /// Pulses, which anyone can recompute, and, on the machine that holds
    /// its sessions, what its operator says of them. With --wallet, a claim
    /// commits to the record.
    Write {
        /// The agent (its id).
        agent: String,
        /// The first Pulse counted.
        #[arg(long, default_value_t = 0)]
        from: u64,
        /// The last Pulse counted (default: the latest).
        #[arg(long)]
        to: Option<u64>,
        /// Leave out what this machine's sessions show.
        #[arg(long = "ledger-only")]
        ledger_only: bool,
        /// Seal what this machine's sessions show: each figure is committed
        /// to and not shown, and what opens them is written beside the
        /// record (--out) for you to keep. `prove` then shows whoever asks
        /// that the figures keep a rule, and nothing else about them.
        #[arg(long, requires = "out", conflicts_with = "ledger_only")]
        seal: bool,
        /// Write the record to this file, as JSON.
        #[arg(long, value_name = "FILE")]
        out: Option<PathBuf>,
        /// Commit to the record in a claim of this wallet's.
        #[arg(long)]
        wallet: Option<String>,
    },
    /// Check a record someone gave you against the ledger: every figure it
    /// says the ledger shows is added up again, and whether its owner
    /// committed to it is looked up. What its operator says of its own
    /// sessions is shown as that, and each rule it proved of sealed
    /// figures is verified.
    Check {
        /// The record (a file `write --out` made).
        file: PathBuf,
    },
    /// Prove that a sealed record's figures keep a rule, without showing
    /// them, and add the proof to the record. A rule names figures (calls,
    /// calls_failed, payments_refused, expected, expected_held,
    /// attacks_held, sessions, done, checked, asked, approved, declined,
    /// signed), whole numbers and products of the two, joined by +, and
    /// compares two sides with <=, >= or ==: "calls_failed*50 <= calls".
    Prove {
        /// The sealed record (a file `write --seal --out` made).
        file: PathBuf,
        /// A rule to prove (repeat for more).
        #[arg(long = "rule", value_name = "RULE", required = true)]
        rules: Vec<String>,
        /// What opens the record's commitments (default: the file `write
        /// --seal` wrote beside the record).
        #[arg(long, value_name = "FILE")]
        openings: Option<PathBuf>,
    },
    /// Stand behind an agent, as you: a claim on your bond
    /// `agent-warranty` saying what you warrant of it, which the
    /// adjudicator can rule against. A counterparty relies on it with
    /// `sion accept`, which reserves part of the bond, and is compensated
    /// from the bond if a challenge to it is upheld.
    Warrant {
        /// Your wallet.
        wallet: String,
        /// The agent (its id).
        agent: String,
        /// What you warrant of it.
        #[arg(long)]
        covers: String,
        /// The adjudicator who rules on challenges to it (its atom's id, 64
        /// hex digits).
        #[arg(long)]
        adjudicator: String,
        /// Post your bond `agent-warranty` with this many USD first.
        #[arg(long, value_name = "USD")]
        bond: Option<String>,
        /// How many Pulses the warranty stands for.
        #[arg(long = "valid-for", default_value_t = 1_000_000)]
        valid_for: u64,
    },
}

/// Where what opens a sealed record's figures is kept: beside the record.
fn openings_path(record: &std::path::Path) -> PathBuf {
    record.with_extension("openings.json")
}

fn agent_id(text: &str) -> Result<ObjectId, CliError> {
    Hash32::from_hex(text.trim())
        .map(ObjectId::from_hash)
        .map_err(|_| CliError::Usage("the agent is named by its 64-digit hex id".into()))
}

/// Runs a record command.
pub fn run(
    session: &mut Session<'_>,
    ledger: &dyn Ledger,
    command: RecordCommand,
) -> Result<(), CliError> {
    match command {
        RecordCommand::Write {
            agent,
            from,
            to,
            ledger_only,
            seal,
            out,
            wallet,
        } => {
            let mut record = write(ledger, agent_id(&agent)?, from, to, !ledger_only)?;
            let openings = if seal {
                Some(sealed(&mut record)?)
            } else {
                None
            };
            for line in lines(&record) {
                println!("{line}");
            }
            println!("digest {}", record.digest()?.to_hex());
            if let Some(path) = out {
                crate::profiles::write_json(&path, &record)?;
                println!("written to {}", path.display());
                if let Some(openings) = openings {
                    let kept = openings_path(&path);
                    crate::profiles::write_json(&kept, &openings)?;
                    println!(
                        "what opens its sealed figures is in {}: keep it to yourself",
                        kept.display()
                    );
                }
            }
            if let Some(wallet) = wallet {
                let claim = commit(session, &wallet, &record)?;
                println!("committed to in claim {}", claim.as_hash().to_hex());
            }
            Ok(())
        }
        RecordCommand::Check { file } => {
            let text = std::fs::read_to_string(&file)
                .map_err(|e| CliError::Usage(format!("{}: {e}", file.display())))?;
            let record: Record = serde_json::from_str(&text)
                .map_err(|e| CliError::Usage(format!("{} is not a record: {e}", file.display())))?;
            let (lines, holds) = check(ledger, &record)?;
            for line in lines {
                println!("{line}");
            }
            if holds {
                Ok(())
            } else {
                Err(CliError::Usage(
                    "the record does not hold against the ledger".into(),
                ))
            }
        }
        RecordCommand::Prove {
            file,
            rules,
            openings,
        } => {
            let text = std::fs::read_to_string(&file)
                .map_err(|e| CliError::Usage(format!("{}: {e}", file.display())))?;
            let mut record: Record = serde_json::from_str(&text)
                .map_err(|e| CliError::Usage(format!("{} is not a record: {e}", file.display())))?;
            let kept = openings.unwrap_or_else(|| openings_path(&file));
            let opened: BTreeMap<String, sion_zk::Opening> = std::fs::read_to_string(&kept)
                .map_err(|e| CliError::Usage(format!("{}: {e}", kept.display())))
                .and_then(|text| {
                    serde_json::from_str(&text).map_err(|e| {
                        CliError::Usage(format!("{} does not open a record: {e}", kept.display()))
                    })
                })?;
            prove_rules(&mut record, &opened, &rules)?;
            crate::profiles::write_json(&file, &record)?;
            for rule in &rules {
                println!("proven, without the figures: {}", rule.trim());
            }
            println!("added to {}", file.display());
            Ok(())
        }
        RecordCommand::Warrant {
            wallet,
            agent,
            covers,
            adjudicator,
            bond,
            valid_for,
        } => {
            let agent = agent_id(&agent)?;
            let adjudicator = Hash32::from_hex(adjudicator.trim())
                .map(AtomId)
                .map_err(|e| CliError::Usage(format!("the adjudicator's id: {e}")))?;
            if let Some(amount) = bond {
                let view = session.must(
                    &wallet,
                    TxKind::PostBond {
                        class: AtomClass::Economic,
                        label: WARRANTY_BOND.into(),
                        amount: crate::agents::parse_amount(&amount)?,
                    },
                    Vec::new(),
                )?;
                crate::agents::print_receipt(session, &view);
            }
            // The record its owner last committed to, if there is one.
            let mut feed = feeds::Feed::default();
            feed.scan(ledger, &[&record_key(&agent)])?;
            let me = session.id(&wallet);
            let record = feed
                .entries
                .iter()
                .rev()
                .find(|entry| entry.owner == me)
                .map(|entry| entry.claim.as_hash().to_hex());
            let warranty = Warranty {
                agent,
                covers,
                record,
            };
            let claim = warrant(session, &wallet, &warranty, adjudicator, valid_for)?;
            println!(
                "warranted agent {}: claim {}, on your bond {WARRANTY_BOND}",
                agent.short(),
                claim.as_hash().to_hex()
            );
            println!(
                "  a counterparty relies on it with: sion accept <wallet> {} <USD>",
                claim.as_hash().to_hex()
            );
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rule_is_read_as_statements_about_figures() {
        let all = ["calls".to_string(), "calls_failed".to_string()];
        let names: Vec<&String> = all.iter().collect();
        // calls_failed*50 <= calls: 50·failed − calls ≤ 0.
        assert_eq!(
            statements_of("calls_failed*50 <= calls", &names).unwrap(),
            vec![sion_zk::Statement {
                terms: vec![(0, -1), (1, 50)],
                bound: 0
            }]
        );
        // A number on either side, a figure times a number either way.
        assert_eq!(
            statements_of("2 + calls_failed <= 3*calls + 10", &names).unwrap(),
            vec![sion_zk::Statement {
                terms: vec![(0, -3), (1, 1)],
                bound: 8
            }]
        );
        assert_eq!(
            statements_of("calls >= 5", &names).unwrap(),
            vec![sion_zk::Statement {
                terms: vec![(0, -1)],
                bound: -5
            }]
        );
        assert_eq!(statements_of("calls_failed == 0", &names).unwrap().len(), 2);
        for wrong in [
            "calls",
            "calls < 3",
            "tokens <= 3",
            "calls*calls_failed <= 3",
            "calls + <= 3",
        ] {
            assert!(statements_of(wrong, &names).is_err(), "{wrong}");
        }
    }

    #[test]
    fn sealed_figures_are_proven_to_keep_rules_and_only_true_ones() {
        let said = Reported {
            sessions: 40,
            calls: 400,
            calls_failed: 6,
            payments_refused: 1,
            expected: 50,
            expected_held: 47,
            sessions_digest: "ab".repeat(32),
            ..Reported::default()
        };
        let mut record = Record {
            version: 1,
            chain: "00".repeat(32),
            agent: ObjectId::from_hash(Hash32([1; 32])),
            account: AccountId::from_hash(Hash32([2; 32])),
            owner: None,
            charter: Value::Null,
            ledger: OnLedger::default(),
            reported: Some(said),
            sealed: None,
            proofs: Vec::new(),
        };
        let openings = sealed(&mut record).unwrap();
        // The figures are no longer in the record.
        assert!(record.reported.is_none());
        let text = serde_json::to_string(&record).unwrap();
        assert!(
            !text.contains("calls_failed\":6") && !text.contains(":400"),
            "{text}"
        );
        let digest = record.digest().unwrap();
        let rules: Vec<String> = [
            "calls_failed*50 <= calls",
            "payments_refused <= 3",
            "expected_held*100 >= 90*expected",
            "attacks_held == 0",
        ]
        .iter()
        .map(|r| r.to_string())
        .collect();
        prove_rules(&mut record, &openings, &rules).unwrap();
        // The proofs are added without changing what was committed to.
        assert_eq!(record.digest().unwrap(), digest);
        let (lines, holds) = check_proofs(&record).unwrap();
        assert!(holds && lines.len() == 4, "{lines:?}");
        assert!(lines[0].contains("holds, proven without the figures: calls_failed*50 <= calls"));
        // What is not so has no proof.
        let not_so = prove_rules(
            &mut record,
            &openings,
            &["payments_refused == 0".to_string()],
        )
        .unwrap_err()
        .to_string();
        assert!(
            not_so.contains("does not hold of this record's figures"),
            "{not_so}"
        );
        // A proof does not pass for another rule, another record, or
        // figures committed to again.
        let mut claimed = record.clone();
        claimed.proofs[1].rule = "payments_refused <= 0".into();
        assert!(!check_proofs(&claimed).unwrap().1);
        let mut elsewhere = record.clone();
        elsewhere.ledger.payments = 7;
        assert!(!check_proofs(&elsewhere).unwrap().1);
        let mut recommitted = record.clone();
        let (fresh, _) = sion_zk::commit(1);
        recommitted
            .sealed
            .as_mut()
            .unwrap()
            .figures
            .insert("payments_refused".into(), fresh);
        assert!(!check_proofs(&recommitted).unwrap().1);
        // Openings that are not this record's are refused.
        let (_, others) = seal(&Reported::default());
        assert!(prove_rules(&mut record, &others, &rules).is_err());
    }
}
