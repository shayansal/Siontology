//! Durable storage: a segmented write-ahead log of executed Pulses plus
//! periodic state snapshots.
//!
//! The design follows Hotgraph's event log (`docs/ops/wal-snapshot-recovery.md`):
//! append-only segments of self-describing records, each with a magic tag,
//! a sequence number, a length and a checksum; a crash leaves at most one
//! torn record at the tail, which recovery detects and truncates. Snapshots
//! are written to a temporary file and renamed into place, so a snapshot is
//! either complete or absent.
//!
//! Recovery loads the newest valid snapshot and re-executes every logged
//! Pulse after it, checking that each reproduces the logged world
//! commitment, so corruption or a nondeterminism bug cannot pass silently.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use borsh::{BorshDeserialize, BorshSerialize};
use fold_consensus::Beacon;
use fold_core::encode::{decode, encode};
use fold_core::hash::hash_domain;
use fold_core::ids::Pulse;
use fold_exec::pulse::{execute_pulse, PulseInput};
use fold_exec::{State, WorldCommitment};

/// Magic tag at the start of every record.
pub const RECORD_MAGIC: &[u8; 8] = b"FOLDWAL1";
/// Magic tag at the start of every snapshot.
pub const SNAPSHOT_MAGIC: &[u8; 8] = b"FOLDSNP1";
/// Default segment size before rotation.
pub const SEGMENT_BYTES: u64 = 8 * 1024 * 1024;

/// Storage errors.
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    /// I/O.
    #[error("io: {0}")]
    Io(#[from] io::Error),
    /// A record or snapshot failed to decode or verify.
    #[error("corrupt {0}")]
    Corrupt(String),
    /// Replaying the log did not reproduce the logged commitment.
    #[error("replay of Pulse {pulse} diverged: logged {logged:?}, got {got:?}")]
    Divergence {
        /// The Pulse.
        pulse: Pulse,
        /// Commitment in the log.
        logged: Box<WorldCommitment>,
        /// Commitment recomputed.
        got: Box<WorldCommitment>,
    },
}

/// One executed Pulse, as logged.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct PulseRecord {
    /// What was executed.
    pub input: PulseInput,
    /// The resulting commitment.
    pub after: WorldCommitment,
    /// The beacon state after the Pulse closed.
    pub beacon: Beacon,
}

/// A full snapshot.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Snapshot {
    /// The state.
    pub state: State,
    /// The beacon state.
    pub beacon: Beacon,
    /// The consensus epoch in progress when the snapshot was taken.
    pub epoch: u64,
}

fn checksum(bytes: &[u8]) -> [u8; 8] {
    let digest = hash_domain("fold/storage/checksum", bytes);
    let mut out = [0u8; 8];
    out.copy_from_slice(&digest.0[..8]);
    out
}

/// The write-ahead log.
#[derive(Debug)]
pub struct Wal {
    dir: PathBuf,
    segment: Option<(PathBuf, File, u64)>,
    next_sequence: u64,
    segment_bytes: u64,
    sync: bool,
}

/// Recovered contents.
#[derive(Debug)]
pub struct Recovered {
    /// The newest snapshot, if any.
    pub snapshot: Option<Snapshot>,
    /// Logged Pulses after the snapshot, in order.
    pub records: Vec<PulseRecord>,
    /// Bytes truncated from a torn tail.
    pub truncated: u64,
}

