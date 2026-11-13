//! A flight recorder for an agent that acts in the world.
//!
//! When a machine an agent drives does harm, what happened is argued from
//! its logs, and the logs are its operator's. A recorder makes them
//! something an investigator can hold the operator to:
//!
//! - **Events** a device reports (a sensor's reading, a command sent, a
//!   decision taken, a fault, a heartbeat) are appended to a memory log
//!   ([`sion_memory`]) in the order they arrive, each with the device's
//!   own time. Recording needs no network: the log is a file on the
//!   device.
//! - **Commits** put the log's head on the ledger, which keeps the log
//!   append-only and gives every record a Pulse by which it was fixed.
//!   What was recorded while the device was out of reach is fixed by the
//!   next commit, and until then is only the device's.
//! - **An extract** ([`Extract`]) hands an investigator a run of records,
//!   every one of them, each proven to be in the log the ledger holds.
//!   Records of channels the operator withholds are there as digests: the
//!   investigator sees that they exist and where, not what they say.
//! - **Sealed readings** are logged as commitments. The operator can
//!   prove a bound on every one of them in an extract (no reading above a
//!   limit) without showing the readings ([`Bound`]).
//! - **What is missing shows.** A stretch with no record, not even a
//!   heartbeat, and a device clock that stepped back are read off the
//!   records' own times ([`findings`]).
//!
//! A recorder shows what was recorded and when it was fixed. It cannot
//! show that what was recorded is what happened.

use std::collections::BTreeMap;
use std::io::BufRead;
use std::path::{Path, PathBuf};

use clap::{Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};
use sion_core::hash::Hash32;
use sion_core::ids::{AccountId, ObjectId, Pulse};
use sion_core::merkle;
use sion_exec::execute::Event as LedgerEvent;
use sion_memory::{
    AtomClass, Interval, LogStore, MemoryAtom, MemoryAtomId, MemoryKind, Statement, Value,
};
use sion_zk::{Commitment, Opening, Proof};

use crate::error::CliError;
use crate::ledger::{Ledger, LedgerExt};
use crate::memory::{committed, memory_error, open_store, store_dir};
use crate::session::Session;

/// Nanoseconds in a millisecond.
const MS: u64 = 1_000_000;

/// How a sealed reading is written in a record: this, then its
/// commitment in hex.
const SEALED: &str = "sealed:";

/// The most records an extract holds.
pub const MAX_EXTRACT: u64 = 20_000;

/// What kind of thing a device reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// What a sensor read.
    Sensor,
    /// A command sent to an actuator.
    Command,
    /// A decision the agent took.
    Decision,
    /// A fault the device noticed.
    Fault,
    /// Nothing but that the recorder is alive.
    Heartbeat,
}

impl Kind {
    fn word(self) -> &'static str {
        match self {
            Kind::Sensor => "sensor",
            Kind::Command => "command",
            Kind::Decision => "decision",
            Kind::Fault => "fault",
            Kind::Heartbeat => "heartbeat",
        }
    }

    fn of(word: &str) -> Option<Kind> {
        [
            Kind::Sensor,
            Kind::Command,
            Kind::Decision,
            Kind::Fault,
            Kind::Heartbeat,
        ]
        .into_iter()
        .find(|kind| kind.word() == word)
    }
}

/// What a device reports: a line of its stream.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    /// The device's time, in milliseconds since the Unix epoch.
    pub t_ms: u64,
    /// What kind of thing it is.
    pub kind: Kind,
    /// Where it comes from or goes to (`arm/joint-3`). Not needed for a
    /// heartbeat.
    #[serde(default)]
    pub channel: String,
    /// What it is (`torque_mnm`, `move_to`). Not needed for a heartbeat.
    #[serde(default)]
    pub name: String,
    /// Its value: a whole number or text. None for a heartbeat.
    #[serde(default)]
    pub value: Option<serde_json::Value>,
}

/// A commit of the log, as the recorder noted it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Committed {
    /// The Pulse it executed in.
    pub pulse: u64,
    /// The log's size after it.
    pub size: u64,
}

/// A recorder on this machine.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    /// Its name.
    pub name: String,
    /// Its log on the ledger.
    pub log: ObjectId,
    /// The wallet that writes it.
    pub wallet: String,
    /// That wallet's account.
    pub writer: AccountId,
    /// How often the device is expected to record something, a heartbeat
    /// if nothing else, in milliseconds.
    pub heartbeat_ms: u64,
    /// The channels whose readings are logged as commitments: a channel
    /// is sealed if it is one of these or under one of them.
    #[serde(default)]
    pub sealed: Vec<String>,
    /// The commits made from this machine.
    #[serde(default)]
    pub commits: Vec<Committed>,
    /// Whether the log was sealed: it takes no more commits.
    #[serde(default)]
    pub closed: bool,
}

fn dir() -> PathBuf {
    crate::setup::home().join("recorders")
}

fn settings_path(name: &str) -> PathBuf {
    dir().join(format!("{name}.json"))
}

fn openings_path(name: &str) -> PathBuf {
    dir().join(format!("{name}.openings.json"))
}

fn named(name: &str) -> Result<(), CliError> {
    let fits = !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if fits {
        Ok(())
    } else {
        Err(CliError::Usage(format!(
            "{name:?} is not a recorder's name: lowercase letters, digits and hyphens"
        )))
    }
}

