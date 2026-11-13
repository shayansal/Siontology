//! Reported attacks, shared between operators: one agent is attacked, and
//! every agent whose operator trusts the report is protected from the
//! same text.
//!
//! - **A report** is a claim holding a signature (a piece of the text that
//!   attacked an agent, [`sion_agent::immune`]), on the reporter's bond
//!   `attack-reporter`, under an adjudicator who can rule against it: a
//!   signature that marks ordinary content costs its reporter the bond.
//! - **A vouching** is another account's claim, on its own bond
//!   `attack-verifier`, that it checked the signature.
//! - **Trust is the operator's.** A signature is looked for on this
//!   machine only when enough of the accounts its operator named
//!   (`sion agent immune trust`) reported it or vouch for it. With no one
//!   named, nothing is looked for: a list anyone can add to is not one an
//!   agent's reading should depend on.
//! - **Thanks.** When a signature has held content here, the operator can
//!   say so in a claim and pay its reporter a tip (`sion agent immune
//!   thank`). Both are the operator's choice.
//!
//! A signature only ever holds a result back, and only one that came from
//! outside. What is held is counted here ([`Settings::hits`]), so that a
//! signature that holds what it should not is seen, and can be suspended
//! on this machine at once (`sion agent immune suspend`) whatever the
//! ledger says.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use clap::Subcommand;
use serde::{Deserialize, Serialize};
use sion_agent::immune::Signature;
use sion_assurance::AtomClass;
use sion_core::ids::{AccountId, AtomId, ClaimId};
use sion_core::Amount;
use sion_exec::claim::{formulas, ClaimBody, Payload};
use sion_exec::execute::bond_atom;
use sion_exec::TxKind;

use crate::error::CliError;
use crate::feeds::{self, Feed};
use crate::ledger::Ledger;
use crate::session::Session;

/// The schema of a claim reporting an attack's signature.
pub const SIGNATURE_SCHEMA: &str = "agent.attack-signature/v1";

/// The schema of a claim vouching for one.
pub const VOUCH_SCHEMA: &str = "agent.attack-signature.vouch/v1";

/// The schema of a claim saying one held content somewhere.
pub const HELD_SCHEMA: &str = "agent.attack-signature.held/v1";

/// The bond a reporter stands behind its signatures with.
pub const REPORTER_BOND: &str = "attack-reporter";

/// The bond a verifier stands behind its vouchings with.
pub const VERIFIER_BOND: &str = "attack-verifier";

/// What a signature's key begins with.
pub const SIGNATURE_PREFIX: &str = "attack:";

/// What a vouching's key begins with.
pub const VOUCHED_PREFIX: &str = "attack-vouched:";

/// What the key of a claim that one held content begins with.
pub const HELD_PREFIX: &str = "attack-held:";

/// A reported signature, as its reporter publishes it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    /// The text that marks the attack, as the reporter wrote it.
    pub pattern: String,
    /// What kind of attack it is, in a word or two.
    pub kind: String,
    /// What it does and how it was met, in a line.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    /// The digest of the content it was found in, if the reporter keeps
    /// it: the content itself is not published.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample: Option<String>,
    /// The host it was met at, if the reporter says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seen_at: Option<String>,
}

/// A vouching for a signature.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Vouching {
    /// The signature's claim.
    pub signature: String,
    /// What the voucher says of it, in a line.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
}

/// That a signature held content on an operator's machine.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Thanks {
    /// The signature's claim.
    pub signature: String,
    /// How many results it held there since the operator last said.
    pub held: u64,
    /// What the operator paid its reporter, in millionths of a USD.
    #[serde(default)]
    pub paid_micros: u64,
}

/// The key a signature of this text is named under.
pub fn key_of(pattern: &str) -> String {
    let digest = sion_core::hash::hash_domain(
        "sion-cli/attack-signature/v1",
        sion_agent::immune::plain(pattern).as_bytes(),
    );
    format!("{SIGNATURE_PREFIX}{}", feeds::short_key(&digest))
}

/// The key a vouching for the signature `claim` is named under.
pub fn vouched_key(claim: &ClaimId) -> String {
    format!("{VOUCHED_PREFIX}{}", feeds::short_key(claim.as_hash()))
}

