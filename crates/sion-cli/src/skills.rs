//! Skills with a supply chain, on today's primitives: releases and audits
//! are claims on bonds, and installing a skill is accepting its release
//! under the decision rule, with the value at risk set to what the agent
//! could lose.
//!
//! - **A bundle** is a skill directory's canonical bytes
//!   ([`sion_agent::skills::bundle`]); its root is the Merkle root a storage
//!   deal over those bytes would commit to, so any copy can be checked and
//!   a deal can keep it.
//! - **An audit** (`skill.audit/v1`) is an auditor's claim, on its own
//!   bond, that a bundle root does what its description says and nothing
//!   else.
//! - **A release** (`skill/v1`) is the publisher's claim naming the skill,
//!   its version, its bundle root and the release it follows. Its formula
//!   is the publisher's dual signature and **its bond or any of the
//!   audits** of that root: an *or* of independent backings adds up, so
//!   forging a release means corrupting the publisher and every auditor.
//! - **Installing** is `Accept` of the release with the value at risk set
//!   to the agent's remaining budget: it goes through only if the backing
//!   is at least κ times that, and reserves it. A skill that turns out to be
//!   malicious is challenged like any claim; when the adjudicator upholds
//!   it, the bonds behind it are slashed and the agents that relied on it
//!   are compensated from the reserved backing.

use std::path::{Path, PathBuf};

use clap::Subcommand;
use serde_json::{json, Value};
use sion_assurance::{AtomClass, Formula, Policy, Ratio};
use sion_core::hash::Hash32;
use sion_core::ids::{AtomId, ClaimId, ObjectId, Pulse};
use sion_core::Amount;
use sion_exec::claim::{claim_id, formulas, ClaimBody, Payload};
use sion_exec::execute::bond_atom;
use sion_exec::object::ObjectData;
use sion_exec::storage::DataTree;
use sion_exec::{ExecStatus, TxKind};

use crate::error::CliError;
use crate::ledger::{Ledger, LedgerExt};
use crate::session::{wallet_dir, Session};

/// The schema of a release.
pub const RELEASE: &str = "skill/v1";
/// The schema of an audit.
pub const AUDIT: &str = "skill.audit/v1";
/// The file in an installed skill that records where it came from.
pub const SOURCE: &str = sion_agent::skills::SOURCE;

