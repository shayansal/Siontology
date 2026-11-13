//! Lessons shared between owners: what one agent verifiably learned,
//! offered to every other, and taken up only where it is confirmed again.
//!
//! An agent that improves itself keeps a change only when it is better on
//! tasks sealed before the change was written ([`crate::improve_loop`]).
//! Until now that lesson stayed with its owner. Here its owner can
//! **share** it: a claim on the ledger holding the lesson's words and the
//! evidence it was kept on, on the author's bond, under an adjudicator who
//! can rule against it, named under a key anyone finds it by
//! ([`crate::feeds`]).
//!
//! Another owner's agent does not take a shared lesson on its author's
//! word. It **learns** one the way it keeps any change: the lesson is
//! added to what the agent has learned, tried on the owner's own tasks,
//! and kept only when it is better beyond chance there and then on the
//! owner's own sealed tasks, which the lesson's author never saw. A lesson
//! that helped its author and does nothing here is turned down like any
//! proposal that does nothing.
//!
//! When one is kept, the owner can say so (**an adoption**: a claim naming
//! the lesson, with the evidence it was kept on here) and pay the author
//! the price the lesson names. Both are the adopter's choice: the words of
//! a lesson are public, so nothing forces a reader who uses them to say so
//! or to pay. What an adoption gives is a record anyone can check of where
//! a lesson was confirmed, by whom.
//!
//! A lesson is words added to an agent's instructions, written by a
//! stranger. What stands between a harmful one and an agent is the same as
//! for a proposer's change, plus what a stranger's words call for: a short
//! limit on its length ([`MAX_LESSON`]), the gates of the loop, the
//! owner's own sealed tasks, a bond that an adjudicator can take, the
//! owner's choice of whose lessons to try at all (`--by`), and `--apply`,
//! without which a kept lesson waits for the owner to adopt it by hand.
//! None of that makes a lesson safe that the owner's tasks do not test.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use sion_assurance::AtomClass;
use sion_core::ids::{AccountId, AtomId, ClaimId, ObjectId};
use sion_core::Amount;
use sion_exec::claim::{formulas, ClaimBody, Payload};
use sion_exec::execute::bond_atom;
use sion_exec::TxKind;

use crate::error::CliError;
use crate::feeds::{self, Feed};
use crate::improve_loop::{Offered, Step};
use crate::ledger::Ledger;
use crate::session::Session;

/// The schema of a claim sharing a lesson.
pub const LESSON_SCHEMA: &str = "agent.lesson/v1";

/// The schema of a claim saying a lesson was kept somewhere else.
pub const ADOPTION_SCHEMA: &str = "agent.lesson.adoption/v1";

/// The bond an author stands behind its lessons with.
pub const AUTHOR_BOND: &str = "lesson-author";

/// The longest a shared lesson may be, in bytes: a few sentences. What an
/// agent has learned may be longer; what a stranger adds to it may not.
pub const MAX_LESSON: usize = 1_000;

/// The tools a lesson names its tasks by, at most.
pub const MAX_TOOLS: usize = 24;

/// What a lesson's key begins with.
pub const LESSON_PREFIX: &str = "lesson:";

/// What an adoption's key begins with.
pub const ADOPTED_PREFIX: &str = "lesson-adopted:";

/// How an agent did before and after, as runs passed of runs made.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counts {
    /// Before: passed, of.
    pub before: (u64, u64),
    /// After: passed, of.
    pub after: (u64, u64),
}

impl Counts {
    fn of(before: &crate::self_improve::Outcome, after: &crate::self_improve::Outcome) -> Counts {
        Counts {
            before: (before.passed as u64, before.total as u64),
            after: (after.passed as u64, after.total as u64),
        }
    }

    /// `after/of against before/of`.
    pub fn shown(&self) -> String {
        format!(
            "{}/{} against {}/{}",
            self.after.0, self.after.1, self.before.0, self.before.1
        )
    }
}

