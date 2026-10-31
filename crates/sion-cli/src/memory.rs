//! Memory logs (specification v1.9) with the `sion-memory` library: a log
//! opened on the ledger, atoms recorded in a local store, heads committed
//! with a proof that the log only grew, and recall answers proved complete
//! against the head the ledger holds.
//!
//! The local store of a log lives in `$SION_HOME/memory/<log id>`. Its
//! atoms never leave the machine unless the writer serves them or archives
//! them; what the ledger holds is the log's size, its Merkle root, its
//! index root and, once archived, its manifest: the storage root of its
//! latest archive ([`crate::archive`]). An agent's episodes are consolidated
//! into notes with `sion memory consolidate` ([`crate::consolidate`]).

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use clap::Subcommand;
use serde_json::{json, Value as Json};
use sion_core::encode::encode;
use sion_core::hash::Hash32;
use sion_core::ids::{AccountId, ObjectId};
use sion_exec::execute::Event;
use sion_exec::memory::MemoryLog;
use sion_exec::object::ObjectData;
use sion_exec::TxKind;
use sion_memory::{
    prove_range, verify_range, AtomClass, Box3, FrameId, Interval, LogStore, MemoryAtom,
    MemoryKind, RangeQuery, Statement, TimeRange, Value,
};

use crate::error::CliError;
use crate::ledger::{Ledger, LedgerExt};
use crate::session::Session;

/// `sion memory …`
#[derive(Debug, Subcommand)]
pub enum MemoryCommand {
    /// Open a memory log on the ledger, owned by the wallet (for an agent
    /// identity with --agent, whose agent must be the wallet), and a local
    /// store for its atoms.
    Open {
        /// The writer's wallet.
        wallet: String,
        /// The agent identity the log belongs to (its id).
        #[arg(long)]
        agent: Option<String>,
    },
    /// Record an atom in a log's local store. It reaches the ledger's
    /// commitment with the next `sion memory commit`.
    Remember {
        /// The writer's wallet.
        wallet: String,
        /// The log (its id, or a unique prefix of it).
        log: String,
        /// What the statement is about, for example pallet-17.
        subject: String,
        /// The relation, for example at.
        predicate: String,
        /// The value: true or false, an integer, @name for an entity, or
        /// text.
        value: String,
        /// observation, coverage, semantic, episodic, procedural, …
        #[arg(long, default_value = "observation")]
        kind: String,
        /// The frame its place is in, for example warehouse-3.
        #[arg(long)]
        frame: Option<String>,
        /// Its place in the frame, in millimetres: x,y,z.
        #[arg(long)]
        at: Option<String>,
        /// Half the size of its box around `at`, in millimetres: dx,dy,dz.
        #[arg(long)]
        size: Option<String>,
        /// When it began to hold, in seconds since the Unix epoch (default:
        /// now).
        #[arg(long)]
        from: Option<u64>,
        /// For how many seconds it held (default: open-ended).
        #[arg(long)]
        seconds: Option<u64>,
        /// Confidence, in thousandths.
        #[arg(long, default_value_t = 1000)]
        confidence: u16,
    },
    /// Commit the local log's head to the ledger, with a proof that it
    /// extends the head the ledger holds.
    Commit {
        /// The writer's wallet.
        wallet: String,
        /// The log (its id, or a unique prefix of it).
        log: String,
        /// Close the log for good.
        #[arg(long)]
        seal: bool,
    },
    /// Everything the log had recorded in a box during an interval, as of
    /// its committed head, with a proof checked against the index root the
    /// ledger holds.
    Recall {
        /// The log (its id, or a unique prefix of it).
        log: String,
        /// The frame, for example warehouse-3.
        frame: String,
        /// The box's lower corner, in millimetres: x,y,z.
        #[arg(long)]
        min: String,
        /// The box's upper corner, in millimetres: x,y,z.
        #[arg(long)]
        max: String,
        /// The interval's start, in seconds since the Unix epoch.
        #[arg(long)]
        from: u64,
        /// The interval's end, in seconds since the Unix epoch (exclusive).
        #[arg(long)]
        until: u64,
    },
    /// A log's head on the ledger and its local store.
    Show {
        /// The log (its id, or a unique prefix of it).
        log: String,
    },
    /// Consolidate an agent's memory: a model reads the episodes recorded
    /// since the last consolidation and the notes the log keeps, and
    /// proposes facts, procedures and preferences worth keeping, each
    /// citing its episodes. What the gates keep is recorded as derived
    /// atoms (the notes they replace are superseded, not deleted), and the
    /// log is committed.
    Consolidate {
        /// The writer's wallet (the agent's).
        wallet: String,
        /// The log (its id, or a unique prefix of it).
        log: String,
        /// The model that proposes: anthropic:<model>, openai:<model>,
        /// gemini:<model> or script:<file>.
        #[arg(long)]
        model: String,
    },
    /// Archive a log for safekeeping: every atom the ledger holds,
    /// encrypted for the writer, named on the ledger as the log's manifest
    /// and kept by a storage provider under a deal.
    Archive {
        /// The writer's wallet.
        wallet: String,
        /// The log (its id, or a unique prefix of it).
        log: String,
        /// Where to write the archive (default:
        /// $SION_HOME/memory/archives/<log>-<size>.bin).
        #[arg(long)]
        out: Option<PathBuf>,
        /// The storage deal's terms.
        #[command(flatten)]
        terms: crate::storage::DealTerms,
    },
    /// Restore a log's local store from its latest archive, checked against
    /// the head on the ledger before anything is replaced.
    Restore {
        /// The writer's wallet.
        wallet: String,
        /// The log (its full id, if the local store is gone).
        log: String,
        /// The archive: a file, or the retrieval address of the provider
        /// that keeps it (http:// or https://).
        #[arg(long)]
        from: String,
        /// With a provider: the deal that keeps the archive (default: your
        /// latest accepted deal for the log's manifest).
        #[arg(long)]
        deal: Option<String>,
        /// Restore over the local store there is.
        #[arg(long)]
        replace: bool,
    },
}