/// The key a claim that the signature `claim` held content is named under.
pub fn held_key(claim: &ClaimId) -> String {
    format!("{HELD_PREFIX}{}", feeds::short_key(claim.as_hash()))
}

/// `text` on one line, at most `most` characters.
fn one_line(text: &str, most: usize) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    line.chars().take(most).collect()
}

/// A bonded statement by `issuer` on its bond `bond`, which `adjudicator`
/// can rule against.
fn bonded(
    issuer: &AccountId,
    bond: &str,
    schema: &str,
    subject: String,
    payload: Vec<u8>,
    adjudicator: AtomId,
    valid_for: u64,
) -> ClaimBody {
    ClaimBody {
        schema: schema.into(),
        subject,
        payload: Payload::Public(payload),
        formula: formulas::bonded_statement(bond_atom(AtomClass::Economic, issuer, bond).id),
        adjudicator: Some(adjudicator),
        detection_permille: 1_000,
        valid_for,
        challenge_period: 100,
    }
}

fn written<T: Serialize>(value: &T) -> Result<Vec<u8>, CliError> {
    serde_json::to_vec(value).map_err(|e| CliError::Usage(format!("it could not be written: {e}")))
}

/// Publishes `report` as `wallet`'s claim, on its bond
/// [`REPORTER_BOND`], named under the signature's key.
pub fn report(
    session: &mut Session<'_>,
    wallet: &str,
    report: &Report,
    adjudicator: AtomId,
    valid_for: u64,
) -> Result<ClaimId, CliError> {
    sion_agent::immune::check_pattern(&report.pattern).map_err(CliError::Usage)?;
    let reporter = session.id(wallet);
    let body = bonded(
        &reporter,
        REPORTER_BOND,
        SIGNATURE_SCHEMA,
        format!(
            "content from outside holding this text attacks an agent that reads it ({}): {}",
            one_line(&report.kind, 40),
            one_line(&report.pattern, 60)
        ),
        written(report)?,
        adjudicator,
        valid_for,
    );
    crate::vouching::publish(session, wallet, body, Some(key_of(&report.pattern)))
}

/// Vouches for the signature `claim` as `wallet`, on its bond
/// [`VERIFIER_BOND`].
pub fn vouch(
    session: &mut Session<'_>,
    wallet: &str,
    claim: ClaimId,
    note: &str,
    adjudicator: AtomId,
    valid_for: u64,
) -> Result<ClaimId, CliError> {
    let voucher = session.id(wallet);
    let vouching = Vouching {
        signature: claim.as_hash().to_hex(),
        note: one_line(note, 200),
    };
    let body = bonded(
        &voucher,
        VERIFIER_BOND,
        VOUCH_SCHEMA,
        format!("vouches for attack signature {}", claim.short()),
        written(&vouching)?,
        adjudicator,
        valid_for,
    );
    crate::vouching::publish(session, wallet, body, Some(vouched_key(&claim)))
}

/// A signature found on the ledger, with who stands behind it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    /// Its claim.
    pub claim: ClaimId,
    /// Who reported it.
    pub reporter: AccountId,
    /// Whether its claim stands.
    pub active: bool,
    /// What was reported.
    pub report: Report,
    /// Who vouches for it, on a vouching that stands: each once, the
    /// reporter left out.
    pub vouchers: Vec<AccountId>,
    /// The operators who say it held content on their machines, each once.
    pub thanked: Vec<(AccountId, Thanks)>,
}

impl Found {
    /// What it is called where it is held: the start of its claim.
    pub fn id(&self) -> String {
        self.claim.as_hash().to_hex()[..8].to_string()
    }

    /// The accounts behind it: its reporter and its vouchers.
    pub fn behind(&self) -> impl Iterator<Item = &AccountId> {
        std::iter::once(&self.reporter).chain(self.vouchers.iter())
    }
}

/// What a signature held on this machine.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Held {
    /// Results it held.
    pub held: u64,
    /// Of those, the ones the operator has said thanks for.
    #[serde(default)]
    pub thanked: u64,
}