/// `sion skill …`
#[derive(Debug, Subcommand)]
pub enum SkillCommand {
    /// Print a skill directory's bundle root, the commitment releases and
    /// audits name, and write its bundle file.
    Pack {
        /// The skill directory (with a SKILL.md).
        dir: PathBuf,
        /// Where to write the bundle (default: the local bundle store).
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Audit a bundle: claim, on your bond, that it does what its
    /// description says and nothing else.
    Audit {
        /// The auditor's wallet.
        wallet: String,
        /// The bundle root (64 hex digits).
        bundle: String,
        /// The label of your bond the audit rests on.
        #[arg(long)]
        bond: String,
        /// The adjudicator atom that rules on challenges.
        #[arg(long)]
        adjudicator: String,
        /// What you checked.
        #[arg(long, default_value = "")]
        note: String,
        /// Validity in Pulses.
        #[arg(long, default_value_t = 100_000)]
        valid_for: u64,
    },
    /// Publish a release of a skill: a claim naming it, its version and its
    /// bundle root, on your bond or the audits of that root, or both.
    Publish {
        /// The publisher's wallet.
        wallet: String,
        /// The skill directory (with a SKILL.md).
        dir: PathBuf,
        /// The version, for example 1.0.0.
        #[arg(long)]
        version: String,
        /// The label of your bond the release rests on.
        #[arg(long)]
        bond: Option<String>,
        /// Audits of the bundle the release rests on (claim ids).
        #[arg(long, value_delimiter = ',')]
        audits: Vec<String>,
        /// The adjudicator atom that rules on challenges.
        #[arg(long)]
        adjudicator: String,
        /// The release this one follows (its claim id).
        #[arg(long)]
        previous: Option<String>,
        /// Where installers can fetch the bundle (an https URL).
        #[arg(long)]
        locator: Option<String>,
        /// Validity in Pulses.
        #[arg(long, default_value_t = 100_000)]
        valid_for: u64,
    },
    /// A release: what it names, the backing behind it, and the decision
    /// rule for a value at risk.
    Show {
        /// The release (its claim id).
        release: String,
        /// The value at risk to decide for (USD).
        #[arg(long, default_value = "100")]
        value: String,
    },
    /// Install a release for an agent: fetch and check its bundle, accept
    /// the release with the agent's remaining budget at risk, and only then
    /// put the skill in the agent's skills.
    Install {
        /// The agent's wallet.
        wallet: String,
        /// The agent (its id).
        agent: String,
        /// The release (its claim id).
        release: String,
        /// The bundle file (default: the local bundle store, then the
        /// release's locator).
        #[arg(long)]
        bundle: Option<PathBuf>,
        /// The value at risk (USD; default: the agent's remaining budget).
        #[arg(long)]
        value: Option<String>,
        /// κ, the margin the backing must have over the value at risk.
        #[arg(long, default_value_t = 2.0)]
        kappa: f64,
        /// How long the agent relies on it, in Pulses.
        #[arg(long, default_value_t = 10_000)]
        pulses: u64,
        /// The directory of the agent's skills (default:
        /// $SION_HOME/agents/skills/<agent>).
        #[arg(long)]
        skills: Option<PathBuf>,
    },
}

/// Where bundles are kept locally: `$SION_HOME/skills/bundles`.
pub fn bundles_dir() -> PathBuf {
    wallet_dir()
        .parent()
        .map(|home| home.join("skills").join("bundles"))
        .unwrap_or_else(|| PathBuf::from(".sion/skills/bundles"))
}

/// A bundle's root: the Merkle root a storage deal over its bytes commits
/// to.
pub fn bundle_root(bytes: &[u8]) -> Hash32 {
    DataTree::of_data(bytes).root()
}

/// Packs a skill directory: its bundle, root, name, description.
pub fn pack(dir: &Path) -> Result<(Vec<u8>, Hash32, String, String), CliError> {
    let bytes = sion_agent::skills::bundle(dir).map_err(CliError::Usage)?;
    let files = sion_agent::skills::unbundle(&bytes).map_err(CliError::Usage)?;
    let skill_md = files
        .iter()
        .find(|(p, _)| p == "SKILL.md")
        .map(|(_, c)| String::from_utf8_lossy(c).into_owned())
        .unwrap_or_default();
    let (name, description, _) = sion_agent::skills::parse(&skill_md).map_err(CliError::Usage)?;
    Ok((bytes.clone(), bundle_root(&bytes), name, description))
}

fn store_bundle(bytes: &[u8], root: &Hash32) -> Result<PathBuf, CliError> {
    let dir = bundles_dir();
    std::fs::create_dir_all(&dir).map_err(|e| CliError::Usage(e.to_string()))?;
    let path = dir.join(format!("{}.skill", root.to_hex()));
    std::fs::write(&path, bytes)
        .map_err(|e| CliError::Usage(format!("{}: {e}", path.display())))?;
    Ok(path)
}

fn parse_hash(text: &str, what: &str) -> Result<Hash32, CliError> {
    Hash32::from_hex(text.trim())
        .map_err(|_| CliError::Usage(format!("{what} must be 64 hex digits")))
}

fn one_line(text: &str, max: usize) -> String {
    let line: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() > max {
        format!("{}…", line.chars().take(max).collect::<String>())
    } else {
        line
    }
}

/// What a release names, read from its payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    /// The release's claim.
    pub claim: ClaimId,
    /// Its publisher.
    pub publisher: sion_core::ids::AccountId,
    /// The skill's name.
    pub name: String,
    /// Its version.
    pub version: String,
    /// The bundle root.
    pub bundle: Hash32,
    /// The bundle's size in bytes.
    pub size: u64,
    /// The release it follows.
    pub previous: Option<ClaimId>,
    /// Where to fetch the bundle.
    pub locator: Option<String>,
    /// The audits it rests on.
    pub audits: Vec<ClaimId>,
}