/// Nanoseconds per second.
const SECOND: u64 = 1_000_000_000;

/// Runs a memory command.
pub fn run(
    session: &mut Session<'_>,
    ledger: &dyn Ledger,
    command: MemoryCommand,
) -> Result<(), CliError> {
    match command {
        MemoryCommand::Open { wallet, agent } => {
            let agent = agent
                .as_deref()
                .map(|a| parse_hash(a, "agent").map(ObjectId::from_hash))
                .transpose()?;
            let (view, log) = open(session, &wallet, agent)?;
            crate::agents::print_receipt(session, &view);
            if let Some(log) = log {
                println!("log {log}");
            }
            Ok(())
        }
        MemoryCommand::Remember {
            wallet,
            log,
            subject,
            predicate,
            value,
            kind,
            frame,
            at,
            size,
            from,
            seconds,
            confidence,
        } => {
            let id = find_log(ledger, &log)?;
            let place = match (frame, at) {
                (Some(frame), Some(at)) => {
                    let half = size
                        .as_deref()
                        .map(parse_point)
                        .transpose()?
                        .unwrap_or([0; 3]);
                    Some((frame, parse_point(&at)?, half))
                }
                (None, None) => None,
                _ => return Err(CliError::Usage("--frame and --at go together".into())),
            };
            let recorded = remember(
                session,
                ledger,
                &wallet,
                &id,
                Remember {
                    subject,
                    predicate,
                    value: parse_value(&value),
                    kind: parse_kind(&kind)?,
                    place,
                    from_s: from,
                    seconds,
                    confidence,
                },
            )?;
            println!(
                "recorded atom {} ({}) in log {}; the log holds {} atoms, {} not yet committed",
                recorded["seq"],
                recorded["id"].as_str().unwrap_or("").get(..8).unwrap_or(""),
                id.0.short(),
                recorded["size"],
                recorded["uncommitted"]
            );
            Ok(())
        }
        MemoryCommand::Commit { wallet, log, seal } => {
            let id = find_log(ledger, &log)?;
            let view = commit(session, ledger, &wallet, &id, seal)?;
            crate::agents::print_receipt(session, &view);
            Ok(())
        }
        MemoryCommand::Recall {
            log,
            frame,
            min,
            max,
            from,
            until,
        } => {
            let id = find_log(ledger, &log)?;
            let answer = recall(
                ledger,
                &id,
                &frame,
                Box3::new(parse_point(&min)?, parse_point(&max)?),
                TimeRange::new(from.saturating_mul(SECOND), until.saturating_mul(SECOND)),
            )?;
            println!(
                "{}",
                serde_json::to_string_pretty(&answer).unwrap_or_default()
            );
            Ok(())
        }
        MemoryCommand::Show { log } => {
            let id = find_log(ledger, &log)?;
            let state = committed(ledger, &id)?;
            let local = LogStore::open(store_dir(&id))
                .ok()
                .map(|store| store.log().len());
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "log": id,
                    "writer": state.writer,
                    "agent": state.agent,
                    "committed": {
                        "size": state.size,
                        "root": state.root,
                        "index": state.index,
                        "manifest": state.manifest,
                        "commits": state.commits,
                        "at_pulse": state.committed_at,
                        "sealed": state.sealed,
                    },
                    "local_atoms": local,
                }))
                .unwrap_or_default()
            );
            Ok(())
        }
        MemoryCommand::Consolidate { wallet, log, model } => {
            let id = find_log(ledger, &log)?;
            let limits = sion_agent::Limits::default();
            let answer = crate::consolidate::run(session, ledger, &wallet, &id, &model, &limits)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&answer).unwrap_or_default()
            );
            Ok(())
        }
        MemoryCommand::Archive {
            wallet,
            log,
            out,
            terms,
        } => {
            let id = find_log(ledger, &log)?;
            let made = crate::archive::make(session, ledger, &wallet, &id, out.as_deref())?;
            for line in &made.receipts {
                println!("{line}");
            }
            let bytes = std::fs::read(&made.path)
                .map_err(|e| CliError::Usage(format!("{}: {e}", made.path.display())))?;
            println!(
                "archive {} ({} bytes, encrypted for {wallet}): {} atoms at root {}; storage root {}, now the log's manifest",
                made.path.display(),
                bytes.len(),
                made.header.size,
                made.header.root.short(),
                made.tree.root().short()
            );
            let (view, deal) =
                crate::storage::post_deal(session, ledger, &wallet, &made.path, &bytes, &terms)?;
            crate::agents::print_receipt(session, &view);
            if let Some(deal) = deal {
                println!("deal {deal}");
            }
            Ok(())
        }
        MemoryCommand::Restore {
            wallet,
            log,
            from,
            deal,
            replace,
        } => {
            let id = find_log(ledger, &log)?;
            let head = committed(ledger, &id)?;
            if head.writer != session.id(&wallet) {
                return Err(CliError::Usage(format!(
                    "log {} is written by another account",
                    id.0.short()
                )));
            }
            let bytes = if from.starts_with("http://") || from.starts_with("https://") {
                let deal = match deal {
                    Some(text) => crate::storage::find_deal(ledger, &text)?,
                    None => crate::archive::deal_for(ledger, &head.writer, &head.manifest)?,
                };
                crate::storage::retrieve(session, ledger, &wallet, &from, deal)?
            } else {
                std::fs::read(&from).map_err(|e| CliError::Usage(format!("{from}: {e}")))?
            };
            let answer = crate::archive::restore(session, ledger, &wallet, &id, &bytes, replace)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&answer).unwrap_or_default()
            );
            Ok(())
        }
    }
}