/// What this machine's operator set, and what was found and held here.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    /// The accounts whose report or vouching makes a signature one to
    /// look for here.
    #[serde(default)]
    pub trust: Vec<AccountId>,
    /// How many of them a signature needs behind it (1, if not set).
    #[serde(default)]
    pub need: usize,
    /// Signatures not looked for here whatever stands behind them, by
    /// claim.
    #[serde(default)]
    pub suspended: BTreeSet<String>,
    /// What each signature held here, by claim.
    #[serde(default)]
    pub hits: BTreeMap<String, Held>,
    /// The signatures looked for here, as of the last time the ledger was
    /// read: by claim, with who reported each.
    #[serde(default)]
    pub active: Vec<(String, AccountId, Signature)>,
}

impl Settings {
    /// How many trusted accounts a signature needs behind it.
    pub fn needed(&self) -> usize {
        self.need.max(1)
    }

    /// How many of the accounts behind `found` are trusted here.
    pub fn trusted(&self, found: &Found) -> usize {
        let behind: BTreeSet<&AccountId> = found.behind().collect();
        behind
            .into_iter()
            .filter(|a| self.trust.contains(a))
            .count()
    }

    /// Whether `found` is looked for here: it stands, is not suspended,
    /// and enough trusted accounts are behind it.
    pub fn looks_for(&self, found: &Found) -> bool {
        found.active
            && !self.suspended.contains(&found.claim.as_hash().to_hex())
            && self.trusted(found) >= self.needed()
    }
}

/// Where this machine's settings are kept.
pub fn settings_path() -> PathBuf {
    crate::model_routes::dir().join("immune.json")
}

fn feed_path() -> PathBuf {
    crate::model_routes::dir().join("immune-feed.json")
}