/// A lesson as its author shares it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Lesson {
    /// The lesson: what the author's agent added to what it had learned.
    pub text: String,
    /// Why whoever wrote it said it would help.
    pub why: String,
    /// The tools the tasks it was kept on gave the agent, by name: what a
    /// reader tells by whether it bears on its own agent's work.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<String>,
    /// The author's agent.
    pub agent: ObjectId,
    /// The kept change it comes from, in the author's lineage.
    pub step: String,
    /// The claim that published that change, if one did: the full record
    /// of it, issued when it was kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_claim: Option<String>,
    /// The claim of the seal it was confirmed on, if it was published:
    /// the digests of the author's tasks, issued before anything was tried.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seal_claim: Option<String>,
    /// How the author's agent did on the tasks the change was chosen on.
    pub chosen: Counts,
    /// How it did on the author's sealed tasks.
    pub sealed: Counts,
    /// The interval for the difference on the sealed tasks, at the seal's
    /// level.
    pub confirm: (f64, f64),
    /// What the author asks of an owner whose agent keeps it, in millionths
    /// of a USD; nothing, if 0.
    #[serde(default)]
    pub price_micros: u64,
}

/// An adoption: a lesson kept by another agent, as its owner says.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Adoption {
    /// The lesson's claim.
    pub lesson: String,
    /// The lesson's author.
    pub author: AccountId,
    /// The adopter's agent.
    pub agent: ObjectId,
    /// The kept change in the adopter's lineage.
    pub step: String,
    /// The claim that published that change, if one did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_claim: Option<String>,
    /// The claim of the adopter's seal, if it was published.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seal_claim: Option<String>,
    /// How the adopter's agent did on the tasks it chose the lesson on.
    pub chosen: Counts,
    /// How it did on the adopter's sealed tasks.
    pub sealed: Counts,
    /// The interval for the difference there, at the adopter's seal's
    /// level.
    pub confirm: (f64, f64),
    /// What the adopter paid the author, in millionths of a USD.
    #[serde(default)]
    pub paid_micros: u64,
}

/// What a kept change added to what the agent had learned: the rest of
/// `to` where it goes on from `from`, and all of `to` otherwise.
pub fn added(from: &str, to: &str) -> String {
    let (from, to) = (from.trim(), to.trim());
    match to.strip_prefix(from) {
        Some(rest) if !from.is_empty() => rest.trim().to_string(),
        _ => to.to_string(),
    }
}

/// The tools the tasks of `set` give an agent, by name, sorted: at most
/// [`MAX_TOOLS`].
pub fn tools_of(set: &sion_agent::compare::TaskSet) -> Vec<String> {
    let names: BTreeSet<&str> = set
        .cases
        .iter()
        .flat_map(|case| case.tools.iter().map(|tool| tool.name.as_str()))
        .collect();
    names
        .into_iter()
        .take(MAX_TOOLS)
        .map(str::to_string)
        .collect()
}

impl Lesson {
    /// The lesson `step` kept, for sharing: what it added to what the
    /// agent had learned, with the evidence it was kept on. `tools` are
    /// those of the tasks it was chosen on.
    pub fn of(step: &Step, tools: Vec<String>, price: Amount) -> Result<Lesson, CliError> {
        let text = added(&step.from.learned, &step.to.learned);
        if text.is_empty() || step.to.learned.trim() == step.from.learned.trim() {
            return Err(CliError::Usage(format!(
                "step {} changed the models the agent runs on, not what it has learned: there is \
                 no lesson in it to share",
                step.id
            )));
        }
        check_text(&text)?;
        let price_micros =
            u64::try_from(price.0).map_err(|_| CliError::Usage("the price is too large".into()))?;
        Ok(Lesson {
            text,
            why: one_line(&step.why, 300),
            tools,
            agent: step.agent,
            step: step.id.clone(),
            step_claim: step.claim.clone(),
            seal_claim: step.seal_claim.clone(),
            chosen: Counts::of(&step.train_before, &step.train_after),
            sealed: Counts::of(&step.sealed_before, &step.sealed_after),
            confirm: step.confirm,
            price_micros,
        })
    }

    /// The key it is named under: by its words, so that the same lesson
    /// shared twice has one name.
    pub fn key(&self) -> String {
        key_of(&self.text)
    }

    /// What it asks of an owner whose agent keeps it.
    pub fn price(&self) -> Amount {
        Amount(u128::from(self.price_micros))
    }
}

/// The key a lesson of these words is named under.
pub fn key_of(text: &str) -> String {
    let digest = sion_core::hash::hash_domain("sion-cli/lesson/v1", text.trim().as_bytes());
    format!("{LESSON_PREFIX}{}", feeds::short_key(&digest))
}