/// Opens a memory log on the ledger for `wallet` (for an agent identity
/// with `agent`) and its local store.
pub fn open(
    session: &mut Session<'_>,
    wallet: &str,
    agent: Option<ObjectId>,
) -> Result<(sion_node::TxView, Option<ObjectId>), CliError> {
    let view = session.send(
        wallet,
        TxKind::OpenMemory {
            agent,
            policy: Hash32::ZERO,
        },
        Vec::new(),
    )?;
    let mut opened = None;
    for event in &view.effects.events {
        if let Event::MemoryOpened { log, account, .. } = event {
            LogStore::open_or_create(store_dir(log), *account).map_err(memory_error)?;
            opened = Some(*log);
        }
    }
    Ok((view, opened))
}

/// What `sion memory remember` records.
#[derive(Debug)]
pub struct Remember {
    /// The subject.
    pub subject: String,
    /// The predicate.
    pub predicate: String,
    /// The value.
    pub value: Value,
    /// The kind of memory.
    pub kind: MemoryKind,
    /// Frame, centre and half-size in millimetres, if it has a place.
    pub place: Option<(String, [i64; 3], [i64; 3])>,
    /// Start of its valid time, in seconds since the Unix epoch (default
    /// now).
    pub from_s: Option<u64>,
    /// How many seconds it held (default open-ended).
    pub seconds: Option<u64>,
    /// Confidence in thousandths.
    pub confidence: u16,
}