/// The recorder `name` on this machine.
pub fn load(name: &str) -> Result<Settings, CliError> {
    named(name)?;
    let path = settings_path(name);
    let text = std::fs::read_to_string(&path)
        .map_err(|_| CliError::Usage(format!("no recorder {name} on this machine")))?;
    serde_json::from_str(&text).map_err(|e| CliError::Usage(format!("{}: {e}", path.display())))
}

fn save(settings: &Settings) -> Result<(), CliError> {
    crate::profiles::write_json(&settings_path(&settings.name), settings)
}

fn openings(name: &str) -> BTreeMap<u64, Opening> {
    std::fs::read_to_string(openings_path(name))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn under(channel: &str, these: &[String]) -> bool {
    these.iter().any(|prefix| {
        channel == prefix
            || channel
                .strip_prefix(prefix.as_str())
                .is_some_and(|rest| rest.starts_with('/'))
    })
}

/// What a record says its value is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reading {
    /// Nothing (a heartbeat).
    None,
    /// A whole number.
    Number(i128),
    /// Text.
    Text(String),
    /// A commitment to a whole number that is not shown.
    Sealed(Commitment),
}

/// A record, read back from the log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    /// Its place in the log.
    pub seq: u64,
    /// The device's time, in milliseconds since the Unix epoch.
    pub t_ms: u64,
    /// Its kind.
    pub kind: Kind,
    /// Its channel.
    pub channel: String,
    /// Its name.
    pub name: String,
    /// Its value.
    pub reading: Reading,
}

impl Record {
    /// The record `atom` holds, if it is a recorder's.
    pub fn of(atom: &MemoryAtom) -> Option<Record> {
        let Statement::Claim {
            subject,
            predicate,
            object,
        } = &atom.statement
        else {
            return None;
        };
        let (kind, name) = predicate.split_once(':')?;
        let reading = match object {
            Value::Integer(number) => Reading::Number(*number),
            Value::Text(text) => match text.strip_prefix(SEALED) {
                Some(hex) => Reading::Sealed(Commitment::from_hex(hex).ok()?),
                None => Reading::Text(text.clone()),
            },
            Value::Boolean(_) => Reading::None,
            _ => return None,
        };
        Some(Record {
            seq: atom.seq,
            t_ms: atom.valid.start / MS,
            kind: Kind::of(kind)?,
            channel: subject.clone(),
            name: name.to_string(),
            reading,
        })
    }

    /// The record in a line.
    pub fn line(&self) -> String {
        let what = match &self.reading {
            Reading::None => String::new(),
            Reading::Number(number) => format!(" = {number}"),
            Reading::Text(text) => format!(" = {text:?}"),
            Reading::Sealed(_) => " = (sealed)".to_string(),
        };
        if self.kind == Kind::Heartbeat {
            return format!("{} {} heartbeat", self.seq, at(self.t_ms));
        }
        format!(
            "{} {} {} {} {}{what}",
            self.seq,
            at(self.t_ms),
            self.kind.word(),
            self.channel,
            self.name
        )
    }
}

/// A device time in words: seconds since the Unix epoch, to the
/// millisecond.
fn at(t_ms: u64) -> String {
    format!("t={}.{:03}", t_ms / 1_000, t_ms % 1_000)
}

/// A span of milliseconds in words.
fn span(ms: u64) -> String {
    format!("{}.{:03} s", ms / 1_000, ms % 1_000)
}

/// The atom that records `event` at place `seq` of `writer`'s log, and
/// what opens its reading if it is sealed.
fn atom_of(
    writer: AccountId,
    seq: u64,
    event: &Event,
    seal: bool,
    now_ns: u64,
) -> Result<(MemoryAtom, Option<Opening>), CliError> {
    let heartbeat = event.kind == Kind::Heartbeat;
    if !heartbeat && (event.channel.trim().is_empty() || event.name.trim().is_empty()) {
        return Err(CliError::Usage(format!(
            "an event at {} names no channel or no name",
            at(event.t_ms)
        )));
    }
    if event.name.contains(':') || event.channel.len() > 200 || event.name.len() > 200 {
        return Err(CliError::Usage(format!(
            "an event's channel and name are at most 200 bytes, and a name holds no colon \
             ({:?})",
            event.name
        )));
    }
    let (object, opening) = match &event.value {
        None | Some(serde_json::Value::Null) => (Value::Boolean(true), None),
        Some(serde_json::Value::Number(number)) if seal => {
            let Some(value) = number.as_u64() else {
                // Its value is not said: the channel is sealed.
                return Err(CliError::Usage(format!(
                    "a sealed reading is a whole number that is not negative (on {})",
                    event.channel
                )));
            };
            let (commitment, opening) = sion_zk::commit(value);
            (
                Value::Text(format!("{SEALED}{}", commitment.to_hex())),
                Some(opening),
            )
        }
        Some(serde_json::Value::Number(number)) => match number.as_i64() {
            Some(value) => (Value::Integer(i128::from(value)), None),
            None => {
                return Err(CliError::Usage(format!(
                    "a reading is a whole number or text: {number} on {} (scale it on the \
                     device: millimetres, not metres)",
                    event.channel
                )))
            }
        },
        Some(serde_json::Value::String(text)) if text.len() <= 2_000 => {
            // Text that would read back as a sealed reading is not taken
            // for one.
            let text = if text.starts_with(SEALED) {
                format!(" {text}")
            } else {
                text.clone()
            };
            (Value::Text(text), None)
        }
        Some(other) => {
            return Err(CliError::Usage(format!(
                "a reading is a whole number or text of at most 2000 bytes, not {other}"
            )))
        }
    };
    let (channel, name) = if heartbeat && event.channel.is_empty() {
        ("recorder".to_string(), "alive".to_string())
    } else {
        (event.channel.clone(), event.name.clone())
    };
    let start = event.t_ms.saturating_mul(MS);
    let atom = MemoryAtom::builder(
        writer,
        seq,
        MemoryKind::Observation,
        AtomClass::Observation,
        Statement::claim(channel, format!("{}:{name}", event.kind.word()), object),
    )
    .valid(Interval::closed(start, start.saturating_add(MS)))
    .observed_at(now_ns)
    .build()
    .map_err(|e| CliError::Usage(format!("the event at {}: {e}", at(event.t_ms))))?;
    Ok((atom, opening))
}