/// Reads a release from the ledger.
pub fn release(ledger: &dyn Ledger, id: &str) -> Result<Release, CliError> {
    let stored = crate::tasks::stored_claim(ledger, id)?;
    if stored.body.schema != RELEASE {
        return Err(CliError::Usage(format!(
            "claim {} is a {}, not a skill release",
            stored.id.short(),
            stored.body.schema
        )));
    }
    let Payload::Public(bytes) = &stored.body.payload else {
        return Err(CliError::Usage("a release's payload is public".into()));
    };
    let payload: Value = serde_json::from_slice(bytes)
        .map_err(|e| CliError::Protocol(format!("release payload: {e}")))?;
    let text = |key: &str| payload[key].as_str().unwrap_or_default().to_string();
    let bundle = parse_hash(&text("bundle"), "the release's bundle")?;
    let previous = payload["previous"]
        .as_str()
        .map(|p| parse_hash(p, "previous").map(ClaimId::from_hash))
        .transpose()?;
    let mut audits = Vec::new();
    collect_claims(&stored.body.formula, &mut audits);
    Ok(Release {
        claim: stored.id,
        publisher: stored.issuer,
        name: text("name"),
        version: text("version"),
        bundle,
        size: payload["size"].as_u64().unwrap_or(0),
        previous,
        locator: payload["locator"].as_str().map(str::to_string),
        audits,
    })
}

fn collect_claims(formula: &Formula, out: &mut Vec<ClaimId>) {
    match formula {
        Formula::Claim(id) => out.push(*id),
        Formula::And(parts) | Formula::Or(parts) => {
            parts.iter().for_each(|p| collect_claims(p, out))
        }
        _ => {}
    }
}

fn adjudicator(text: &str) -> Result<AtomId, CliError> {
    Ok(AtomId::from_hash(parse_hash(text, "the adjudicator")?))
}

/// Issues an audit of `bundle` on the auditor's bond; returns its id.
pub fn audit(
    session: &mut Session<'_>,
    wallet: &str,
    bundle: Hash32,
    bond: &str,
    adjudicator_atom: AtomId,
    note: &str,
    valid_for: u64,
) -> Result<(ClaimId, sion_node::TxView), CliError> {
    let auditor = session.id(wallet);
    let payload = json!({"bundle": bundle.to_hex(), "note": note});
    let body = ClaimBody {
        schema: AUDIT.into(),
        subject: format!(
            "Skill bundle {} does what its description says and nothing beyond it",
            bundle.short()
        ),
        payload: Payload::Public(payload.to_string().into_bytes()),
        formula: formulas::bonded_statement(bond_atom(AtomClass::Economic, &auditor, bond).id),
        adjudicator: Some(adjudicator_atom),
        detection_permille: 1000,
        valid_for,
        challenge_period: 100,
    };
    let id = claim_id(&auditor, &body);
    let view = session.send(wallet, TxKind::IssueClaim { claim: body }, Vec::new())?;
    Ok((id, view))
}

