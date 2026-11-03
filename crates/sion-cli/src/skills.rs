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
        /// Results of settled bounties for this version (claim ids): the
        /// release rests on them too, and records them.
        #[arg(long, value_delimiter = ',')]
        evidence: Vec<String>,
        /// The release this one follows (its claim id).
        #[arg(long)]
        previous: Option<String>,
        /// Where installers can fetch the bundle (an https URL).
        #[arg(long)]
        locator: Option<String>,
        /// Validity in Pulses.
        #[arg(long, default_value_t = 100_000)]
        valid_for: u64,
        /// Do not name the release as your latest in your index (spec v1.15).
        #[arg(long)]
        no_index: bool,
    },
    /// A publisher's latest release of a skill, from its index on the
    /// ledger (spec v1.15), proven against a certified header.
    Latest {
        /// The publisher (wallet name or account) and the skill, as
        /// `publisher/name`.
        skill: String,
        /// The chain identity the proof must verify against. Without it the
        /// node vouches for itself.
        #[arg(long, env = "SION_CHAIN")]
        chain: Option<String>,
        /// A certified Pulse you know of from elsewhere: a proof from more
        /// than 16 Pulses before it is refused as stale.
        #[arg(long)]
        min_pulse: Option<u64>,
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
        /// The release: its claim id, or `publisher/name` for the latest
        /// release in the publisher's index, proven against a certified
        /// header.
        release: String,
        /// With `publisher/name`: the chain identity the index's proof must
        /// verify against. Without it the node vouches for itself.
        #[arg(long, env = "SION_CHAIN")]
        chain: Option<String>,
        /// With `publisher/name`: a certified Pulse you know of from
        /// elsewhere; an index proof from more than 16 Pulses before it is
        /// refused as stale.
        #[arg(long)]
        min_pulse: Option<u64>,
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
        /// Grant the permissions the skill declares (calls it pre-approves,
        /// where its scripts may connect, the secrets they receive), shown
        /// before anything is signed. Without it, a skill that declares
        /// any is not installed.
        #[arg(long)]
        accept_permissions: bool,
    },
    /// Run an evaluation set on a skill: each case with stand-in tools,
    /// then its checks. Needs no node.
    Eval {
        /// The skill directory (with a SKILL.md).
        dir: PathBuf,
        /// The evaluation set (a JSON file).
        #[arg(long)]
        evals: PathBuf,
        /// The agent's model: anthropic:<model>, openai:<model>,
        /// gemini:<model>, local:<model> or script:<file>. Repeat for
        /// fallbacks, in order.
        #[arg(long = "model", required = true)]
        models: Vec<String>,
        /// Write each run as a trajectory (JSON Lines in chat format),
        /// labelled with its result.
        #[arg(long)]
        trajectories: Option<PathBuf>,
    },
    /// Evolve a skill against an evaluation set: a model proposes variants
    /// (new instructions, and other files but never scripts) for the cases
    /// it fails, each is scored under gates, and the best version found is
    /// kept as a draft. Needs no node.
    Evolve {
        /// The skill directory (with a SKILL.md).
        dir: PathBuf,
        /// The evaluation set (a JSON file).
        #[arg(long)]
        evals: PathBuf,
        /// The agent's model, as for `eval`. Repeat for fallbacks.
        #[arg(long = "model", required = true)]
        models: Vec<String>,
        /// The model that proposes variants.
        #[arg(long)]
        mutator: String,
        /// Generations, at most.
        #[arg(long, default_value_t = 3)]
        generations: usize,
        /// The largest SKILL.md a version may have, in bytes.
        #[arg(long, default_value_t = sion_agent::evolve::MAX_BYTES)]
        max_bytes: usize,
        /// An evaluation set of the same skill whose cases the evolution
        /// never sees: the best version is kept as a draft only if it
        /// passes every one of them that the version given passes.
        #[arg(long, value_name = "FILE")]
        held_out: Option<PathBuf>,
    },
    /// Vouch for an evaluation set on your bond: claim that it has no known
    /// way to be passed without doing what the skill describes.
    Vouch {
        /// Your wallet.
        wallet: String,
        /// The evaluation set (a JSON file).
        #[arg(long)]
        evals: PathBuf,
        /// The label of your bond the claim rests on.
        #[arg(long)]
        bond: String,
        /// The adjudicator atom that rules on challenges.
        #[arg(long)]
        adjudicator: String,
        /// Validity in Pulses.
        #[arg(long, default_value_t = 100_000)]
        valid_for: u64,
    },
    /// Post a bounty for a better version of a release: a Task Charter
    /// whose spec names the release, the evaluation set and the cases to
    /// pass, and whose verifiers are fixed now, before any version exists.
    Bounty {
        /// The poster's wallet.
        wallet: String,
        /// The release to improve (its claim id).
        release: String,
        /// The evaluation set (a JSON file).
        #[arg(long)]
        evals: PathBuf,
        /// Escrow (USD), paid for an accepted version.
        #[arg(long)]
        amount: String,
        /// Verifiers whose attestations count (comma-separated wallets or
        /// accounts).
        #[arg(long, value_delimiter = ',', required = true)]
        verifiers: Vec<String>,
        /// Verifiers who must each attest a version, among --verifiers
        /// (comma-separated).
        #[arg(long, value_delimiter = ',')]
        must_attest: Vec<String>,
        /// How many attestations a version needs at least.
        #[arg(long, value_name = "N", default_value_t = 1)]
        min_attestations: u8,
        /// The adjudicator atom results must name.
        #[arg(long)]
        adjudicator: String,
        /// Claims every result must rest on, such as the set's vouching
        /// (comma-separated ids).
        #[arg(long, value_delimiter = ',')]
        requires: Vec<String>,
        /// The worker (default: anyone may take it).
        #[arg(long)]
        worker: Option<String>,
        /// The cases a version must pass (default: all).
        #[arg(long)]
        pass: Option<usize>,
        /// Pulses until the deadline.
        #[arg(long, default_value_t = 1_000)]
        deadline: u64,
        /// κ of your decision rule for a result.
        #[arg(long, default_value_t = 2.0)]
        kappa: f64,
        /// Pulses you rely on a paid version (its backing stays reserved).
        #[arg(long, default_value_t = 1_000)]
        reliance: u64,
    },
    /// Verify a version for a bounty, as one of its verifiers: check that
    /// it keeps the skill's purpose, run the evaluation set on it yourself,
    /// and attest on your bond only if enough cases pass.
    Verify {
        /// The verifier's wallet.
        wallet: String,
        /// The bounty (its task id).
        task: String,
        /// The version's directory.
        dir: PathBuf,
        /// The agent's model, as for `eval`. Repeat for fallbacks.
        #[arg(long = "model", required = true)]
        models: Vec<String>,
        /// The evaluation set (default: the local store, by the digest the
        /// spec names).
        #[arg(long)]
        evals: Option<PathBuf>,
        /// The task's spec (default: the local store).
        #[arg(long)]
        spec: Option<PathBuf>,
        /// The label of your bond the attestation rests on.
        #[arg(long, default_value = crate::tasks::VERIFIER_BOND_LABEL)]
        bond: String,
    },
    /// Submit a version for a bounty, as its worker, with its attestations,
    /// and settle the task.
    Submit {
        /// The worker's wallet.
        wallet: String,
        /// The bounty (its task id).
        task: String,
        /// The version's directory.
        dir: PathBuf,
        /// Attestation claim ids (comma-separated).
        #[arg(long, value_delimiter = ',')]
        attestations: Vec<String>,
    },
    /// A release's lineage: each version back to the first, with what it
    /// rests on and the evidence for each step.
    Lineage {
        /// The release (its claim id).
        release: String,
    },
    /// Import an Agent Skill for a profile's agent, from a folder or a git
    /// repository: it is read and reported on (its files, its scripts, the
    /// hosts and patterns in them, the permissions it asks for), and
    /// installed only once --accept names its bundle root after you read
    /// it, or --audit names an active audit of that root on a bond.
    Import {
        /// A folder, github:owner/repo, a git address (https://…),
        /// clawhub:owner/slug[@version], skills.sh:owner/repo/skill, or a
        /// site's skill (well-known:example.org#name).
        source: String,
        /// The profile whose agent gets it.
        #[arg(long)]
        profile: String,
        /// The skill's folder within the repository.
        #[arg(long)]
        path: Option<PathBuf>,
        /// The branch or tag to read (default: the repository's own).
        #[arg(long = "ref", value_name = "REF")]
        rev: Option<String>,
        /// Install it: the bundle root the report gave, after reading it.
        #[arg(long, value_name = "ROOT", conflicts_with = "audit")]
        accept: Option<String>,
        /// Install it on an audit: the claim of an active audit of its
        /// bundle on a bond (needs the node).
        #[arg(long, value_name = "CLAIM")]
        audit: Option<String>,
        /// Grant the permissions it asks for (a network, secrets).
        #[arg(long)]
        accept_permissions: bool,
        /// Install it despite the findings the report says hold it back,
        /// once you have read them.
        #[arg(long)]
        accept_risks: bool,
        /// A host its scripts may reach, declared for it in its metadata
        /// (repeat for more; needs --accept-permissions).
        #[arg(long = "grant-network", value_name = "HOST")]
        grant_network: Vec<String>,
        /// A secret its scripts receive as an environment variable,
        /// declared for it (repeat for more; keep it with sion agent secret
        /// add; needs --accept-permissions).
        #[arg(long = "grant-secret", value_name = "NAME")]
        grant_secret: Vec<String>,
    },
    /// Search a registry for skills to import: ClawHub (OpenClaw's) or
    /// skills.sh. What a registry says of a skill is shown, not relied on.
    Search {
        /// What to look for.
        query: String,
        /// The registry: clawhub or skills.sh.
        #[arg(long, default_value = "clawhub")]
        registry: String,
        /// How many to list.
        #[arg(long, default_value_t = 10)]
        limit: usize,
    },
    /// Teach a task by doing it once in a browser window: what you do is
    /// recorded (never a password, a secret field or a login's user name;
    /// at most 10 minutes) and written down as a draft skill for the
    /// agent's browser, to read and then install with `sion skill import`.
    Teach(crate::teach::TeachArgs),
    /// Install sion's starter skills for a profile's agent: web-research,
    /// code-change, pr-review, ci-fix, inbox-triage, scheduling and
    /// document-brief (all of them, or those named).
    Starter {
        /// The profile whose agent gets them.
        #[arg(long)]
        profile: String,
        /// The skills (default: all).
        names: Vec<String>,
    },
}