/// Appends an atom to `log`'s local store.
pub fn remember(
    session: &mut Session<'_>,
    ledger: &dyn Ledger,
    wallet: &str,
    log: &ObjectId,
    atom: Remember,
) -> Result<Json, CliError> {
    let writer = session.id(wallet);
    let mut store = open_store(log, writer)?;
    let start = atom
        .from_s
        .map(|s| s.saturating_mul(SECOND))
        .unwrap_or_else(now);
    let valid = match atom.seconds {
        Some(s) => Interval::closed(start, start.saturating_add(s.saturating_mul(SECOND))),
        None => Interval::starting(start),
    };
    let mut builder = MemoryAtom::builder(
        writer,
        store.log().len(),
        atom.kind,
        class_of(atom.kind),
        Statement::claim(atom.subject, atom.predicate, atom.value),
    )
    .valid(valid)
    .observed_at(now())
    .confidence(atom.confidence);
    if let Some((frame, centre, half)) = atom.place {
        builder = builder.space(FrameId::named(&frame), Box3::around(centre, half), 0);
    }
    let built = builder
        .build()
        .map_err(|e| CliError::Usage(e.to_string()))?;
    let id = built.id();
    let seq = store.append(built).map_err(memory_error)?;
    let size = store.log().len();
    let committed_size = committed(ledger, log)?.size;
    Ok(json!({
        "log": log,
        "id": id,
        "seq": seq,
        "size": size,
        "uncommitted": size.saturating_sub(committed_size),
    }))
}

/// Commits `log`'s local head to the ledger, with a consistency proof from
/// the head the ledger holds.
pub fn commit(
    session: &mut Session<'_>,
    ledger: &dyn Ledger,
    wallet: &str,
    log: &ObjectId,
    seal: bool,
) -> Result<sion_node::TxView, CliError> {
    let writer = session.id(wallet);
    let store = open_store(log, writer)?;
    let on_chain = committed(ledger, log)?;
    let local = store.log();
    if local.len() < on_chain.size {
        return Err(CliError::Usage(format!(
            "the local store holds {} atoms, fewer than the {} the ledger has",
            local.len(),
            on_chain.size
        )));
    }
    if local.root_at(on_chain.size).map_err(memory_error)? != on_chain.root {
        return Err(CliError::Usage(
            "the local store does not extend the head the ledger holds".into(),
        ));
    }
    let head = local.head(local.len()).map_err(memory_error)?;
    let proof = local
        .consistency_proof(on_chain.size)
        .map_err(memory_error)?;
    let reference = session.reference(log)?;
    // The manifest names the log's latest archive; a commit carries it
    // forward.
    session.send(
        wallet,
        TxKind::CommitMemory {
            size: head.size,
            root: head.root,
            index: head.index_root,
            manifest: on_chain.manifest,
            proof,
            seal,
        },
        vec![reference],
    )
}