/// Publishes a release of the skill in `dir`; returns its id and the bundle
/// root.
#[allow(clippy::too_many_arguments)]
pub fn publish(
    session: &mut Session<'_>,
    ledger: &dyn Ledger,
    wallet: &str,
    dir: &Path,
    version: &str,
    bond: Option<&str>,
    audits: &[ClaimId],
    adjudicator_atom: AtomId,
    previous: Option<ClaimId>,
    locator: Option<&str>,
    valid_for: u64,
) -> Result<(ClaimId, Hash32, sion_node::TxView), CliError> {
    let (bytes, root, name, description) = pack(dir)?;
    store_bundle(&bytes, &root)?;
    let publisher = session.id(wallet);
    // Each audit must be an audit of this very bundle.
    for id in audits {
        let stored = crate::tasks::stored_claim(ledger, &id.to_string())?;
        let ok = stored.body.schema == AUDIT
            && matches!(&stored.body.payload, Payload::Public(p)
                if serde_json::from_slice::<Value>(p).ok().and_then(|v| v["bundle"].as_str().map(str::to_string)) == Some(root.to_hex()));
        if !ok {
            return Err(CliError::Usage(format!(
                "claim {} is not an audit of bundle {}",
                id.short(),
                root.short()
            )));
        }
    }
    let mut backing: Vec<Formula> = Vec::new();
    if let Some(label) = bond {
        backing.push(Formula::atom(
            bond_atom(AtomClass::Economic, &publisher, label).id,
        ));
    }
    backing.extend(audits.iter().map(|a| Formula::Claim(*a)));
    if backing.is_empty() {
        return Err(CliError::Usage(
            "a release rests on your bond, on audits, or both: give --bond or --audits".into(),
        ));
    }
    let formula = Formula::and(vec![formulas::dual_signature(), Formula::or(backing)]);
    let payload = json!({
        "name": name,
        "version": version,
        "bundle": root.to_hex(),
        "size": bytes.len(),
        "previous": previous.map(|p| p.to_string()),
        "locator": locator,
    });
    let body = ClaimBody {
        schema: RELEASE.into(),
        subject: one_line(&format!("Skill {name} {version}: {description}"), 200),
        payload: Payload::Public(payload.to_string().into_bytes()),
        formula,
        adjudicator: Some(adjudicator_atom),
        detection_permille: 1000,
        valid_for,
        challenge_period: 100,
    };
    let id = claim_id(&publisher, &body);
    let view = session.send(wallet, TxKind::IssueClaim { claim: body }, Vec::new())?;
    Ok((id, root, view))
}

/// The bytes of a release's bundle: from `file`, the local store, or the
/// release's locator, checked against its root.
pub fn fetch_bundle(release: &Release, file: Option<&Path>) -> Result<Vec<u8>, CliError> {
    let cached = bundles_dir().join(format!("{}.skill", release.bundle.to_hex()));
    let bytes = if let Some(path) = file {
        std::fs::read(path).map_err(|e| CliError::Usage(format!("{}: {e}", path.display())))?
    } else if cached.exists() {
        std::fs::read(&cached).map_err(|e| CliError::Usage(e.to_string()))?
    } else if let Some(locator) = &release.locator {
        let client = sion_http::Client::new(sion_http::Options::default())
            .map_err(|e| CliError::Usage(e.to_string()))?;
        client
            .get(locator, sion_agent::skills::MAX_BUNDLE as u64)
            .map_err(|e| CliError::Connection(format!("{locator}: {e}")))?
            .body
    } else {
        return Err(CliError::Usage(format!(
            "no copy of bundle {}: give --bundle",
            release.bundle.short()
        )));
    };
    if bundle_root(&bytes) != release.bundle {
        return Err(CliError::Verification(format!(
            "the bundle does not hash to the release's root {}",
            release.bundle.short()
        )));
    }
    Ok(bytes)
}

/// The agent's remaining budget.
pub fn budget(ledger: &dyn Ledger, agent: ObjectId) -> Result<Amount, CliError> {
    let charter = crate::agents::identity(ledger, agent)?;
    let mut total = 0u128;
    for id in &charter.allotments {
        if let Some(object) = ledger.object(*id)? {
            if let ObjectData::Allotment(a) = object.data {
                total += a.remaining.0;
            }
        }
    }
    Ok(Amount(total))
}