fn now_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64)
}

/// The channel and name of the record a recorder writes in place of an
/// event it could not record as it came.
pub const UNREADABLE: (&str, &str) = ("recorder", "unreadable");

/// The fault recorded in place of something that could not be: when, and
/// why.
fn unreadable(t_ms: u64, why: &str) -> Event {
    Event {
        t_ms,
        kind: Kind::Fault,
        channel: UNREADABLE.0.into(),
        name: UNREADABLE.1.into(),
        value: Some(serde_json::Value::String(why.chars().take(300).collect())),
    }
}

/// Appends `events` to the recorder's log, in order, on this machine. A
/// recorder drops nothing: an event that cannot be recorded as it came
/// is recorded as a fault that says why, at its time. Returns the place
/// of the first record, how many were made, and how many of them are
/// such faults.
pub fn record(settings: &Settings, events: &[Event]) -> Result<(u64, u64, u64), CliError> {
    let mut store = open_store(&settings.log, settings.writer)?;
    let first = store.log().len();
    let mut kept = openings(&settings.name);
    let (mut sealed, mut faults) = (false, 0);
    for event in events {
        let seq = store.log().len();
        let seal = event.kind == Kind::Sensor && under(&event.channel, &settings.sealed);
        let (atom, opening) = match atom_of(settings.writer, seq, event, seal, now_ns()) {
            Ok(made) => made,
            Err(why) => {
                faults += 1;
                let fault = unreadable(event.t_ms, &why.to_string());
                atom_of(settings.writer, seq, &fault, false, now_ns())?
            }
        };
        // A line that was not an event reaches here as such a fault.
        if event.kind == Kind::Fault && (event.channel.as_str(), event.name.as_str()) == UNREADABLE
        {
            faults += 1;
        }
        if let Some(opening) = opening {
            kept.insert(seq, opening);
            sealed = true;
        }
        store.append(atom).map_err(memory_error)?;
    }
    if sealed {
        crate::profiles::write_json(&openings_path(&settings.name), &kept)?;
    }
    Ok((first, store.log().len() - first, faults))
}

/// Every record of the recorder's log on this machine.
pub fn records(settings: &Settings) -> Result<Vec<Record>, CliError> {
    let store = LogStore::open(store_dir(&settings.log)).map_err(memory_error)?;
    Ok(store.log().atoms().iter().filter_map(Record::of).collect())
}

/// Commits the recorder's log to the ledger and notes the commit.
pub fn commit(
    session: &mut Session<'_>,
    ledger: &dyn Ledger,
    settings: &mut Settings,
    seal: bool,
) -> Result<Committed, CliError> {
    let view = crate::memory::commit(session, ledger, &settings.wallet, &settings.log, seal)?;
    let size = view
        .effects
        .events
        .iter()
        .find_map(|event| match event {
            LedgerEvent::MemoryCommitted { size, .. } => Some(*size),
            _ => None,
        })
        .ok_or_else(|| {
            CliError::Transaction(format!(
                "the ledger did not take the commit: {:?}",
                view.effects.status
            ))
        })?;
    let made = Committed {
        pulse: view.pulse.0,
        size,
    };
    settings.commits.push(made);
    settings.closed |= seal;
    save(settings)?;
    Ok(made)
}

/// A record as someone reading a run of them sees it: shown, or only
/// known to be there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Seen {
    /// Shown.
    Shown(Record),
    /// Withheld: its place is known, and nothing else.
    Withheld(u64),
}

/// What a run of records shows to be missing or wrong, from their own
/// times: stretches longer than twice the heartbeat with nothing
/// recorded, and a device clock that stepped back. Across a withheld
/// record nothing is said: its time is not known.
pub fn findings(seen: &[Seen], heartbeat_ms: u64) -> Vec<String> {
    let mut lines = Vec::new();
    let mut previous: Option<&Record> = None;
    for item in seen {
        let record = match item {
            Seen::Shown(record) => record,
            Seen::Withheld(_) => {
                previous = None;
                continue;
            }
        };
        if let Some(before) = previous {
            if record.t_ms < before.t_ms {
                lines.push(format!(
                    "the device's clock stepped back {} at record {}",
                    span(before.t_ms - record.t_ms),
                    record.seq
                ));
            } else if heartbeat_ms > 0 && record.t_ms - before.t_ms > heartbeat_ms * 2 {
                lines.push(format!(
                    "silence: nothing recorded for {}, from {} (record {}) to {} (record {})",
                    span(record.t_ms - before.t_ms),
                    at(before.t_ms),
                    before.seq,
                    at(record.t_ms),
                    record.seq
                ));
            }
        }
        previous = Some(record);
    }
    lines
}