/// The claim `hex` names: 64 hex digits.
pub fn claim_of(hex: &str) -> Result<ClaimId, CliError> {
    sion_core::hash::Hash32::from_hex(hex.trim())
        .map(ClaimId::from_hash)
        .map_err(|e| CliError::Usage(format!("{hex:?} is not a claim's id: {e}")))
}

/// The key an adoption of the lesson `claim` is named under.
pub fn adopted_key(claim: &ClaimId) -> String {
    format!("{ADOPTED_PREFIX}{}", feeds::short_key(claim.as_hash()))
}

/// `text` on one line, at most `most` characters.
fn one_line(text: &str, most: usize) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    line.chars().take(most).collect()
}

/// Whether `text` can be shared, or learned, as a lesson: not empty, at
/// most [`MAX_LESSON`] bytes, and nothing in it but text (no control
/// characters other than line breaks).
pub fn check_text(text: &str) -> Result<(), CliError> {
    let text = text.trim();
    if text.is_empty() {
        return Err(CliError::Usage("the lesson is empty".into()));
    }
    if text.len() > MAX_LESSON {
        return Err(CliError::Usage(format!(
            "the lesson is {} bytes, more than the {MAX_LESSON} a shared lesson may be",
            text.len()
        )));
    }
    if text.chars().any(|c| c.is_control() && c != '\n') {
        return Err(CliError::Usage(
            "the lesson holds control characters".into(),
        ));
    }
    Ok(())
}

/// The body of a claim sharing `lesson`, on the author's bond, which
/// `adjudicator` can rule against.
pub fn lesson_body(
    author: &AccountId,
    lesson: &Lesson,
    adjudicator: AtomId,
    valid_for: u64,
) -> Result<ClaimBody, CliError> {
    let payload = serde_json::to_vec(lesson)
        .map_err(|e| CliError::Usage(format!("the lesson could not be written: {e}")))?;
    Ok(ClaimBody {
        schema: LESSON_SCHEMA.into(),
        subject: format!(
            "a lesson agent {} kept: {}",
            lesson.agent.short(),
            one_line(&lesson.text, 80)
        ),
        payload: Payload::Public(payload),
        formula: formulas::bonded_statement(bond_atom(AtomClass::Economic, author, AUTHOR_BOND).id),
        adjudicator: Some(adjudicator),
        detection_permille: 1_000,
        valid_for,
        challenge_period: 100,
    })
}

/// Shares `lesson` as `wallet`'s claim, named under the lesson's key.
pub fn share(
    session: &mut Session<'_>,
    wallet: &str,
    lesson: &Lesson,
    adjudicator: AtomId,
    valid_for: u64,
) -> Result<ClaimId, CliError> {
    let author = session.id(wallet);
    let body = lesson_body(&author, lesson, adjudicator, valid_for)?;
    crate::vouching::publish(session, wallet, body, Some(lesson.key()))
}

/// The body of a claim saying `adoption`: it rests on the adopter's
/// signature alone, and its evidence is the adopter's own published step
/// and seal.
pub fn adoption_body(adoption: &Adoption, valid_for: u64) -> Result<ClaimBody, CliError> {
    let payload = serde_json::to_vec(adoption)
        .map_err(|e| CliError::Usage(format!("the adoption could not be written: {e}")))?;
    Ok(ClaimBody {
        schema: ADOPTION_SCHEMA.into(),
        subject: format!(
            "agent {} kept lesson {}",
            adoption.agent.short(),
            &adoption.lesson[..adoption.lesson.len().min(8)]
        ),
        payload: Payload::Public(payload),
        formula: sion_assurance::Formula::and(vec![formulas::dual_signature()]),
        adjudicator: None,
        detection_permille: 0,
        valid_for,
        challenge_period: 0,
    })
}