/// Runs the skill commands that need no node (`pack`, `eval`, `evolve`);
/// None for the others.
pub fn run_offline(command: &SkillCommand) -> Option<Result<(), CliError>> {
    let print = |lines: Vec<String>| {
        for line in lines {
            println!("{line}");
        }
    };
    match command {
        SkillCommand::Pack { dir, out } => Some(pack_command(dir, out.as_deref())),
        SkillCommand::Eval {
            dir,
            evals,
            models,
            trajectories,
        } => Some(
            crate::skill_improve::eval(dir, evals, models, trajectories.as_deref())
                .map(|(_, lines)| print(lines)),
        ),
        SkillCommand::Evolve {
            dir,
            evals,
            models,
            mutator,
            generations,
            max_bytes,
            held_out,
        } => {
            let settings = sion_agent::evolve::Settings {
                generations: *generations,
                max_bytes: *max_bytes,
                ..sion_agent::evolve::Settings::default()
            };
            Some(
                crate::skill_improve::evolve(
                    dir,
                    evals,
                    held_out.as_deref(),
                    models,
                    mutator,
                    &settings,
                )
                .map(|(_, _, lines)| print(lines)),
            )
        }
        SkillCommand::Import {
            source,
            profile,
            path,
            rev,
            accept,
            audit: None,
            accept_permissions,
            accept_risks,
            grant_network,
            grant_secret,
        } => Some(
            crate::skill_import::Source::parse(source, rev.clone())
                .map_err(CliError::Usage)
                .and_then(|source| {
                    crate::skill_import::import(
                        &source,
                        path.as_deref(),
                        profile,
                        accept
                            .clone()
                            .map(crate::skill_import::Vouched::Review)
                            .as_ref(),
                        &crate::skill_import::Consent {
                            network: grant_network.clone(),
                            secrets: grant_secret.clone(),
                            permissions: *accept_permissions,
                            risks: *accept_risks,
                        },
                        None,
                    )
                })
                .map(print),
        ),
        SkillCommand::Search {
            query,
            registry,
            limit,
        } => Some(
            crate::skill_registries::search(registry, query, *limit).map(|found| {
                if found.is_empty() {
                    println!("{registry} has nothing for {query:?}");
                }
                for skill in found {
                    println!("{}", skill.line());
                }
            }),
        ),
        SkillCommand::Starter { profile, names } => {
            Some(crate::skill_import::starter(profile, names).map(print))
        }
        SkillCommand::Teach(args) => Some(crate::teach::run(args)),
        _ => None,
    }
}