/// When the records from place `first` to `last` were fixed, in lines:
/// which commit took each run of them, as the recorder noted, and which
/// are not committed at all.
fn fixing(commits: &[Committed], first: u64, last: u64) -> Vec<String> {
    let mut lines = Vec::new();
    let mut from = 0;
    for commit in commits {
        let (a, b) = (from.max(first), commit.size.saturating_sub(1).min(last));
        if commit.size > from && a <= b {
            lines.push(format!(
                "  records {a} to {b} were fixed at Pulse {}",
                commit.pulse
            ));
        }
        from = from.max(commit.size);
    }
    if from <= last {
        lines.push(format!(
            "  records {} to {last} are in no commit this recorder noted",
            from.max(first)
        ));
    }
    lines
}

/// A record in an extract: shown whole, or as its digest alone.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Row {
    /// The record, if it is shown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub atom: Option<MemoryAtom>,
    /// Its digest, if it is withheld.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<MemoryAtomId>,
    /// The path that shows it is at its place in the log.
    pub path: Vec<Hash32>,
}

/// A bound proven of every sealed reading of one kind in an extract.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bound {
    /// The channel.
    pub channel: String,
    /// The name.
    pub name: String,
    /// What every reading is at most, if that is proven.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at_most: Option<u64>,
    /// What every reading is at least, if that is proven.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at_least: Option<u64>,
    /// The proof.
    pub proof: Proof,
}

/// A run of a recorder's records, for an investigator.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Extract {
    /// The format's version.
    pub version: u32,
    /// The recorder's name.
    pub recorder: String,
    /// Its log.
    pub log: ObjectId,
    /// Who writes it.
    pub writer: AccountId,
    /// The log's size when the extract was made, as the ledger held it.
    pub size: u64,
    /// Its root then.
    pub root: Hash32,
    /// The Pulse of the commit that made it so.
    pub committed_at: u64,
    /// How often the device is expected to record something.
    pub heartbeat_ms: u64,
    /// The place of the first record.
    pub first: u64,
    /// The records, each at the place after the one before.
    pub rows: Vec<Row>,
    /// The commits the recorder noted.
    pub commits: Vec<Committed>,
    /// Bounds proven of its sealed readings.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bounds: Vec<Bound>,
}

impl Extract {
    /// The place of the last record.
    pub fn last(&self) -> u64 {
        self.first + (self.rows.len() as u64).saturating_sub(1)
    }

    /// The records as a reader sees them.
    pub fn seen(&self) -> Vec<Seen> {
        self.rows
            .iter()
            .enumerate()
            .map(|(at, row)| {
                let seq = self.first + at as u64;
                match row.atom.as_ref().and_then(Record::of) {
                    Some(record) => Seen::Shown(record),
                    None => Seen::Withheld(seq),
                }
            })
            .collect()
    }

    /// The commitments of the sealed readings of `channel` and `name`
    /// that are shown, with their places.
    fn sealed(&self, channel: &str, name: &str) -> Vec<(u64, Commitment)> {
        self.seen()
            .into_iter()
            .filter_map(|seen| match seen {
                Seen::Shown(Record {
                    seq,
                    channel: c,
                    name: n,
                    reading: Reading::Sealed(commitment),
                    ..
                }) if c == channel && n == name => Some((seq, commitment)),
                _ => None,
            })
            .collect()
    }

    /// What a bound's proof is bound to: this log, these records, this
    /// bound.
    fn context(&self, bound: &Bound) -> Vec<u8> {
        format!(
            "sion-cli/recorder/bound/v1|{}|{}|{}|{}|{}|{}|{:?}|{:?}",
            self.log.0.to_hex(),
            self.root.to_hex(),
            self.first,
            self.last(),
            bound.channel,
            bound.name,
            bound.at_most,
            bound.at_least
        )
        .into_bytes()
    }
}

fn statements(
    count: usize,
    at_most: Option<u64>,
    at_least: Option<u64>,
) -> Vec<sion_zk::Statement> {
    let mut statements = Vec::new();
    for at in 0..count {
        if let Some(most) = at_most {
            statements.push(sion_zk::Statement::at_most(at, most));
        }
        if let Some(least) = at_least {
            statements.push(sion_zk::Statement::at_least(at, least));
        }
    }
    statements
}