impl Wal {
    /// Opens (creating) a log in `dir`, truncating any torn tail, and returns
    /// what it contains.
    pub fn open(dir: &Path, sync: bool) -> Result<(Wal, Recovered), StorageError> {
        fs::create_dir_all(dir)?;
        let snapshot = latest_snapshot(dir)?;
        let floor = snapshot.as_ref().map(|s| s.state.pulse).unwrap_or(Pulse(0));
        let mut segments = list(dir, "wal-", ".log")?;
        segments.sort();
        let mut records = Vec::new();
        let mut next_sequence = 0u64;
        let mut truncated = 0u64;
        let mut last = None;
        for (index, path) in segments.iter().enumerate() {
            let mut bytes = Vec::new();
            File::open(path)?.read_to_end(&mut bytes)?;
            let mut offset = 0usize;
            while offset < bytes.len() {
                match parse_record(&bytes[offset..]) {
                    Some((sequence, payload, used)) if sequence == next_sequence => {
                        let record: PulseRecord = decode(payload)
                            .map_err(|_| StorageError::Corrupt(format!("record {sequence}")))?;
                        if record.input.pulse > floor {
                            records.push(record);
                        }
                        next_sequence += 1;
                        offset += used;
                    }
                    _ => {
                        // A torn or corrupt tail: only acceptable in the last segment.
                        if index + 1 != segments.len() {
                            return Err(StorageError::Corrupt(format!(
                                "{} at offset {offset}",
                                path.display()
                            )));
                        }
                        truncated = (bytes.len() - offset) as u64;
                        let file = OpenOptions::new().write(true).open(path)?;
                        file.set_len(offset as u64)?;
                        file.sync_all()?;
                        break;
                    }
                }
            }
            last = Some(path.clone());
        }
        let segment = match last {
            Some(path) => {
                let file = OpenOptions::new().append(true).open(&path)?;
                let len = file.metadata()?.len();
                Some((path, file, len))
            }
            None => None,
        };
        let wal = Wal {
            dir: dir.to_path_buf(),
            segment,
            next_sequence,
            segment_bytes: SEGMENT_BYTES,
            sync,
        };
        Ok((
            wal,
            Recovered {
                snapshot,
                records,
                truncated,
            },
        ))
    }

    /// Appends one executed Pulse.
    pub fn append(&mut self, record: &PulseRecord) -> Result<(), StorageError> {
        let payload = encode(record);
        let needs_new = match &self.segment {
            None => true,
            Some((_, _, len)) => *len >= self.segment_bytes,
        };
        if needs_new {
            let path = self.dir.join(format!("wal-{:016}.log", self.next_sequence));
            let file = OpenOptions::new().create(true).append(true).open(&path)?;
            self.segment = Some((path, file, 0));
        }
        let (_, file, len) = self.segment.as_mut().expect("segment opened above");
        let mut frame = Vec::with_capacity(payload.len() + 28);
        frame.extend_from_slice(RECORD_MAGIC);
        frame.extend_from_slice(&self.next_sequence.to_be_bytes());
        frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        frame.extend_from_slice(&checksum(&payload));
        frame.extend_from_slice(&payload);
        file.write_all(&frame)?;
        if self.sync {
            file.sync_data()?;
        }
        *len += frame.len() as u64;
        self.next_sequence += 1;
        Ok(())
    }

    /// Writes a snapshot atomically and keeps the two newest.
    pub fn snapshot(&self, snapshot: &Snapshot) -> Result<(), StorageError> {
        let payload = encode(snapshot);
        let name = format!("snapshot-{:016}.bin", snapshot.state.pulse.0);
        let tmp = self.dir.join(format!("{name}.tmp"));
        {
            let mut file = File::create(&tmp)?;
            file.write_all(SNAPSHOT_MAGIC)?;
            file.write_all(&(payload.len() as u64).to_be_bytes())?;
            file.write_all(&checksum(&payload))?;
            file.write_all(&payload)?;
            file.sync_all()?;
        }
        fs::rename(&tmp, self.dir.join(&name))?;
        let mut snapshots = list(&self.dir, "snapshot-", ".bin")?;
        snapshots.sort();
        while snapshots.len() > 2 {
            fs::remove_file(snapshots.remove(0))?;
        }
        Ok(())
    }

    /// Records written so far.
    pub fn len(&self) -> u64 {
        self.next_sequence
    }

    /// Whether nothing was ever written.
    pub fn is_empty(&self) -> bool {
        self.next_sequence == 0
    }
}

fn parse_record(bytes: &[u8]) -> Option<(u64, &[u8], usize)> {
    if bytes.len() < 28 || &bytes[..8] != RECORD_MAGIC {
        return None;
    }
    let sequence = u64::from_be_bytes(bytes[8..16].try_into().ok()?);
    let len = u32::from_be_bytes(bytes[16..20].try_into().ok()?) as usize;
    let sum: [u8; 8] = bytes[20..28].try_into().ok()?;
    let payload = bytes.get(28..28 + len)?;
    (checksum(payload) == sum).then_some((sequence, payload, 28 + len))
}

fn list(dir: &Path, prefix: &str, suffix: &str) -> Result<Vec<PathBuf>, StorageError> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name.starts_with(prefix) && name.ends_with(suffix) {
            out.push(path);
        }
    }
    Ok(out)
}