/// Writes a checked bundle's files into `skills/<name>`, keeping the
/// version it replaces, and records where they came from.
pub fn put_in_place(
    skills: &Path,
    name: &str,
    files: &[(String, Vec<u8>)],
    source: &Value,
) -> Result<PathBuf, CliError> {
    let dir = skills.join(name);
    let io = |e: std::io::Error| CliError::Usage(format!("{}: {e}", dir.display()));
    if dir.exists() {
        let history = dir.join(sion_agent::skills::HISTORY);
        std::fs::create_dir_all(&history).map_err(io)?;
        let version = std::fs::read_dir(&history).map_err(io)?.count() + 1;
        if let Ok(old) = std::fs::read(dir.join("SKILL.md")) {
            std::fs::write(history.join(format!("{version}.md")), old).map_err(io)?;
        }
        // Remove the old files (not the history) before writing the new.
        for entry in std::fs::read_dir(&dir).map_err(io)?.flatten() {
            if entry.file_name() != sion_agent::skills::HISTORY {
                let path = entry.path();
                if path.is_dir() {
                    std::fs::remove_dir_all(&path).map_err(io)?;
                } else {
                    std::fs::remove_file(&path).map_err(io)?;
                }
            }
        }
    }
    for (relative, content) in files {
        let path = dir.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(io)?;
        }
        std::fs::write(&path, content).map_err(io)?;
    }
    std::fs::write(
        dir.join(SOURCE),
        serde_json::to_string_pretty(source).unwrap_or_default(),
    )
    .map_err(io)?;
    Ok(dir)
}

/// Runs a skill command.
pub fn run(
    session: &mut Session<'_>,
    ledger: &dyn Ledger,
    command: SkillCommand,
) -> Result<(), CliError> {
    match command {
        SkillCommand::Pack { dir, out } => {
            let (bytes, root, name, _) = pack(&dir)?;
            let path = match out {
                Some(path) => {
                    std::fs::write(&path, &bytes).map_err(|e| CliError::Usage(e.to_string()))?;
                    path
                }
                None => store_bundle(&bytes, &root)?,
            };
            println!(
                "skill {name}: bundle {root} ({} bytes) at {}",
                bytes.len(),
                path.display()
            );
        }
        SkillCommand::Audit {
            wallet,
            bundle,
            bond,
            adjudicator: judge,
            note,
            valid_for,
        } => {
            let (id, view) = audit(
                session,
                &wallet,
                parse_hash(&bundle, "the bundle")?,
                &bond,
                adjudicator(&judge)?,
                &note,
                valid_for,
            )?;
            crate::agents::print_receipt(session, &view);
            println!("audit {id}");
        }
        SkillCommand::Publish {
            wallet,
            dir,
            version,
            bond,
            audits,
            adjudicator: judge,
            previous,
            locator,
            valid_for,
        } => {
            let audits: Vec<ClaimId> = audits
                .iter()
                .map(|a| parse_hash(a, "an audit").map(ClaimId::from_hash))
                .collect::<Result<_, _>>()?;
            let previous = previous
                .map(|p| parse_hash(&p, "the previous release").map(ClaimId::from_hash))
                .transpose()?;
            let (id, root, view) = publish(
                session,
                ledger,
                &wallet,
                &dir,
                &version,
                bond.as_deref(),
                &audits,
                adjudicator(&judge)?,
                previous,
                locator.as_deref(),
                valid_for,
            )?;
            crate::agents::print_receipt(session, &view);
            println!("release {id}\nbundle {root}");
        }
        SkillCommand::Show { release: id, value } => {
            let release = release(ledger, &id)?;
            let decision = ledger.decide(
                release.claim,
                crate::agents::parse_amount(&value)?,
                Policy::standard(),
            )?;
            let out = json!({
                "release": release.claim,
                "name": release.name,
                "version": release.version,
                "bundle": release.bundle,
                "size": release.size,
                "previous": release.previous,
                "locator": release.locator,
                "audits": release.audits,
                "publisher": release.publisher,
                "decision": {
                    "value_at_risk": value,
                    "accept": decision["accept"],
                    "reasons": decision["reasons"],
                },
            });
            println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
        }
        SkillCommand::Install {
            wallet,
            agent,
            release: id,
            bundle,
            value,
            kappa,
            pulses,
            skills,
        } => {
            let agent = ObjectId::from_hash(parse_hash(&agent, "the agent")?);
            let lines = install(
                session,
                ledger,
                &wallet,
                agent,
                &id,
                bundle.as_deref(),
                value.as_deref(),
                kappa,
                pulses,
                skills,
            )?;
            for line in lines {
                println!("{line}");
            }
        }
    }
    Ok(())
}