/// Makes an extract of the committed records whose device time is from
/// `from_ms` and before `until_ms`: the run from the first such record to
/// the last, every record between them. Records of the channels in
/// `withhold` are given as digests; heartbeats and sealed readings are
/// always shown.
pub fn extract(
    ledger: &dyn Ledger,
    settings: &Settings,
    from_ms: u64,
    until_ms: u64,
    withhold: &[String],
) -> Result<Extract, CliError> {
    let state = committed(ledger, &settings.log)?;
    let store = LogStore::open(store_dir(&settings.log)).map_err(memory_error)?;
    let local = store.log();
    if local.len() < state.size || local.root_at(state.size).map_err(memory_error)? != state.root {
        return Err(CliError::Usage(
            "the log on this machine is not the one the ledger holds".into(),
        ));
    }
    let atoms = &local.atoms()[..state.size as usize];
    let inside = |atom: &MemoryAtom| {
        let t_ms = atom.valid.start / MS;
        t_ms >= from_ms && t_ms < until_ms
    };
    let (Some(first), Some(last)) = (
        atoms.iter().position(inside),
        atoms.iter().rposition(inside),
    ) else {
        return Err(CliError::Usage(format!(
            "no committed record has a device time from {} and before {} ({} record(s) on this \
             machine are not committed)",
            at(from_ms),
            at(until_ms),
            local.len() - state.size
        )));
    };
    if (last - first) as u64 >= MAX_EXTRACT {
        return Err(CliError::Usage(format!(
            "that is {} records: an extract holds at most {MAX_EXTRACT}",
            last - first + 1
        )));
    }
    let mut rows = Vec::new();
    for (seq, atom) in atoms.iter().enumerate().take(last + 1).skip(first) {
        let path = local
            .inclusion_proof_at(seq as u64, state.size)
            .map_err(memory_error)?;
        let held = Record::of(atom).is_some_and(|record| {
            record.kind != Kind::Heartbeat
                && !matches!(record.reading, Reading::Sealed(_))
                && under(&record.channel, withhold)
        });
        rows.push(if held {
            Row {
                atom: None,
                id: Some(atom.id()),
                path,
            }
        } else {
            Row {
                atom: Some(atom.clone()),
                id: None,
                path,
            }
        });
    }
    Ok(Extract {
        version: 1,
        recorder: settings.name.clone(),
        log: settings.log,
        writer: settings.writer,
        size: state.size,
        root: state.root,
        committed_at: state.committed_at.0,
        heartbeat_ms: settings.heartbeat_ms,
        first: first as u64,
        rows,
        commits: settings.commits.clone(),
        bounds: Vec::new(),
    })
}

/// Proves, of every sealed reading of `channel` and `name` shown in
/// `extract`, that it is at most `at_most` and at least `at_least`, with
/// the openings this machine keeps, and adds the proof to the extract.
/// There is no proof of readings that break the bound.
pub fn prove(
    settings: &Settings,
    extract: &mut Extract,
    channel: &str,
    name: &str,
    at_most: Option<u64>,
    at_least: Option<u64>,
) -> Result<usize, CliError> {
    if at_most.is_none() && at_least.is_none() {
        return Err(CliError::Usage(
            "a bound is --at-most, --at-least or both".into(),
        ));
    }
    let sealed = extract.sealed(channel, name);
    if sealed.is_empty() {
        return Err(CliError::Usage(format!(
            "the extract shows no sealed reading of {channel} {name}"
        )));
    }
    let kept = openings(&settings.name);
    let mut opened = Vec::new();
    for (seq, commitment) in &sealed {
        match kept.get(seq) {
            Some(opening) if opening.commitment() == *commitment => opened.push(opening.clone()),
            _ => {
                return Err(CliError::Usage(format!(
                    "this machine does not hold what opens the reading at record {seq}"
                )))
            }
        }
    }
    let mut bound = Bound {
        channel: channel.to_string(),
        name: name.to_string(),
        at_most,
        at_least,
        proof: Proof { ranges: Vec::new() },
    };
    bound.proof = sion_zk::prove(
        &extract.context(&bound),
        &opened,
        &statements(opened.len(), at_most, at_least),
    )
    .map_err(|e| {
        CliError::Usage(format!(
            "no proof of that bound: {e}. A bound that a reading breaks cannot be proven, and \
             one proof holds at most {} readings and bounds together",
            sion_zk::MAX_VALUES
        ))
    })?;
    extract
        .bounds
        .retain(|had| !(had.channel == bound.channel && had.name == bound.name));
    extract.bounds.push(bound);
    Ok(opened.len())
}