fn read_snapshot(path: &Path) -> Option<Snapshot> {
    let mut bytes = Vec::new();
    File::open(path).ok()?.read_to_end(&mut bytes).ok()?;
    if bytes.len() < 24 || &bytes[..8] != SNAPSHOT_MAGIC {
        return None;
    }
    let len = u64::from_be_bytes(bytes[8..16].try_into().ok()?) as usize;
    let sum: [u8; 8] = bytes[16..24].try_into().ok()?;
    let payload = bytes.get(24..24 + len)?;
    if checksum(payload) != sum {
        return None;
    }
    let mut snapshot: Snapshot = decode(payload).ok()?;
    // Never trust a stored tree: recompute it from the objects.
    snapshot.state.rebuild_tree();
    Some(snapshot)
}

fn latest_snapshot(dir: &Path) -> Result<Option<Snapshot>, StorageError> {
    let mut snapshots = list(dir, "snapshot-", ".bin")?;
    snapshots.sort();
    for path in snapshots.iter().rev() {
        if let Some(snapshot) = read_snapshot(path) {
            return Ok(Some(snapshot));
        }
    }
    Ok(None)
}

/// Rebuilds the state from a snapshot (or genesis) and the logged Pulses.
pub fn replay(
    mut state: State,
    mut beacon: Beacon,
    records: &[PulseRecord],
) -> Result<(State, Beacon), StorageError> {
    for record in records {
        if record.input.pulse <= state.pulse {
            continue;
        }
        let output = execute_pulse(&mut state, &record.input, true)
            .map_err(|e| StorageError::Corrupt(format!("Pulse {}: {e}", record.input.pulse)))?;
        if output.after != record.after {
            return Err(StorageError::Divergence {
                pulse: record.input.pulse,
                logged: Box::new(record.after),
                got: Box::new(output.after),
            });
        }
        beacon = record.beacon.clone();
    }
    Ok((state, beacon))
}

#[cfg(test)]
mod tests {
    use super::*;
    use fold_core::hash::Hash32;
    use fold_exec::sim::{sim_beacon, Chain};

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fold-storage-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn log_survives_a_torn_tail_and_replays_to_the_same_state() {
        let dir = temp_dir("torn");
        let mut chain = Chain::new(&["a", "b"], 1_000, 2);
        let genesis = chain.state.clone();
        let (mut wal, recovered) = Wal::open(&dir, false).unwrap();
        assert!(recovered.records.is_empty());
        let b = chain.id("b");
        for amount in 1..=3u64 {
            let tx = chain
                .tx(
                    "a",
                    fold_exec::TxKind::Transfer {
                        asset: fold_core::ids::AssetId::usd(),
                        outputs: vec![(b, fold_core::Amount::units(amount))],
                    },
                )
                .unwrap();
            let pulse = chain.state.pulse.next();
            let input = PulseInput {
                pulse,
                beacon: sim_beacon(pulse),
                owned: vec![tx],
                contended: Vec::new(),
            };
            let output = execute_pulse(&mut chain.state, &input, true).unwrap();
            for w in chain.wallets.values_mut() {
                w.settle();
            }
            wal.append(&PulseRecord {
                input,
                after: output.after,
                beacon: Beacon::new(Hash32::ZERO),
            })
            .unwrap();
        }
        drop(wal);
        // Tear the last record.
        let segment = list(&dir, "wal-", ".log").unwrap().pop().unwrap();
        let len = fs::metadata(&segment).unwrap().len();
        OpenOptions::new()
            .write(true)
            .open(&segment)
            .unwrap()
            .set_len(len - 5)
            .unwrap();
        let (wal, recovered) = Wal::open(&dir, false).unwrap();
        assert_eq!(recovered.records.len(), 2);
        assert!(recovered.truncated > 0);
        assert_eq!(wal.len(), 2);
        let (state, _) = replay(
            genesis.clone(),
            Beacon::new(Hash32::ZERO),
            &recovered.records,
        )
        .unwrap();
        assert_eq!(state.pulse, Pulse(2));
        // A snapshot plus the log reproduces the same state.
        wal.snapshot(&Snapshot {
            state: state.clone(),
            beacon: Beacon::new(Hash32::ZERO),
            epoch: 0,
        })
        .unwrap();
        let (_, again) = Wal::open(&dir, false).unwrap();
        assert_eq!(
            again.snapshot.unwrap().state.commitment(),
            state.commitment()
        );
        assert!(again.records.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }
}