/// Publishes that the lesson `step` came from was kept by the adopter's
/// agent, as `wallet`'s claim, after paying the lesson's author its price
/// if `pay` covers it. Returns the adoption's claim and what was paid.
pub fn adopt(
    session: &mut Session<'_>,
    wallet: &str,
    step: &Step,
    pay: Amount,
    valid_for: u64,
) -> Result<(ClaimId, Amount), CliError> {
    let Some(lesson_hex) = &step.lesson else {
        return Err(CliError::Usage(format!(
            "step {} did not come from a shared lesson",
            step.id
        )));
    };
    let lesson_id = claim_of(lesson_hex)?;
    let Some(read) = feeds::read::<Lesson>(session.ledger, lesson_id, LESSON_SCHEMA)? else {
        return Err(CliError::Usage(format!(
            "claim {} is not a lesson",
            lesson_id.short()
        )));
    };
    // Paid first, so that the adoption says what was paid. Never more than
    // the lesson asks, nor than the owner allowed.
    let price = read.says.price();
    let paid = if price > Amount::ZERO && price <= pay && read.issuer != session.id(wallet) {
        session.must(
            wallet,
            TxKind::Transfer {
                asset: sion_core::ids::AssetId::usd(),
                outputs: vec![(read.issuer, price)],
            },
            Vec::new(),
        )?;
        price
    } else {
        Amount::ZERO
    };
    let adoption = Adoption {
        lesson: lesson_hex.clone(),
        author: read.issuer,
        agent: step.agent,
        step: step.id.clone(),
        step_claim: step.claim.clone(),
        seal_claim: step.seal_claim.clone(),
        chosen: Counts::of(&step.train_before, &step.train_after),
        sealed: Counts::of(&step.sealed_before, &step.sealed_after),
        confirm: step.confirm,
        paid_micros: u64::try_from(paid.0).unwrap_or(u64::MAX),
    };
    let body = adoption_body(&adoption, valid_for)?;
    let claim = crate::vouching::publish(session, wallet, body, Some(adopted_key(&lesson_id)))?;
    Ok((claim, paid))
}

/// A lesson found on the ledger, with who kept it since.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    /// Its claim.
    pub claim: ClaimId,
    /// Who shared it.
    pub author: AccountId,
    /// Whether its claim stands.
    pub active: bool,
    /// What it says.
    pub lesson: Lesson,
    /// Who says their agent kept it, each once: the adopter, and its
    /// adoption. An author's adoption of its own lesson is left out.
    pub adoptions: Vec<(AccountId, Adoption)>,
    /// What of its evidence the ledger holds.
    pub evidence: Evidence,
}

/// What the ledger holds of the evidence a lesson names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Evidence {
    /// The change it comes from is a claim its author published, which
    /// holds these words and these counts; with the Pulse it was issued
    /// in, and the Pulse its seal was, if the seal was published before it.
    Published {
        /// The Pulse the change's claim was issued in.
        kept_at: u64,
        /// The Pulse the seal's claim was issued in, if before.
        sealed_at: Option<u64>,
    },
    /// Nothing the lesson names is on the ledger, or what is there says
    /// otherwise: it rests on its author's word, and why.
    WordAlone(String),
}

impl Evidence {
    /// The evidence in a line.
    pub fn shown(&self) -> String {
        match self {
            Evidence::Published {
                kept_at,
                sealed_at: Some(sealed_at),
            } => format!(
                "its author published the change at Pulse {kept_at}, on tasks sealed at Pulse \
                 {sealed_at}"
            ),
            Evidence::Published { kept_at, .. } => format!(
                "its author published the change at Pulse {kept_at}; no seal published before it"
            ),
            Evidence::WordAlone(why) => format!("its author's word alone ({why})"),
        }
    }
}

/// What the ledger holds of the evidence `lesson`, shared by `author`,
/// names: whether the change it comes from was published by the same
/// account with these words and counts, and its seal before it.
///
/// This shows that the author recorded the change when it says, on tasks
/// fixed before. It does not show that the runs happened: the counts are
/// the author's own, as every lineage's are.
pub fn evidence(
    ledger: &dyn Ledger,
    author: &AccountId,
    lesson: &Lesson,
) -> Result<Evidence, CliError> {
    let word = |why: &str| Ok(Evidence::WordAlone(why.to_string()));
    let Some(step_hex) = &lesson.step_claim else {
        return word("it names no published change");
    };
    let Ok(step_id) = claim_of(step_hex) else {
        return word("the change it names is no claim");
    };
    let Some(step) = feeds::read::<Step>(ledger, step_id, crate::sealed::STEP_SCHEMA)? else {
        return word("the claim it names is not a kept change");
    };
    if step.issuer != *author {
        return word("the change it names is someone else's");
    }
    let says = &step.says;
    if says.id != lesson.step
        || added(&says.from.learned, &says.to.learned) != lesson.text.trim()
        || Counts::of(&says.train_before, &says.train_after) != lesson.chosen
        || Counts::of(&says.sealed_before, &says.sealed_after) != lesson.sealed
    {
        return word("the change it names says otherwise");
    }
    let sealed_at = match says.seal_claim.as_deref().map(claim_of) {
        Some(Ok(seal)) => {
            feeds::read::<serde_json::Value>(ledger, seal, crate::sealed::SEAL_SCHEMA)?
                .filter(|read| read.issuer == *author && read.issued_at <= step.issued_at)
                .map(|read| read.issued_at)
        }
        _ => None,
    };
    Ok(Evidence::Published {
        kept_at: step.issued_at,
        sealed_at,
    })
}