/// Checks `extract` against the ledger: a line for each thing looked at,
/// and whether everything that can be checked holds.
pub fn check(ledger: &dyn Ledger, extract: &Extract) -> Result<(Vec<String>, bool), CliError> {
    let mut lines = Vec::new();
    let mut holds = true;
    let state = committed(ledger, &extract.log)?;
    if state.writer != extract.writer {
        return Ok((
            vec!["differs: the log is written by another account than the extract names".into()],
            false,
        ));
    }
    let current = state.size == extract.size && state.root == extract.root;
    if current {
        lines.push(format!(
            "holds: the extract is of the log as the ledger holds it: {} record(s), committed \
             at Pulse {}{}",
            state.size,
            state.committed_at.0,
            if state.sealed {
                ", and sealed: it takes no more"
            } else {
                ""
            }
        ));
    } else {
        holds = false;
        lines.push(format!(
            "differs: the ledger holds the log at {} record(s), committed at Pulse {}, and the \
             extract was made at {}: have it made again",
            state.size, state.committed_at.0, extract.size
        ));
    }
    if extract.rows.is_empty() || extract.last() >= extract.size {
        return Ok((
            vec!["differs: the extract's records are not within the log it names".into()],
            false,
        ));
    }
    let mut broken = 0;
    for (place, row) in extract.rows.iter().enumerate() {
        let seq = extract.first + place as u64;
        let leaf = match (&row.atom, &row.id) {
            (Some(atom), None) if atom.seq == seq && atom.writer == extract.writer => atom.leaf(),
            (None, Some(id)) => id.leaf(),
            _ => {
                broken += 1;
                continue;
            }
        };
        if merkle::verify_inclusion(&extract.root, extract.size, seq, &leaf, &row.path).is_err() {
            broken += 1;
        }
    }
    let seen = extract.seen();
    let withheld = seen
        .iter()
        .filter(|s| matches!(s, Seen::Withheld(_)))
        .count();
    if broken == 0 {
        lines.push(format!(
            "holds: records {} to {}, all {} of them, are in that log in this order ({} shown, \
             {withheld} withheld)",
            extract.first,
            extract.last(),
            extract.rows.len(),
            extract.rows.len() - withheld
        ));
    } else {
        holds = false;
        lines.push(format!(
            "differs: {broken} of the {} record(s) are not at their place in that log",
            extract.rows.len()
        ));
    }
    let shown: Vec<&Record> = seen
        .iter()
        .filter_map(|s| match s {
            Seen::Shown(record) => Some(record),
            Seen::Withheld(_) => None,
        })
        .collect();
    if let (Some(first), Some(last)) = (shown.first(), shown.last()) {
        lines.push(format!(
            "  the records shown run from {} to {} by the device's clock",
            at(first.t_ms),
            at(last.t_ms)
        ));
    }
    // When they were fixed: the head's Pulse is the ledger's; the batches
    // before it are the recorder's note, confirmed where the node still
    // holds the commit's event.
    if current {
        lines.push(format!(
            "  every one of them was fixed no later than Pulse {}",
            state.committed_at.0
        ));
    }
    let mut from = 0;
    for commit in &extract.commits {
        let (a, b) = (
            from.max(extract.first),
            commit.size.saturating_sub(1).min(extract.last()),
        );
        if commit.size > from && a <= b {
            let confirmed = ledger
                .events(Pulse(commit.pulse), 5_000)
                .map(|page| {
                    page.iter().any(|view| {
                        view.pulse.0 == commit.pulse
                            && matches!(&view.event, LedgerEvent::MemoryCommitted { log, size, .. }
                                if *log == extract.log && *size == commit.size)
                    })
                })
                .unwrap_or(false);
            lines.push(format!(
                "  records {a} to {b} were fixed at Pulse {} ({})",
                commit.pulse,
                if confirmed {
                    "the ledger's events confirm it"
                } else {
                    "the recorder's note: this node no longer holds that Pulse's events"
                }
            ));
        }
        from = from.max(commit.size);
    }
    let found = findings(&seen, extract.heartbeat_ms);
    if found.is_empty() {
        lines.push(format!(
            "  no stretch longer than {} without a record, and the device's clock never stepped \
             back{}",
            span(extract.heartbeat_ms * 2),
            if withheld > 0 {
                ", among the records shown"
            } else {
                ""
            }
        ));
    }
    for line in found {
        lines.push(format!("  {line}"));
    }
    for bound in &extract.bounds {
        let sealed = extract.sealed(&bound.channel, &bound.name);
        let commitments: Vec<Commitment> = sealed.iter().map(|(_, c)| *c).collect();
        let said = match (bound.at_most, bound.at_least) {
            (Some(most), Some(least)) => format!("from {least} to {most}"),
            (Some(most), None) => format!("at most {most}"),
            (None, Some(least)) => format!("at least {least}"),
            (None, None) => "bound by nothing".to_string(),
        };
        let proven = !commitments.is_empty()
            && sion_zk::verify(
                &extract.context(bound),
                &commitments,
                &statements(commitments.len(), bound.at_most, bound.at_least),
                &bound.proof,
            )
            .is_ok();
        if proven {
            lines.push(format!(
                "holds, proven without the readings: every one of the {} sealed reading(s) of \
                 {} {} in these records is {said}",
                commitments.len(),
                bound.channel,
                bound.name
            ));
            if withheld > 0 {
                lines.push(format!(
                    "  ({withheld} record(s) are withheld: whether any is a reading of this \
                     channel is not shown)"
                ));
            }
        } else {
            holds = false;
            lines.push(format!(
                "differs: the proof that every sealed reading of {} {} is {said} does not hold \
                 for these records",
                bound.channel, bound.name
            ));
        }
    }
    Ok((lines, holds))
}

/// Where a recorder stands on this machine, in lines.
pub fn lines(settings: &Settings) -> Result<Vec<String>, CliError> {
    let records = records(settings)?;
    let store = LogStore::open(store_dir(&settings.log)).map_err(memory_error)?;
    let size = store.log().len();
    let fixed = settings.commits.last().map_or(0, |c| c.size);
    let mut lines = vec![format!(
        "recorder {}: {size} record(s) in log {}, {} fixed by a commit, {} only on this machine",
        settings.name,
        settings.log.0.short(),
        fixed.min(size),
        size.saturating_sub(fixed)
    )];
    if let (Some(first), Some(last)) = (records.first(), records.last()) {
        lines.push(format!(
            "  from {} to {} by the device's clock; a record is expected every {}",
            at(first.t_ms),
            at(last.t_ms),
            span(settings.heartbeat_ms)
        ));
        lines.extend(fixing(&settings.commits, 0, size.saturating_sub(1)));
    }
    let seen: Vec<Seen> = records.into_iter().map(Seen::Shown).collect();
    for line in findings(&seen, settings.heartbeat_ms) {
        lines.push(format!("  {line}"));
    }
    Ok(lines)
}