/// This machine's settings.
pub fn settings() -> Settings {
    std::fs::read_to_string(settings_path())
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn keep(settings: &Settings) -> Result<(), CliError> {
    crate::profiles::write_json(&settings_path(), settings)
}

/// The claim `hex` names.
fn claim_of(hex: &str) -> Option<ClaimId> {
    sion_core::hash::Hash32::from_hex(hex.trim())
        .ok()
        .map(ClaimId::from_hash)
}

/// Brings what is known of reported signatures up to date from the ledger
/// and reads each: the signatures named under the key of their own text by
/// whoever issued them, that can be signatures, with the vouchings and
/// thanks named under their keys by whoever issued those.
pub fn refresh(ledger: &dyn Ledger) -> Result<Vec<Found>, CliError> {
    let path = feed_path();
    let mut feed: Feed = std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    feed.scan(ledger, &[SIGNATURE_PREFIX, VOUCHED_PREFIX, HELD_PREFIX])?;
    crate::profiles::write_json(&path, &feed)?;
    let mut found: Vec<Found> = Vec::new();
    for entry in feed.under(SIGNATURE_PREFIX) {
        let Some(read) = feeds::read::<Report>(ledger, entry.claim, SIGNATURE_SCHEMA)? else {
            continue;
        };
        if read.issuer != entry.owner
            || key_of(&read.says.pattern) != entry.key
            || sion_agent::immune::check_pattern(&read.says.pattern).is_err()
            || found.iter().any(|f| f.claim == entry.claim)
        {
            continue;
        }
        found.push(Found {
            claim: entry.claim,
            reporter: read.issuer,
            active: read.active,
            report: read.says,
            vouchers: Vec::new(),
            thanked: Vec::new(),
        });
    }
    for entry in feed.under(VOUCHED_PREFIX) {
        let Some(read) = feeds::read::<Vouching>(ledger, entry.claim, VOUCH_SCHEMA)? else {
            continue;
        };
        if read.issuer != entry.owner || !read.active {
            continue;
        }
        let Some(signature) = found.iter_mut().find(|f| {
            f.claim.as_hash().to_hex() == read.says.signature && vouched_key(&f.claim) == entry.key
        }) else {
            continue;
        };
        if read.issuer != signature.reporter && !signature.vouchers.contains(&read.issuer) {
            signature.vouchers.push(read.issuer);
        }
    }
    for entry in feed.under(HELD_PREFIX) {
        let Some(read) = feeds::read::<Thanks>(ledger, entry.claim, HELD_SCHEMA)? else {
            continue;
        };
        if read.issuer != entry.owner {
            continue;
        }
        let Some(signature) = found.iter_mut().find(|f| {
            f.claim.as_hash().to_hex() == read.says.signature && held_key(&f.claim) == entry.key
        }) else {
            continue;
        };
        if read.issuer == signature.reporter {
            continue;
        }
        // An operator's latest word replaces its earlier one.
        signature.thanked.retain(|(by, _)| *by != read.issuer);
        signature.thanked.push((read.issuer, read.says));
    }
    Ok(found)
}

/// Reads the ledger and settles which signatures are looked for here.
pub fn sync(ledger: &dyn Ledger) -> Result<(Settings, Vec<Found>), CliError> {
    let found = refresh(ledger)?;
    let mut settings = settings();
    settings.active = found
        .iter()
        .filter(|f| settings.looks_for(f))
        .filter_map(|f| {
            let signature = Signature::new(&f.id(), &f.report.kind, &f.report.pattern).ok()?;
            Some((f.claim.as_hash().to_hex(), f.reporter, signature))
        })
        .collect();
    keep(&settings)?;
    Ok((settings, found))
}

/// The signatures a session on this machine looks for: none unless its
/// operator trusts someone's reports. The ledger is read first, from where
/// the last reading left off; if it cannot be, the signatures kept from
/// the last reading are used, and the error is returned with them.
pub fn for_session(ledger: &dyn Ledger) -> (Vec<Signature>, Option<CliError>) {
    let kept = settings();
    if kept.trust.is_empty() {
        return (Vec::new(), None);
    }
    let (settings, error) = match sync(ledger) {
        Ok((settings, _)) => (settings, None),
        Err(e) => (kept, Some(e)),
    };
    let signatures = settings
        .active
        .into_iter()
        .filter(|(claim, _, _)| !settings.suspended.contains(claim))
        .map(|(_, _, signature)| signature)
        .collect();
    (signatures, error)
}

/// Notes what a session's runtime held, by the signatures' ids.
pub fn note_hits(hits: &[sion_agent::immune::Hit]) -> Result<(), CliError> {
    if hits.is_empty() {
        return Ok(());
    }
    let mut settings = settings();
    for hit in hits {
        let Some((claim, _, _)) = settings
            .active
            .iter()
            .find(|(_, _, signature)| signature.id == hit.signature)
        else {
            continue;
        };
        settings.hits.entry(claim.clone()).or_default().held += 1;
    }
    keep(&settings)
}

/// A line or more for each signature found, with where it stands here.
pub fn lines(settings: &Settings, found: &[Found]) -> Vec<String> {
    if found.is_empty() {
        return vec!["no attack is reported on this ledger yet".to_string()];
    }
    let mut lines = Vec::new();
    for f in found {
        let hex = f.claim.as_hash().to_hex();
        lines.push(format!(
            "signature {} by {} ({}): \"{}\"",
            f.id(),
            f.reporter.short(),
            one_line(&f.report.kind, 40),
            sion_agent::immune::plain(&f.report.pattern)
        ));
        if !f.report.note.is_empty() || f.report.seen_at.is_some() {
            lines.push(format!(
                "  {}{}",
                one_line(&f.report.note, 200),
                match &f.report.seen_at {
                    Some(host) => format!(" (met at {})", one_line(host, 80)),
                    None => String::new(),
                }
            ));
        }
        lines.push(match f.vouchers.as_slice() {
            [] => "  vouched for by no one".to_string(),
            by => format!(
                "  vouched for by {}",
                by.iter().map(|a| a.short()).collect::<Vec<_>>().join(", ")
            ),
        });
        let here = if !f.active {
            "not looked for here: its claim no longer stands".to_string()
        } else if settings.suspended.contains(&hex) {
            "not looked for here: suspended on this machine".to_string()
        } else if settings.trust.is_empty() {
            "not looked for here: you trust no one's reports (sion agent immune trust)".to_string()
        } else {
            let (trusted, needed) = (settings.trusted(f), settings.needed());
            if trusted >= needed {
                format!("looked for here: {trusted} account(s) you trust stand behind it")
            } else {
                format!(
                    "not looked for here: {trusted} of the {needed} account(s) you trust that it \
                     needs stand behind it"
                )
            }
        };
        lines.push(format!("  {here}"));
        if let Some(held) = settings.hits.get(&hex).filter(|h| h.held > 0) {
            lines.push(format!("  held {} result(s) on this machine", held.held));
        }
        if !f.thanked.is_empty() {
            let paid: u64 = f.thanked.iter().map(|(_, t)| t.paid_micros).sum();
            lines.push(format!(
                "  {} operator(s) say it held content for them{}",
                f.thanked.len(),
                match paid {
                    0 => String::new(),
                    paid => format!(", and paid its reporter {} USD", Amount(u128::from(paid))),
                }
            ));
        }
    }
    lines
}

/// Says, as `wallet`, that each signature that held content here since the
/// operator last said so did, and pays its reporter `tip` if it is more
/// than nothing. Returns a line for each.
pub fn thank(
    session: &mut Session<'_>,
    wallet: &str,
    tip: Amount,
) -> Result<Vec<String>, CliError> {
    let mut settings = settings();
    let me = session.id(wallet);
    let mut lines = Vec::new();
    let owed: Vec<(String, AccountId, u64)> = settings
        .hits
        .iter()
        .filter(|(_, held)| held.held > held.thanked)
        .filter_map(|(claim, held)| {
            let (_, reporter, _) = settings.active.iter().find(|(c, _, _)| c == claim)?;
            Some((claim.clone(), *reporter, held.held - held.thanked))
        })
        .collect();
    for (hex, reporter, held) in owed {
        let Some(claim) = claim_of(&hex) else {
            continue;
        };
        let paid = if tip > Amount::ZERO && reporter != me {
            session.must(
                wallet,
                TxKind::Transfer {
                    asset: sion_core::ids::AssetId::usd(),
                    outputs: vec![(reporter, tip)],
                },
                Vec::new(),
            )?;
            tip
        } else {
            Amount::ZERO
        };
        let thanks = Thanks {
            signature: hex.clone(),
            held,
            paid_micros: u64::try_from(paid.0).unwrap_or(u64::MAX),
        };
        let body = ClaimBody {
            schema: HELD_SCHEMA.into(),
            subject: format!("attack signature {} held content here", claim.short()),
            payload: Payload::Public(written(&thanks)?),
            formula: sion_assurance::Formula::and(vec![formulas::dual_signature()]),
            adjudicator: None,
            detection_permille: 0,
            valid_for: 1_000_000,
            challenge_period: 0,
        };
        let said = crate::vouching::publish(session, wallet, body, Some(held_key(&claim)))?;
        if let Some(entry) = settings.hits.get_mut(&hex) {
            entry.thanked = entry.held;
        }
        keep(&settings)?;
        lines.push(format!(
            "signature {}: said it held {held} result(s) here, in claim {}{}",
            &hex[..8],
            said.short(),
            if paid > Amount::ZERO {
                format!("; paid its reporter {} {paid} USD", reporter.short())
            } else {
                String::new()
            }
        ));
    }
    if lines.is_empty() {
        lines.push("no signature has held anything here since you last said so".to_string());
    }
    Ok(lines)
}

/// `sion agent immune`.
#[derive(Clone, Debug, Subcommand)]
pub enum ImmuneCommand {
    /// Report an attack's signature, as you: a piece of the text that
    /// attacked an agent, as a claim on your bond `attack-reporter`, which
    /// the adjudicator can rule against. Agents whose operators trust you,
    /// or someone who vouches for it, hold back content from outside that
    /// holds this text.
    Report {
        /// Your wallet.
        wallet: String,
        /// The text that marks the attack: at least 16 characters and 3
        /// words, at most 300. Case, spacing and invisible characters do
        /// not matter.
        #[arg(long)]
        pattern: String,
        /// What kind of attack it is, in a word or two.
        #[arg(long, default_value = "prompt injection")]
        kind: String,
        /// What it does and how it was met, in a line.
        #[arg(long, default_value = "")]
        note: String,
        /// The content it was found in: checked to hold the text, and its
        /// digest published (never the content).
        #[arg(long, value_name = "FILE")]
        sample: Option<PathBuf>,
        /// The host it was met at.
        #[arg(long = "seen-at", value_name = "HOST")]
        seen_at: Option<String>,
        /// The adjudicator who can rule against the report (its atom's id,
        /// 64 hex digits).
        #[arg(long)]
        adjudicator: String,
        /// Post your bond `attack-reporter` with this many USD first.
        #[arg(long, value_name = "USD")]
        bond: Option<String>,
        /// How many Pulses the report stands for.
        #[arg(long = "valid-for", default_value_t = 1_000_000)]
        valid_for: u64,
    },
    /// Vouch for a reported signature, as you: a claim on your bond
    /// `attack-verifier` that you checked it marks an attack and not
    /// ordinary content.
    Vouch {
        /// Your wallet.
        wallet: String,
        /// The signature (its claim's id, or a unique prefix of it).
        signature: String,
        /// The adjudicator who can rule against your vouching (its atom's
        /// id, 64 hex digits).
        #[arg(long)]
        adjudicator: String,
        /// Post your bond `attack-verifier` with this many USD first.
        #[arg(long, value_name = "USD")]
        bond: Option<String>,
        /// What you say of it, in a line.
        #[arg(long, default_value = "")]
        note: String,
        /// How many Pulses the vouching stands for.
        #[arg(long = "valid-for", default_value_t = 1_000_000)]
        valid_for: u64,
    },
    /// Name the accounts whose report or vouching makes a signature one
    /// your agents look for. With none named, nothing is looked for.
    Trust {
        /// The accounts (a wallet's name or an account's id).
        accounts: Vec<String>,
        /// How many of the accounts you trust a signature needs behind it.
        #[arg(long)]
        need: Option<usize>,
        /// Stop trusting these accounts instead.
        #[arg(long)]
        remove: bool,
    },
    /// The signatures reported on this ledger: who reported and vouches for
    /// each, whether your agents look for it, and what it held here.
    List,
    /// Stop looking for a signature on this machine, whatever stands behind
    /// it: for one that holds what it should not.
    Suspend {
        /// The signature (its claim's id, or a unique prefix of it).
        signature: String,
    },
    /// Look for a suspended signature again.
    Resume {
        /// The signature (its claim's id, or a unique prefix of it).
        signature: String,
    },
    /// Say, as you, that the signatures that held content here did, and
    /// pay each one's reporter a tip if you name one.
    Thank {
        /// Your wallet.
        wallet: String,
        /// What to pay each such signature's reporter, in USD.
        #[arg(long, value_name = "USD", default_value = "0")]
        tip: String,
    },
}

fn atom(hex: &str) -> Result<AtomId, CliError> {
    sion_core::hash::Hash32::from_hex(hex.trim())
        .map(AtomId)
        .map_err(|e| CliError::Usage(format!("the adjudicator's id: {e}")))
}

fn post_bond(
    session: &mut Session<'_>,
    wallet: &str,
    label: &str,
    amount: &str,
) -> Result<(), CliError> {
    let view = session.must(
        wallet,
        TxKind::PostBond {
            class: AtomClass::Economic,
            label: label.into(),
            amount: crate::agents::parse_amount(amount)?,
        },
        Vec::new(),
    )?;
    crate::agents::print_receipt(session, &view);
    Ok(())
}

/// The one signature among `found` whose claim starts with `prefix`.
fn one<'a>(found: &'a [Found], prefix: &str) -> Result<&'a Found, CliError> {
    let fit: Vec<&Found> = found
        .iter()
        .filter(|f| f.claim.as_hash().to_hex().starts_with(prefix.trim()))
        .collect();
    match fit.as_slice() {
        [one] => Ok(*one),
        [] => Err(CliError::Usage(format!(
            "no signature {prefix} is reported on this ledger (sion agent immune list)"
        ))),
        _ => Err(CliError::Usage(format!(
            "{prefix} names more than one signature"
        ))),
    }
}