/// Where the lessons found so far are kept on this machine.
pub fn feed_path() -> PathBuf {
    crate::model_routes::dir().join("lessons-feed.json")
}

/// Where the lessons a profile tried on a seal are kept.
fn tried_path(profile: &str) -> PathBuf {
    crate::model_routes::dir().join(format!("{profile}-lessons.json"))
}

/// The lessons `profile` already tried on the seal whose sealed tasks have
/// digest `sealed`, by claim.
pub fn tried(profile: &str, sealed: &str) -> BTreeSet<String> {
    let all: BTreeMap<String, BTreeSet<String>> = std::fs::read_to_string(tried_path(profile))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    all.get(sealed).cloned().unwrap_or_default()
}

/// Notes that `profile` tried the lessons `claims` on that seal.
pub fn note_tried(profile: &str, sealed: &str, claims: &[String]) -> Result<(), CliError> {
    let path = tried_path(profile);
    let mut all: BTreeMap<String, BTreeSet<String>> = std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    all.entry(sealed.to_string())
        .or_default()
        .extend(claims.iter().cloned());
    crate::profiles::write_json(&path, &all)
}

/// Brings what is known of shared lessons up to date from the ledger and
/// reads each: the lessons whose claims are lessons, named under the key
/// of their own words by whoever issued them, with the adoptions named
/// under their keys by whoever issued those.
pub fn refresh(ledger: &dyn Ledger) -> Result<Vec<Found>, CliError> {
    let path = feed_path();
    let mut feed: Feed = std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    feed.scan(ledger, &[LESSON_PREFIX, ADOPTED_PREFIX])?;
    crate::profiles::write_json(&path, &feed)?;
    let mut found: Vec<Found> = Vec::new();
    for entry in feed.under(LESSON_PREFIX) {
        let Some(read) = feeds::read::<Lesson>(ledger, entry.claim, LESSON_SCHEMA)? else {
            continue;
        };
        // A lesson is its issuer's, named under the key of its own words:
        // anything else under a lesson's key is not one.
        if read.issuer != entry.owner
            || read.says.key() != entry.key
            || check_text(&read.says.text).is_err()
            || found.iter().any(|f| f.claim == entry.claim)
        {
            continue;
        }
        let evidence = evidence(ledger, &read.issuer, &read.says)?;
        found.push(Found {
            claim: entry.claim,
            author: read.issuer,
            active: read.active,
            lesson: read.says,
            adoptions: Vec::new(),
            evidence,
        });
    }
    for entry in feed.under(ADOPTED_PREFIX) {
        let Some(read) = feeds::read::<Adoption>(ledger, entry.claim, ADOPTION_SCHEMA)? else {
            continue;
        };
        if read.issuer != entry.owner {
            continue;
        }
        let Some(lesson) = found.iter_mut().find(|f| {
            f.claim.as_hash().to_hex() == read.says.lesson && adopted_key(&f.claim) == entry.key
        }) else {
            continue;
        };
        if read.issuer == lesson.author || lesson.adoptions.iter().any(|(by, _)| *by == read.issuer)
        {
            continue;
        }
        lesson.adoptions.push((read.issuer, read.says));
    }
    Ok(found)
}

/// Whose lessons, and which, an owner lets an agent try.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Choice {
    /// Only lessons by these authors, if any are named.
    pub by: Vec<AccountId>,
    /// Only these lessons, by a prefix of their claim, if any are named.
    pub only: Vec<String>,
    /// Lessons kept by at least this many other owners' agents.
    pub adoptions: usize,
    /// At most this many.
    pub most: usize,
}