/// Names `manifest` as the storage root of `log`'s latest archive: a
/// commit at the size the ledger holds, which keeps its root and index.
pub fn set_manifest(
    session: &mut Session<'_>,
    ledger: &dyn Ledger,
    wallet: &str,
    log: &ObjectId,
    manifest: Hash32,
) -> Result<sion_node::TxView, CliError> {
    let on_chain = committed(ledger, log)?;
    let reference = session.reference(log)?;
    session.send(
        wallet,
        TxKind::CommitMemory {
            size: on_chain.size,
            root: on_chain.root,
            index: on_chain.index,
            manifest,
            proof: Vec::new(),
            seal: false,
        },
        vec![reference],
    )
}

/// Recalls from the local store of `log` at its committed size and checks
/// the answer against the ledger's index root, as a reader would.
pub fn recall(
    ledger: &dyn Ledger,
    log: &ObjectId,
    frame: &str,
    bbox: Box3,
    time: TimeRange,
) -> Result<Json, CliError> {
    let state = committed(ledger, log)?;
    let store = LogStore::open(store_dir(log)).map_err(memory_error)?;
    let local = store.log();
    if local.root_at(state.size).map_err(memory_error)? != state.root {
        return Err(CliError::Usage(
            "the local store does not match the head the ledger holds".into(),
        ));
    }
    let query = RangeQuery::new(FrameId::named(frame), bbox, time);
    let index = local.index(state.size).map_err(memory_error)?;
    let (results, proof) = prove_range(&index, &query).map_err(memory_error)?;
    verify_range(&state.index, &query, &results, &proof).map_err(memory_error)?;
    let atoms: Vec<Json> = results
        .iter()
        .filter_map(|r| local.atom_by_id(&r.id))
        .map(describe_atom)
        .collect();
    Ok(json!({
        "log": log,
        "as_of": { "size": state.size, "index": state.index, "pulse": state.committed_at },
        "verified": "complete against the index root the ledger holds",
        "proof_bytes": encode(&proof).len(),
        "atoms": atoms,
    }))
}

/// A statement in words: `pallet-17 at aisle-4`.
pub fn statement_text(statement: &Statement) -> String {
    match statement {
        Statement::Claim {
            subject,
            predicate,
            object,
        } => format!("{subject} {predicate} {}", value_text(object)),
        Statement::Revision { targets, state, .. } => format!(
            "{} atom(s) now {}",
            targets.len(),
            format!("{state:?}").to_lowercase()
        ),
        Statement::Conflict { members, kind } => format!(
            "{} atoms disagree ({})",
            members.len(),
            format!("{kind:?}").to_lowercase()
        ),
        Statement::Erasure { key } => format!("key {} destroyed", key.0.short()),
        Statement::Content(hash) => format!("content {}", hash.short()),
    }
}

pub(crate) fn value_text(value: &Value) -> String {
    match value {
        Value::Entity(name) => name.clone(),
        Value::Text(text) => format!("{text:?}"),
        Value::Integer(n) => n.to_string(),
        Value::Decimal { mantissa, scale } => format!("{mantissa}e-{scale}"),
        Value::Boolean(b) => b.to_string(),
        Value::Bytes(bytes) => format!("{} bytes", bytes.len()),
        Value::Hash(hash) => hash.short(),
    }
}

pub(crate) fn describe_atom(atom: &MemoryAtom) -> Json {
    json!({
        "id": atom.id(),
        "seq": atom.seq,
        "kind": format!("{:?}", atom.kind).to_lowercase(),
        "statement": statement_text(&atom.statement),
        "valid_from_s": atom.valid.start / SECOND,
        "valid_until_s": atom.valid.end.map(|e| e / SECOND),
        "confidence_permille": atom.confidence,
    })
}

/// The ledger's head of a log.
pub fn committed(ledger: &dyn Ledger, log: &ObjectId) -> Result<MemoryLog, CliError> {
    match ledger.object(*log)?.map(|o| o.data) {
        Some(ObjectData::MemoryLog(state)) => Ok(state),
        Some(_) => Err(CliError::Usage(format!("{log} is not a memory log"))),
        None => Err(CliError::Usage(format!("no memory log {log}"))),
    }
}