/// `sion agent recorder`.
#[derive(Clone, Debug, Subcommand)]
pub enum RecorderCommand {
    /// Open a recorder: a log on the ledger that this wallet writes, and
    /// its store on this machine.
    Open {
        /// The recorder's name (lowercase letters, digits and hyphens).
        name: String,
        /// The wallet that writes it.
        #[arg(long)]
        wallet: String,
        /// How often the device records something, a heartbeat if nothing
        /// else, in milliseconds: a stretch of more than twice this with
        /// no record is reported.
        #[arg(long = "heartbeat-ms", default_value_t = 1_000)]
        heartbeat_ms: u64,
        /// A channel whose sensor readings are logged as commitments, not
        /// shown (it and the channels under it; repeat for more).
        #[arg(long = "seal", value_name = "CHANNEL")]
        sealed: Vec<String>,
    },
    /// Record events on this machine, with no network: lines of JSON,
    /// each {"t_ms": …, "kind": "sensor|command|decision|fault|heartbeat",
    /// "channel": …, "name": …, "value": a whole number or text}.
    Record {
        /// The recorder.
        name: String,
        /// The file of events (standard input, if not given).
        #[arg(long, value_name = "FILE")]
        file: Option<PathBuf>,
    },
    /// Commit the log to the ledger: every record made so far is fixed as
    /// of this Pulse and cannot be changed or left out afterwards.
    Commit {
        /// The recorder.
        name: String,
        /// Close the log for good: after an incident, so that an extract
        /// of it stays checkable.
        #[arg(long)]
        seal: bool,
    },
    /// Where a recorder stands on this machine: how much is recorded and
    /// fixed, stretches with no record, and a clock that stepped back.
    Show {
        /// The recorder.
        name: String,
        /// List the last records too.
        #[arg(long, default_value_t = 0)]
        last: usize,
    },
    /// Make an extract for an investigator: every committed record from
    /// the first to the last whose device time is in a window, each with
    /// the path that shows it is in the log the ledger holds.
    Extract {
        /// The recorder.
        name: String,
        /// The window's start, in milliseconds since the Unix epoch.
        #[arg(long = "from-ms")]
        from_ms: u64,
        /// The window's end (exclusive).
        #[arg(long = "until-ms")]
        until_ms: u64,
        /// A channel whose records are given as digests only (it and the
        /// channels under it; repeat for more).
        #[arg(long, value_name = "CHANNEL")]
        withhold: Vec<String>,
        /// The file to write.
        #[arg(long, value_name = "FILE")]
        out: PathBuf,
    },
    /// Prove a bound on every sealed reading of one kind in an extract,
    /// without showing the readings, and add the proof to the extract.
    Prove {
        /// The recorder.
        name: String,
        /// The extract.
        extract: PathBuf,
        /// The channel.
        #[arg(long)]
        channel: String,
        /// The reading's name.
        #[arg(long = "reading")]
        reading: String,
        /// What every reading is at most.
        #[arg(long = "at-most")]
        at_most: Option<u64>,
        /// What every reading is at least.
        #[arg(long = "at-least")]
        at_least: Option<u64>,
    },
    /// Check an extract against the ledger, as an investigator: that its
    /// records are in the log the ledger holds, all of them and in order,
    /// by when they were fixed, what their own times show to be missing,
    /// and the bounds proven of its sealed readings.
    Check {
        /// The extract.
        extract: PathBuf,
        /// List the records shown.
        #[arg(long)]
        records: bool,
    },
}

impl RecorderCommand {
    /// Whether the command needs a node: recording, looking and proving
    /// do not.
    pub fn needs_ledger(&self) -> bool {
        !matches!(
            self,
            RecorderCommand::Record { .. }
                | RecorderCommand::Show { .. }
                | RecorderCommand::Prove { .. }
        )
    }
}

fn read_extract(path: &Path) -> Result<Extract, CliError> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| CliError::Usage(format!("{}: {e}", path.display())))?;
    serde_json::from_str(&text)
        .map_err(|e| CliError::Usage(format!("{} is not an extract: {e}", path.display())))
}

/// The events in `reader`, a line of JSON each; empty lines are passed
/// over. A line that is not an event becomes a fault that says so, at the
/// time of the event before it (or now, for the first): nothing a device
/// sent is dropped without a trace.
pub fn events(reader: impl BufRead) -> Result<Vec<Event>, CliError> {
    let mut events: Vec<Event> = Vec::new();
    for (number, line) in reader.lines().enumerate() {
        let line = line.map_err(|e| CliError::Usage(format!("line {}: {e}", number + 1)))?;
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str(&line) {
            Ok(event) => events.push(event),
            Err(why) => {
                let t_ms = events.last().map_or(now_ns() / MS, |event| event.t_ms);
                events.push(unreadable(
                    t_ms,
                    &format!("line {} was not an event: {why}", number + 1),
                ));
            }
        }
    }
    Ok(events)
}