fn pack_command(dir: &Path, out: Option<&Path>) -> Result<(), CliError> {
    let (bytes, root, name, _) = pack(dir)?;
    let path = match out {
        Some(path) => {
            std::fs::write(path, &bytes).map_err(|e| CliError::Usage(e.to_string()))?;
            path.to_path_buf()
        }
        None => store_bundle(&bytes, &root)?,
    };
    println!(
        "skill {name}: bundle {root} ({} bytes) at {}",
        bytes.len(),
        path.display()
    );
    Ok(())
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

/// Keeps a bundle in the local store, under its root.
pub fn store_bundle(bytes: &[u8], root: &Hash32) -> Result<PathBuf, CliError> {
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
    /// The verified improvements it rests on: settled bounties' results,
    /// as the release records them.
    pub evidence: Vec<Value>,
}

/// The key a publisher's index names its latest release of `name` under:
/// `skill:<name>`.
pub fn index_key(name: &str) -> String {
    format!("skill:{name}")
}

/// `publisher/name` as its two parts.
pub fn split_skill(text: &str) -> Result<(&str, &str), CliError> {
    match text.split_once('/') {
        Some((publisher, name))
            if !publisher.is_empty() && sion_agent::skills::valid_name(name) =>
        {
            Ok((publisher, name))
        }
        _ => Err(CliError::Usage(format!(
            "{text:?} is not publisher/name, with a skill's name"
        ))),
    }
}

/// The chain identity to verify against, if one is given (64 hex digits).
pub fn pinned_chain(text: Option<&str>) -> Result<Option<Hash32>, CliError> {
    text.map(|t| parse_hash(t, "the chain identity"))
        .transpose()
}

/// Names `claim` as `wallet`'s latest release of `name` in its index (spec
/// v1.15).
pub fn index_release(
    session: &mut Session<'_>,
    wallet: &str,
    name: &str,
    claim: ClaimId,
) -> Result<sion_node::TxView, CliError> {
    session.send(
        wallet,
        TxKind::IndexClaim {
            key: index_key(name),
            claim,
        },
        Vec::new(),
    )
}

/// `publisher`'s latest release of `name`: the claim its index entry names,
/// the entry proven against a certified header as a light client would
/// (against `pinned`, if given), and fresh (see
/// [`crate::verify::verify_state`]; `min_pulse` is a certified Pulse the
/// caller knows of from elsewhere), then the release read and checked to
/// be that publisher's release of that skill.
pub fn latest(
    ledger: &dyn Ledger,
    pinned: Option<Hash32>,
    min_pulse: Option<Pulse>,
    publisher: sion_core::ids::AccountId,
    name: &str,
) -> Result<
    (
        Release,
        sion_exec::index::IndexEntry,
        crate::verify::VerifiedState,
    ),
    CliError,
> {
    let id = sion_exec::index::index_id(&publisher, &index_key(name));
    let proven = crate::verify::verify_object_now(ledger, pinned, id, min_pulse)?;
    let entry = match proven.objects.first().and_then(|(_, o)| o.as_ref()) {
        Some(object) => match &object.data {
            ObjectData::IndexEntry(entry) => entry.clone(),
            _ => {
                return Err(CliError::Verification(format!(
                    "{id} holds something other than an index entry"
                )))
            }
        },
        None => {
            return Err(CliError::Usage(format!(
                "the publisher has indexed no release of {name} (proven absent: {})",
                proven.line()
            )))
        }
    };
    let found = release(ledger, &entry.claim.to_string())?;
    if found.publisher != publisher || found.name != name {
        return Err(CliError::Verification(format!(
            "the index names claim {}, which is not {name} by this publisher",
            entry.claim.short()
        )));
    }
    Ok((found, entry, proven))
}

/// A release as an install names it: a claim id as it is, or
/// `publisher/name` resolved to the publisher's latest release through its
/// index, proven against a certified header (against `pinned`, if given).
/// Returns the claim id, and for a name, a line saying how it was found.
pub fn resolve(
    session: &mut Session<'_>,
    ledger: &dyn Ledger,
    text: &str,
    pinned: Option<Hash32>,
    min_pulse: Option<Pulse>,
) -> Result<(String, Option<String>), CliError> {
    if !text.contains('/') {
        return Ok((text.to_string(), None));
    }
    let (publisher, name) = split_skill(text)?;
    let publisher = session.id(publisher);
    let (found, _, proven) = latest(ledger, pinned, min_pulse, publisher, name)?;
    let line = format!(
        "{name} {} is {}'s latest release ({}): {}",
        found.version,
        session.names().account(&publisher),
        found.claim.short(),
        proven.line()
    );
    Ok((found.claim.to_string(), Some(line)))
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
    let evidence: Vec<Value> = payload["evidence"].as_array().cloned().unwrap_or_default();
    let results: Vec<String> = evidence
        .iter()
        .filter_map(|e| e["result"].as_str().map(str::to_string))
        .collect();
    let mut audits = Vec::new();
    collect_claims(&stored.body.formula, &mut audits);
    audits.retain(|a| !results.contains(&a.to_string()));
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
        evidence,
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

/// What a release rests on: the publisher's bond, audits of its bundle,
/// and results of settled bounties for it (see
/// [`crate::skill_improve::evidence`]).
#[derive(Clone, Debug, Default)]
pub struct Backing<'a> {
    /// The label of the publisher's bond.
    pub bond: Option<&'a str>,
    /// Audits of the bundle.
    pub audits: &'a [ClaimId],
    /// Results of settled bounties for a better version of the release it
    /// follows, for this very bundle.
    pub evidence: &'a [ClaimId],
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
    backing: Backing<'_>,
    adjudicator_atom: AtomId,
    previous: Option<ClaimId>,
    locator: Option<&str>,
    valid_for: u64,
) -> Result<(ClaimId, Hash32, sion_node::TxView), CliError> {
    let Backing {
        bond,
        audits,
        evidence,
    } = backing;
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
    // Each piece of evidence must be a settled bounty's result for these
    // very bytes, improving the release this one follows.
    let recorded = evidence
        .iter()
        .map(|e| crate::skill_improve::evidence(ledger, *e, previous, &bytes))
        .collect::<Result<Vec<_>, _>>()?;
    let mut backing: Vec<Formula> = Vec::new();
    if let Some(label) = bond {
        backing.push(Formula::atom(
            bond_atom(AtomClass::Economic, &publisher, label).id,
        ));
    }
    backing.extend(audits.iter().map(|a| Formula::Claim(*a)));
    backing.extend(evidence.iter().map(|e| Formula::Claim(*e)));
    if backing.is_empty() {
        return Err(CliError::Usage(
            "a release rests on your bond, on audits or on evidence: give --bond, --audits or --evidence"
                .into(),
        ));
    }
    let formula = Formula::and(vec![formulas::dual_signature(), Formula::or(backing)]);
    let mut payload = json!({
        "name": name,
        "version": version,
        "bundle": root.to_hex(),
        "size": bytes.len(),
        "previous": previous.map(|p| p.to_string()),
        "locator": locator,
    });
    if !recorded.is_empty() {
        payload["evidence"] = json!(recorded);
    }
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

/// Takes out of use every installed skill in `skills` whose release is no
/// longer active (falsified, or expired): its directory is renamed so that
/// the agent no longer sees it. Returns a line for each.
pub fn quarantine_falsified(ledger: &dyn Ledger, skills: &Path) -> Vec<String> {
    let mut lines = Vec::new();
    for skill in sion_agent::skills::list(skills) {
        let Ok(text) = std::fs::read_to_string(skill.dir.join(SOURCE)) else {
            continue;
        };
        let Some(release) = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|v| v["release"].as_str().map(str::to_string))
        else {
            continue;
        };
        let Ok(id) = parse_hash(&release, "release") else {
            continue;
        };
        let state = match ledger.claim(ClaimId::from_hash(id)) {
            Ok(view) => status_word(&view["status"]),
            Err(_) => continue,
        };
        if state == "active" {
            continue;
        }
        let state = if state.is_empty() {
            "inactive".to_string()
        } else {
            state
        };
        let target = skills.join(format!("{}.{state}-{}", skill.name, &release[..8]));
        if std::fs::rename(&skill.dir, &target).is_ok() {
            lines.push(format!(
                "skill {} taken out of use: its release {} is {state}",
                skill.name,
                &release[..8]
            ));
        }
    }
    lines
}

/// The state a claim's status names, in lower case: `active`, `falsified`
/// or `expired`. The node reports a status in its debug form
/// (`Falsified { at: Pulse(9), causes: [...] }`), of which only the first
/// word is the state; empty if there is none.
pub fn status_word(status: &Value) -> String {
    match status {
        Value::String(text) => text
            .split(|c: char| !c.is_ascii_alphabetic())
            .next()
            .unwrap_or_default()
            .to_lowercase(),
        Value::Object(map) => map
            .keys()
            .next()
            .map(|k| k.to_lowercase())
            .unwrap_or_default(),
        _ => String::new(),
    }
}

/// Runs a skill command.
pub fn run(
    session: &mut Session<'_>,
    ledger: &dyn Ledger,
    command: SkillCommand,
) -> Result<(), CliError> {
    if let Some(result) = run_offline(&command) {
        return result;
    }
    match command {
        SkillCommand::Pack { .. } | SkillCommand::Eval { .. } | SkillCommand::Evolve { .. } => {}
        SkillCommand::Latest {
            skill,
            chain,
            min_pulse,
        } => {
            let (publisher, name) = split_skill(&skill)?;
            let publisher = session.id(publisher);
            let (found, entry, proven) = latest(
                ledger,
                pinned_chain(chain.as_deref())?,
                min_pulse.map(Pulse),
                publisher,
                name,
            )?;
            let out = json!({
                "skill": name,
                "publisher": found.publisher,
                "release": found.claim,
                "version": found.version,
                "bundle": found.bundle,
                "previous": found.previous,
                "index": {
                    "entry": sion_exec::index::index_id(&publisher, &index_key(name)),
                    "releases_named": entry.count,
                    "updated_at": entry.updated_at,
                    "proof": proven.line(),
                },
            });
            println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
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
            evidence,
            previous,
            locator,
            valid_for,
            no_index,
        } => {
            let audits = claim_ids(&audits, "an audit")?;
            let evidence = claim_ids(&evidence, "a result")?;
            let previous = previous
                .map(|p| parse_hash(&p, "the previous release").map(ClaimId::from_hash))
                .transpose()?;
            let (id, root, view) = publish(
                session,
                ledger,
                &wallet,
                &dir,
                &version,
                Backing {
                    bond: bond.as_deref(),
                    audits: &audits,
                    evidence: &evidence,
                },
                adjudicator(&judge)?,
                previous,
                locator.as_deref(),
                valid_for,
            )?;
            crate::agents::print_receipt(session, &view);
            println!("release {id}\nbundle {root}");
            if !no_index {
                let name = release(ledger, &id.to_string())?.name;
                let view = index_release(session, &wallet, &name, id)?;
                crate::agents::print_receipt(session, &view);
                if view.status != ExecStatus::Success {
                    return Err(CliError::Transaction(
                        "the release was published, but not indexed".into(),
                    ));
                }
            }
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
                "evidence": release.evidence,
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
            chain,
            min_pulse,
            bundle,
            value,
            kappa,
            pulses,
            skills,
            accept_permissions,
        } => {
            let agent = ObjectId::from_hash(parse_hash(&agent, "the agent")?);
            let (id, line) = resolve(
                session,
                ledger,
                &id,
                pinned_chain(chain.as_deref())?,
                min_pulse.map(Pulse),
            )?;
            if let Some(line) = line {
                println!("{line}");
            }
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
                accept_permissions,
            )?;
            for line in lines {
                println!("{line}");
            }
        }
        SkillCommand::Vouch {
            wallet,
            evals,
            bond,
            adjudicator: judge,
            valid_for,
        } => {
            let (id, view) = crate::skill_improve::vouch(
                session,
                &wallet,
                &evals,
                &bond,
                adjudicator(&judge)?,
                valid_for,
            )?;
            crate::agents::print_receipt(session, &view);
            println!("vouching {id}");
        }
        SkillCommand::Bounty {
            wallet,
            release: id,
            evals,
            amount,
            verifiers,
            must_attest,
            min_attestations,
            adjudicator: judge,
            requires,
            worker,
            pass,
            deadline,
            kappa,
            reliance,
        } => {
            let requires = claim_ids(&requires, "a required claim")?;
            let (task, spec, view) = crate::skill_improve::bounty(
                session,
                ledger,
                &wallet,
                &id,
                &evals,
                &amount,
                &verifiers,
                &crate::skill_improve::Attesting {
                    must: must_attest,
                    at_least: min_attestations,
                },
                adjudicator(&judge)?,
                &requires,
                worker.as_deref(),
                pass,
                deadline,
                kappa,
                reliance,
            )?;
            crate::agents::print_receipt(session, &view);
            println!(
                "task {task}
spec {}",
                spec.display()
            );
        }
        SkillCommand::Verify {
            wallet,
            task,
            dir,
            models,
            evals,
            spec,
            bond,
        } => {
            let task = ObjectId::from_hash(parse_hash(&task, "the task")?);
            crate::skill_improve::verify(
                session,
                ledger,
                &wallet,
                task,
                &dir,
                evals.as_deref(),
                spec.as_deref(),
                &models,
                &bond,
            )?;
        }
        SkillCommand::Submit {
            wallet,
            task,
            dir,
            attestations,
        } => {
            let task = ObjectId::from_hash(parse_hash(&task, "the task")?);
            let attestations = claim_ids(&attestations, "an attestation")?;
            crate::skill_improve::submit(session, ledger, &wallet, task, &dir, &attestations)?;
        }
        SkillCommand::Import {
            source,
            profile,
            path,
            rev,
            audit,
            accept_permissions,
            accept_risks,
            grant_network,
            grant_secret,
            ..
        } => {
            let source =
                crate::skill_import::Source::parse(&source, rev).map_err(CliError::Usage)?;
            for line in crate::skill_import::import(
                &source,
                path.as_deref(),
                &profile,
                audit.map(crate::skill_import::Vouched::Audit).as_ref(),
                &crate::skill_import::Consent {
                    network: grant_network,
                    secrets: grant_secret,
                    permissions: accept_permissions,
                    risks: accept_risks,
                },
                Some(ledger),
            )? {
                println!("{line}");
            }
        }
        SkillCommand::Starter { .. } | SkillCommand::Search { .. } | SkillCommand::Teach(_) => {}
        SkillCommand::Lineage { release: id } => {
            for line in crate::skill_improve::lineage(ledger, &id)? {
                println!("{line}");
            }
        }
    }
    Ok(())
}