/// Where a log's local store lives.
pub fn store_dir(log: &ObjectId) -> PathBuf {
    crate::session::wallet_dir()
        .parent()
        .map(|home| home.join("memory"))
        .unwrap_or_else(|| PathBuf::from(".sion/memory"))
        .join(log.0.to_hex())
}

pub(crate) fn open_store(log: &ObjectId, writer: AccountId) -> Result<LogStore, CliError> {
    let store = LogStore::open_or_create(store_dir(log), writer).map_err(memory_error)?;
    if store.log().writer() != writer {
        return Err(CliError::Usage(format!(
            "log {} is written by another account",
            log.0.short()
        )));
    }
    Ok(store)
}

/// A log by full id, or a unique prefix of a log with a local store.
pub fn find_log(_ledger: &dyn Ledger, text: &str) -> Result<ObjectId, CliError> {
    let text = text.trim().to_lowercase();
    if let Ok(hash) = Hash32::from_hex(&text) {
        return Ok(ObjectId::from_hash(hash));
    }
    let root = store_dir(&ObjectId::from_hash(Hash32::ZERO));
    let dir = root.parent().map(PathBuf::from).unwrap_or_default();
    let mut matches = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with(&text) {
                if let Ok(hash) = Hash32::from_hex(&name) {
                    matches.push(ObjectId::from_hash(hash));
                }
            }
        }
    }
    match matches.as_slice() {
        [one] => Ok(*one),
        [] => Err(CliError::Usage(format!(
            "no local memory log starts with {text:?}; give its full id"
        ))),
        _ => Err(CliError::Usage(format!(
            "{} logs start with {text:?}; give more digits",
            matches.len()
        ))),
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

fn parse_hash(text: &str, what: &str) -> Result<Hash32, CliError> {
    Hash32::from_hex(text.trim()).map_err(|_| CliError::Usage(format!("bad {what} id {text:?}")))
}

/// `"12000,3500,0"` → `[12000, 3500, 0]`.
pub fn parse_point(text: &str) -> Result<[i64; 3], CliError> {
    let parts: Vec<&str> = text.split(',').map(str::trim).collect();
    let bad = || CliError::Usage(format!("{text:?} is not x,y,z in millimetres"));
    if parts.len() != 3 {
        return Err(bad());
    }
    let mut point = [0i64; 3];
    for (slot, part) in point.iter_mut().zip(parts) {
        *slot = part.parse().map_err(|_| bad())?;
    }
    Ok(point)
}

/// true/false, an integer, `@name` for an entity, or text.
pub fn parse_value(text: &str) -> Value {
    match text {
        "true" => Value::Boolean(true),
        "false" => Value::Boolean(false),
        _ => {
            if let Some(name) = text.strip_prefix('@') {
                Value::Entity(name.to_string())
            } else if let Ok(n) = text.parse::<i128>() {
                Value::Integer(n)
            } else {
                Value::Text(text.to_string())
            }
        }
    }
}

/// A memory kind by name: observation, coverage, world-state, …
pub fn parse_kind(text: &str) -> Result<MemoryKind, CliError> {
    let wanted = text.trim().to_lowercase().replace(['-', '_', ' '], "");
    MemoryKind::ALL
        .iter()
        .copied()
        .find(|kind| format!("{kind:?}").to_lowercase() == wanted)
        .ok_or_else(|| {
            CliError::Usage(format!(
                "unknown kind {text:?}; one of {}",
                MemoryKind::ALL
                    .iter()
                    .map(|k| format!("{k:?}").to_lowercase())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        })
}

/// The class a recorded atom of `kind` is filed under: what sensors
/// capture is an observation; anything else the writer asserts.
fn class_of(kind: MemoryKind) -> AtomClass {
    match kind {
        MemoryKind::Observation
        | MemoryKind::Coverage
        | MemoryKind::MapTile
        | MemoryKind::Pose
        | MemoryKind::Trajectory => AtomClass::Observation,
        _ => AtomClass::Assertion,
    }
}

pub(crate) fn memory_error(error: sion_memory::MemoryError) -> CliError {
    CliError::Usage(format!("memory: {error}"))
}