/// Runs a recorder command; `session` is there when a node is needed.
pub fn run(
    session: Option<(&mut Session<'_>, &dyn Ledger)>,
    command: RecorderCommand,
) -> Result<(), CliError> {
    let no_node = || CliError::Usage("this needs a node".into());
    match command {
        RecorderCommand::Open {
            name,
            wallet,
            heartbeat_ms,
            sealed,
        } => {
            named(&name)?;
            if settings_path(&name).exists() {
                return Err(CliError::Usage(format!(
                    "this machine already has a recorder {name}"
                )));
            }
            let (session, _) = session.ok_or_else(no_node)?;
            let (_, opened) = crate::memory::open(session, &wallet, None)?;
            let log =
                opened.ok_or_else(|| CliError::Transaction("the ledger opened no log".into()))?;
            let settings = Settings {
                name: name.clone(),
                log,
                wallet: wallet.clone(),
                writer: session.id(&wallet),
                heartbeat_ms,
                sealed,
                commits: Vec::new(),
                closed: false,
            };
            save(&settings)?;
            println!("opened recorder {name}: log {log}, written by {wallet}");
            if !settings.sealed.is_empty() {
                println!(
                    "  sensor readings of {} are logged as commitments; what opens them is kept \
                     in {}",
                    settings.sealed.join(", "),
                    openings_path(&name).display()
                );
            }
            Ok(())
        }
        RecorderCommand::Record { name, file } => {
            let settings = load(&name)?;
            let events = match file {
                Some(path) => events(std::io::BufReader::new(
                    std::fs::File::open(&path)
                        .map_err(|e| CliError::Usage(format!("{}: {e}", path.display())))?,
                ))?,
                None => events(std::io::stdin().lock())?,
            };
            let (first, count, faults) = record(&settings, &events)?;
            match count {
                0 => println!("nothing to record"),
                _ => println!(
                    "recorded {count} event(s) as records {first} to {}: on this machine until \
                     the next commit",
                    first + count - 1
                ),
            }
            if faults > 0 {
                println!(
                    "  {faults} of them could not be recorded as they came: each is recorded as \
                     a fault that says why"
                );
            }
            if settings.closed && count > 0 {
                println!(
                    "  this recorder's log is sealed and takes no more commits: what is \
                     recorded now stays on this machine (open another recorder to go on)"
                );
            }
            Ok(())
        }
        RecorderCommand::Commit { name, seal } => {
            let mut settings = load(&name)?;
            let (session, ledger) = session.ok_or_else(no_node)?;
            let before = settings.commits.last().map_or(0, |c| c.size);
            let made = commit(session, ledger, &mut settings, seal)?;
            println!(
                "committed at Pulse {}: records {before} to {} are fixed{}",
                made.pulse,
                made.size.saturating_sub(1),
                if seal {
                    ", and the log is sealed: it takes no more"
                } else {
                    ""
                }
            );
            Ok(())
        }
        RecorderCommand::Show { name, last } => {
            let settings = load(&name)?;
            for line in lines(&settings)? {
                println!("{line}");
            }
            if last > 0 {
                let records = records(&settings)?;
                for record in &records[records.len().saturating_sub(last)..] {
                    println!("  {}", record.line());
                }
            }
            Ok(())
        }
        RecorderCommand::Extract {
            name,
            from_ms,
            until_ms,
            withhold,
            out,
        } => {
            let settings = load(&name)?;
            let (_, ledger) = session.ok_or_else(no_node)?;
            let made = extract(ledger, &settings, from_ms, until_ms, &withhold)?;
            crate::profiles::write_json(&out, &made)?;
            let withheld = made.rows.iter().filter(|row| row.atom.is_none()).count();
            println!(
                "extract of records {} to {} ({} shown, {withheld} withheld), as of Pulse {}: {}",
                made.first,
                made.last(),
                made.rows.len() - withheld,
                made.committed_at,
                out.display()
            );
            Ok(())
        }
        RecorderCommand::Prove {
            name,
            extract: path,
            channel,
            reading,
            at_most,
            at_least,
        } => {
            let settings = load(&name)?;
            let mut made = read_extract(&path)?;
            let count = prove(&settings, &mut made, &channel, &reading, at_most, at_least)?;
            crate::profiles::write_json(&path, &made)?;
            println!(
                "proved a bound on {count} sealed reading(s) of {channel} {reading}, without \
                 showing them: {}",
                path.display()
            );
            Ok(())
        }
        RecorderCommand::Check {
            extract: path,
            records,
        } => {
            let made = read_extract(&path)?;
            let (_, ledger) = session.ok_or_else(no_node)?;
            let (lines, holds) = check(ledger, &made)?;
            for line in lines {
                println!("{line}");
            }
            if records {
                for seen in made.seen() {
                    match seen {
                        Seen::Shown(record) => println!("  {}", record.line()),
                        Seen::Withheld(seq) => println!("  {seq} (withheld)"),
                    }
                }
            }
            if holds {
                Ok(())
            } else {
                Err(CliError::Verification(
                    "the extract does not check out".into(),
                ))
            }
        }
    }
}