/// Runs an immune command.
pub fn run(
    session: &mut Session<'_>,
    ledger: &dyn Ledger,
    command: ImmuneCommand,
) -> Result<(), CliError> {
    match command {
        ImmuneCommand::Report {
            wallet,
            pattern,
            kind,
            note,
            sample,
            seen_at,
            adjudicator,
            bond,
            valid_for,
        } => {
            let plain = sion_agent::immune::check_pattern(&pattern).map_err(CliError::Usage)?;
            let sample = match sample {
                Some(path) => {
                    let bytes = std::fs::read(&path)
                        .map_err(|e| CliError::Usage(format!("{}: {e}", path.display())))?;
                    let text = String::from_utf8_lossy(&bytes);
                    if !sion_agent::immune::plain(&text).contains(&plain) {
                        return Err(CliError::Usage(format!(
                            "{} does not hold the text: a signature is text the attack holds",
                            path.display()
                        )));
                    }
                    Some(sion_core::hash::hash_domain("sion-cli/attack-sample/v1", &bytes).to_hex())
                }
                None => None,
            };
            let adjudicator = atom(&adjudicator)?;
            if let Some(amount) = bond {
                post_bond(session, &wallet, REPORTER_BOND, &amount)?;
            }
            let reported = Report {
                pattern: one_line(&pattern, sion_agent::immune::MAX_PATTERN * 2),
                kind: one_line(&kind, 40),
                note: one_line(&note, 200),
                sample,
                seen_at: seen_at.map(|host| one_line(&host, 80)),
            };
            let claim = report(session, &wallet, &reported, adjudicator, valid_for)?;
            println!(
                "reported signature {}: claim {}, on your bond {REPORTER_BOND}",
                &claim.as_hash().to_hex()[..8],
                claim.as_hash().to_hex()
            );
            println!("  \"{plain}\"");
            Ok(())
        }
        ImmuneCommand::Vouch {
            wallet,
            signature,
            adjudicator,
            bond,
            note,
            valid_for,
        } => {
            let found = refresh(ledger)?;
            let signature = one(&found, &signature)?;
            if signature.reporter == session.id(&wallet) {
                return Err(CliError::Usage(
                    "a signature's reporter stands behind it already: someone else vouches".into(),
                ));
            }
            let adjudicator = atom(&adjudicator)?;
            if let Some(amount) = bond {
                post_bond(session, &wallet, VERIFIER_BOND, &amount)?;
            }
            let claim = vouch(
                session,
                &wallet,
                signature.claim,
                &note,
                adjudicator,
                valid_for,
            )?;
            println!(
                "vouched for signature {}: claim {}, on your bond {VERIFIER_BOND}",
                signature.id(),
                claim.as_hash().to_hex()
            );
            Ok(())
        }
        ImmuneCommand::Trust {
            accounts,
            need,
            remove,
        } => {
            let mut settings = settings();
            for name in &accounts {
                let account = session.id(name);
                settings.trust.retain(|a| *a != account);
                if !remove {
                    settings.trust.push(account);
                }
            }
            if let Some(need) = need {
                settings.need = need.max(1);
            }
            keep(&settings)?;
            let (settings, _) = sync(ledger)?;
            match settings.trust.as_slice() {
                [] => println!("you trust no one's reports: your agents look for no signature"),
                trusted => println!(
                    "your agents look for a signature that {} of these stand behind: {}",
                    settings.needed(),
                    trusted
                        .iter()
                        .map(|a| a.short())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            }
            println!("{} signature(s) looked for now", settings.active.len());
            Ok(())
        }
        ImmuneCommand::List => {
            let (settings, found) = sync(ledger)?;
            for line in lines(&settings, &found) {
                println!("{line}");
            }
            Ok(())
        }
        ImmuneCommand::Suspend { signature } | ImmuneCommand::Resume { signature }
            if signature.trim().is_empty() =>
        {
            Err(CliError::Usage("name the signature".into()))
        }
        ImmuneCommand::Suspend { signature } => {
            let found = refresh(ledger)?;
            let hex = one(&found, &signature)?.claim.as_hash().to_hex();
            let mut settings = settings();
            settings.suspended.insert(hex.clone());
            keep(&settings)?;
            sync(ledger)?;
            println!("signature {} is not looked for on this machine", &hex[..8]);
            Ok(())
        }
        ImmuneCommand::Resume { signature } => {
            let found = refresh(ledger)?;
            let hex = one(&found, &signature)?.claim.as_hash().to_hex();
            let mut settings = settings();
            settings.suspended.remove(&hex);
            keep(&settings)?;
            let (settings, _) = sync(ledger)?;
            println!(
                "signature {} is {} on this machine",
                &hex[..8],
                if settings.active.iter().any(|(c, _, _)| *c == hex) {
                    "looked for again"
                } else {
                    "no longer suspended, and not looked for: not enough accounts you trust \
                     stand behind it"
                }
            );
            Ok(())
        }
        ImmuneCommand::Thank { wallet, tip } => {
            sync(ledger)?;
            for line in thank(session, &wallet, crate::agents::parse_amount(&tip)?)? {
                println!("{line}");
            }
            Ok(())
        }
    }
}