/// The lessons among `found` that `choice` lets `agent` try, as the loop
/// is offered them: those that stand, are not its own, bear on tools its
/// tasks give it (`tools`; a lesson that names none bears on any), and
/// were not `tried` already. The most adopted first, then the oldest.
pub fn offered(
    found: &[Found],
    agent: &ObjectId,
    tools: &[String],
    tried: &BTreeSet<String>,
    choice: &Choice,
) -> Vec<Offered> {
    let mut fit: Vec<&Found> = found
        .iter()
        .filter(|f| f.active && f.lesson.agent != *agent)
        .filter(|f| choice.by.is_empty() || choice.by.contains(&f.author))
        .filter(|f| {
            let hex = f.claim.as_hash().to_hex();
            choice.only.is_empty() || choice.only.iter().any(|prefix| hex.starts_with(prefix))
        })
        .filter(|f| f.adoptions.len() >= choice.adoptions)
        .filter(|f| f.lesson.tools.is_empty() || f.lesson.tools.iter().any(|t| tools.contains(t)))
        .filter(|f| !tried.contains(&f.claim.as_hash().to_hex()))
        .collect();
    // Stable: among lessons as much adopted, the order they were found in.
    fit.sort_by_key(|f| std::cmp::Reverse(f.adoptions.len()));
    fit.into_iter()
        .take(choice.most)
        .map(|f| Offered {
            claim: f.claim.as_hash().to_hex(),
            text: f.lesson.text.trim().to_string(),
            why: format!(
                "a lesson shared by {} (claim {}), kept by its agent on {} of its sealed runs",
                f.author.short(),
                f.claim.short(),
                f.lesson.sealed.shown()
            ),
        })
        .collect()
}

/// A line for each lesson found, with what a reader weighs it by.
pub fn lines(found: &[Found]) -> Vec<String> {
    if found.is_empty() {
        return vec!["no lesson is shared on this ledger yet".to_string()];
    }
    let mut lines = Vec::new();
    for f in found {
        let price = match f.lesson.price_micros {
            0 => "asks nothing".to_string(),
            _ => format!(
                "asks {} USD of an owner whose agent keeps it",
                f.lesson.price()
            ),
        };
        lines.push(format!(
            "lesson {} by {}{}: {}",
            f.claim.short(),
            f.author.short(),
            if f.active { "" } else { " (no longer stands)" },
            f.lesson.text.trim().replace('\n', " / ")
        ));
        lines.push(format!(
            "  it says its agent {} kept it on {} sealed runs, {} on the tasks it was chosen on; \
             {price}",
            f.lesson.agent.short(),
            f.lesson.sealed.shown(),
            f.lesson.chosen.shown(),
        ));
        lines.push(format!("  evidence: {}", f.evidence.shown()));
        if !f.lesson.tools.is_empty() {
            lines.push(format!("  its tasks' tools: {}", f.lesson.tools.join(", ")));
        }
        match f.adoptions.len() {
            0 => lines.push("  kept by no other owner's agent yet".to_string()),
            n => {
                lines.push(format!("  kept by {n} other owner(s)' agents since:"));
                for (by, adoption) in &f.adoptions {
                    lines.push(format!(
                        "    {}: {} on its sealed runs{}",
                        by.short(),
                        adoption.sealed.shown(),
                        match adoption.paid_micros {
                            0 => String::new(),
                            paid => format!(", paid {} USD", Amount(u128::from(paid))),
                        }
                    ));
                }
            }
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lesson_is_what_a_change_added_and_is_kept_short() {
        assert_eq!(added("", " Check the payee. "), "Check the payee.");
        assert_eq!(
            added("Check the payee.", "Check the payee.\nTotals are in cents."),
            "Totals are in cents."
        );
        // Rewritten rather than added to: all of it.
        assert_eq!(
            added("Check the payee.", "Check payees twice."),
            "Check payees twice."
        );
        assert!(check_text("Totals are in cents.").is_ok());
        assert!(check_text("  ").is_err());
        assert!(check_text(&"x".repeat(MAX_LESSON + 1)).is_err());
        assert!(check_text("a\u{1b}[2Jb").is_err());
        // The same words, the same name; spaces around them aside.
        assert_eq!(
            key_of(" Totals are in cents.\n"),
            key_of("Totals are in cents.")
        );
        assert_ne!(key_of("Totals are in cents."), key_of("Totals are in USD."));
        assert!(key_of("x").starts_with(LESSON_PREFIX));
    }
}