/// Installs a release for an agent; returns what to print.
#[allow(clippy::too_many_arguments)]
pub fn install(
    session: &mut Session<'_>,
    ledger: &dyn Ledger,
    wallet: &str,
    agent: ObjectId,
    id: &str,
    bundle: Option<&Path>,
    value: Option<&str>,
    kappa: f64,
    pulses: u64,
    skills: Option<PathBuf>,
) -> Result<Vec<String>, CliError> {
    let charter = crate::agents::identity(ledger, agent)?;
    if session.id(wallet) != charter.agent {
        return Err(CliError::Usage(format!(
            "{wallet} is not agent {}'s account",
            agent.short()
        )));
    }
    let release = release(ledger, id)?;
    let bytes = fetch_bundle(&release, bundle)?;
    let files = sion_agent::skills::unbundle(&bytes).map_err(CliError::Usage)?;
    let skill_md = files
        .iter()
        .find(|(p, _)| p == "SKILL.md")
        .map(|(_, c)| String::from_utf8_lossy(c).into_owned())
        .unwrap_or_default();
    let (name, _, _) = sion_agent::skills::parse(&skill_md).map_err(CliError::Usage)?;
    if name != release.name {
        return Err(CliError::Verification(format!(
            "the bundle holds skill {name}, the release names {}",
            release.name
        )));
    }
    let value_at_risk = match value {
        Some(text) => crate::agents::parse_amount(text)?,
        None => budget(ledger, agent)?,
    };
    let policy =
        Policy::standard().with_kappa(Ratio::new((kappa * 1000.0).round().max(1.0) as u64, 1000));
    let mut lines = vec![format!(
        "bundle {} checked: {} files, skill {} {}",
        release.bundle.short(),
        files.len(),
        release.name,
        release.version
    )];
    // Ask first, for free: a refusal costs no fee.
    let decision = ledger.decide(release.claim, value_at_risk, policy.clone())?;
    if decision["accept"] != true {
        let reasons: Vec<String> = decision["reasons"]
            .as_array()
            .map(|r| {
                r.iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        return Err(CliError::Verification(format!(
            "not installed: the backing behind release {} does not cover {value_at_risk} USD at risk: {}",
            release.claim.short(),
            reasons.join("; ")
        )));
    }
    let until = Pulse(ledger.pulse()?.0 + pulses);
    let mut nonce = [0u8; 8];
    rand_core::RngCore::fill_bytes(&mut rand_core::OsRng, &mut nonce);
    let view = session.send(
        wallet,
        TxKind::Accept {
            claim: release.claim,
            value_at_risk,
            policy,
            until,
            nonce: u64::from_be_bytes(nonce),
        },
        Vec::new(),
    )?;
    lines.push(format!(
        "Pulse {}: {}",
        view.pulse.0,
        crate::describe::receipt(&session.names(), &view.effects)
    ));
    if view.status != ExecStatus::Success {
        return Err(CliError::Transaction(format!(
            "not installed: the ledger did not accept release {}",
            release.claim.short()
        )));
    }
    let root = skills.unwrap_or_else(|| crate::runtime::skills_dir(&agent));
    let source = json!({
        "release": release.claim,
        "name": release.name,
        "version": release.version,
        "bundle": release.bundle,
        "publisher": release.publisher,
        "audits": release.audits,
        "value_at_risk": value_at_risk.to_string(),
        "accepted_at": view.pulse.0,
        "relied_on_until": until.0,
    });
    let dir = put_in_place(&root, &name, &files, &source)?;
    lines.push(format!(
        "installed {} {} in {}",
        release.name,
        release.version,
        dir.display()
    ));
    Ok(lines)
}