fn claim_ids(texts: &[String], what: &str) -> Result<Vec<ClaimId>, CliError> {
    texts
        .iter()
        .map(|t| parse_hash(t, what).map(ClaimId::from_hash))
        .collect()
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
    accept_permissions: bool,
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
    let permissions = sion_agent::skills::front_matter(&skill_md)
        .and_then(|front| front.permissions())
        .map_err(|e| {
            CliError::Verification(format!(
                "not installed: the permissions skill {name} declares cannot be read: {e}"
            ))
        })?;
    let value_at_risk = match value {
        Some(text) => crate::agents::parse_amount(text)?,
        None => budget(ledger, agent)?,
    };
    let policy =
        Policy::standard().with_kappa(Ratio::new((kappa * 1000.0).round().max(1.0) as u64, 1000));
    // Shown now, before the acceptance is signed.
    println!(
        "bundle {} checked: {} {}, skill {} {}",
        release.bundle.short(),
        files.len(),
        if files.len() == 1 { "file" } else { "files" },
        release.name,
        release.version
    );
    let granted = permissions.describe();
    if granted.is_empty() {
        println!("permissions: none (its scripts, if any, get no network and no secrets)");
    } else {
        println!("permissions it asks for:");
        for line in &granted {
            println!("  {line}");
        }
    }
    if permissions.needs_consent() && !accept_permissions {
        return Err(CliError::Usage(format!(
            "not installed: skill {name} asks for the permissions above; nothing was signed. \
             To grant them, install again with --accept-permissions"
        )));
    }
    let mut lines = Vec::new();
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
        "permissions": granted,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_status_names_its_state_by_its_first_word() {
        assert_eq!(status_word(&json!("Active")), "active");
        assert_eq!(status_word(&json!("Expired")), "expired");
        assert_eq!(
            status_word(&json!(
                "Falsified { at: Pulse(109), causes: [AtomId(f88c2190)] }"
            )),
            "falsified"
        );
        assert_eq!(
            status_word(&json!({"Falsified": {"at": 109, "causes": []}})),
            "falsified"
        );
        assert_eq!(status_word(&Value::Null), "");
    }
}
