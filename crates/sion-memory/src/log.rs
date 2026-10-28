//! The writer's log: an append-only list of one writer's atoms, its heads,
//! the ledger's commit history, and a simple on-disk store.
//!
//! # The contract with the ledger
//!
//! The log's leaves are `sion_core::merkle::leaf_hash(&atom_id.0 .0)` for
//! each atom in log order, its root is `sion_core::merkle::root(&leaves)`
//! (`empty_root()` when empty), and consistency proofs between sizes are
//! `sion_core::merkle::consistency_proof`'s. A `CommitMemory` transaction
//! moves the ledger's head `{ size, root, index, manifest }` forward after
//! checking such a proof; the index root is [`crate::index`]'s, opaque to
//! the ledger.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use sion_core::encode::{decode, encode};
use sion_core::hash::Hash32;
use sion_core::ids::{AccountId, Pulse};

use crate::atom::{MemoryAtom, MemoryAtomId};
use crate::error::{MemoryError, Result};
use crate::index::MemoryIndex;
use crate::merkle::MerkleCache;
use crate::seal::KeyRing;

/// A writer's head: what it signs and the ledger records (less the storage
/// manifest, which this crate does not define).
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Hash,
    BorshSerialize,
    BorshDeserialize,
    Serialize,
    Deserialize,
)]
pub struct Head {
    /// Atoms in the log.
    pub size: u64,
    /// The log root.
    pub root: Hash32,
    /// The index root over the same atoms.
    pub index_root: Hash32,
}

/// One `CommitMemory` the ledger executed: the Pulse and the new size.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Hash,
    BorshSerialize,
    BorshDeserialize,
    Serialize,
    Deserialize,
)]
pub struct Commit {
    /// The Pulse in which the commit executed.
    pub pulse: Pulse,
    /// The log size it committed.
    pub size: u64,
}

/// A log's commits in ledger order: Pulses and sizes never decrease.
#[derive(
    Clone,
    Debug,
    Default,
    PartialEq,
    Eq,
    Hash,
    BorshSerialize,
    BorshDeserialize,
    Serialize,
    Deserialize,
)]
pub struct CommitHistory {
    commits: Vec<Commit>,
}

impl CommitHistory {
    /// A history, checked for order.
    pub fn new(commits: Vec<Commit>) -> Result<Self> {
        let mut history = CommitHistory::default();
        for commit in commits {
            history.push(commit)?;
        }
        Ok(history)
    }

    /// Records the next commit.
    pub fn push(&mut self, commit: Commit) -> Result<()> {
        if let Some(last) = self.commits.last() {
            if commit.pulse < last.pulse {
                return Err(MemoryError::InvalidCommits("pulses decrease"));
            }
            if commit.size < last.size {
                return Err(MemoryError::InvalidCommits("sizes decrease"));
            }
        }
        self.commits.push(commit);
        Ok(())
    }

    /// The commits.
    pub fn commits(&self) -> &[Commit] {
        &self.commits
    }

    /// The size of the last commit (0 before any).
    pub fn committed_size(&self) -> u64 {
        self.commits.last().map_or(0, |c| c.size)
    }

    /// The history as of a Pulse: the commits executed at or before it.
    pub fn as_of(&self, pulse: Pulse) -> CommitHistory {
        CommitHistory {
            commits: self
                .commits
                .iter()
                .copied()
                .filter(|c| c.pulse <= pulse)
                .collect(),
        }
    }

    /// The transaction time of the atom at `seq`: the Pulse of the first
    /// commit whose size covers it, or `None` if no commit does yet.
    pub fn known_at(&self, seq: u64) -> Option<Pulse> {
        let first = self.commits.partition_point(|c| c.size <= seq);
        self.commits.get(first).map(|c| c.pulse)
    }
}

/// The transaction time of the atom at `seq`, given its log's commits
/// `(pulse, size)` in ledger order: the Pulse of the first commit whose size
/// covers the atom (`size > seq`), or `None` if none does yet.
pub fn known_at(seq: u64, commits: &[(Pulse, u64)]) -> Result<Option<Pulse>> {
    let history = CommitHistory::new(
        commits
            .iter()
            .map(|&(pulse, size)| Commit { pulse, size })
            .collect(),
    )?;
    Ok(history.known_at(seq))
}

/// An append-only list of one writer's atoms with its Merkle tree.
#[derive(Clone, Debug)]
pub struct WriterLog {
    writer: AccountId,
    atoms: Vec<MemoryAtom>,
    ids: Vec<MemoryAtomId>,
    by_id: BTreeMap<MemoryAtomId, u64>,
    tree: MerkleCache,
}

impl WriterLog {
    /// An empty log for `writer`.
    pub fn new(writer: AccountId) -> Self {
        WriterLog {
            writer,
            atoms: Vec::new(),
            ids: Vec::new(),
            by_id: BTreeMap::new(),
            tree: MerkleCache::new(),
        }
    }

    /// The writer.
    pub fn writer(&self) -> AccountId {
        self.writer
    }

    /// Checks that `atom` would be accepted as the next atom.
    pub fn check(&self, atom: &MemoryAtom) -> Result<()> {
        if atom.writer != self.writer {
            return Err(MemoryError::WrongWriter {
                expected: self.writer,
                actual: atom.writer,
            });
        }
        if atom.seq != self.len() {
            return Err(MemoryError::WrongSeq {
                expected: self.len(),
                actual: atom.seq,
            });
        }
        atom.validate()?;
        Ok(())
    }

    /// Appends the next atom (its writer must be the log's, its `seq` the
    /// log's length, and it must validate); returns its sequence number.
    pub fn append(&mut self, atom: MemoryAtom) -> Result<u64> {
        self.check(&atom)?;
        let id = atom.id();
        let seq = atom.seq;
        self.tree.push(id.leaf());
        self.ids.push(id);
        self.by_id.insert(id, seq);
        self.atoms.push(atom);
        Ok(seq)
    }

    /// Atoms in the log.
    pub fn len(&self) -> u64 {
        self.atoms.len() as u64
    }

    /// Whether the log is empty.
    pub fn is_empty(&self) -> bool {
        self.atoms.is_empty()
    }

    fn check_size(&self, size: u64) -> Result<()> {
        if size > self.len() {
            return Err(MemoryError::OutOfRange {
                index: size,
                size: self.len(),
            });
        }
        Ok(())
    }

    /// The current root.
    pub fn root(&self) -> Hash32 {
        self.tree.root_at(self.len())
    }

    /// The root of the first `size` atoms.
    pub fn root_at(&self, size: u64) -> Result<Hash32> {
        self.check_size(size)?;
        Ok(self.tree.root_at(size))
    }

    /// Inclusion proof of the atom at `seq` against the current root.
    pub fn inclusion_proof(&self, seq: u64) -> Result<Vec<Hash32>> {
        self.tree.inclusion_proof(seq, self.len())
    }

    /// Inclusion proof of the atom at `seq` against the root at `size`.
    pub fn inclusion_proof_at(&self, seq: u64, size: u64) -> Result<Vec<Hash32>> {
        self.check_size(size)?;
        self.tree.inclusion_proof(seq, size)
    }

    /// Consistency proof from `old_size` to the current size.
    pub fn consistency_proof(&self, old_size: u64) -> Result<Vec<Hash32>> {
        self.tree.consistency_proof(old_size, self.len())
    }

    /// Consistency proof from `old_size` to `new_size`.
    pub fn consistency_proof_between(&self, old_size: u64, new_size: u64) -> Result<Vec<Hash32>> {
        self.check_size(new_size)?;
        self.tree.consistency_proof(old_size, new_size)
    }

    /// The atoms, in log order.
    pub fn atoms(&self) -> &[MemoryAtom] {
        &self.atoms
    }

    /// The atom at `seq`.
    pub fn atom(&self, seq: u64) -> Option<&MemoryAtom> {
        usize::try_from(seq).ok().and_then(|i| self.atoms.get(i))
    }

    /// The atom ids, in log order.
    pub fn ids(&self) -> &[MemoryAtomId] {
        &self.ids
    }

    /// The position of the atom with this id.
    pub fn seq_of(&self, id: &MemoryAtomId) -> Option<u64> {
        self.by_id.get(id).copied()
    }

    /// The atom with this id.
    pub fn atom_by_id(&self, id: &MemoryAtomId) -> Option<&MemoryAtom> {
        self.seq_of(id).and_then(|seq| self.atom(seq))
    }

    /// The leaves, `leaf_hash(atom_id)` in log order.
    pub fn leaves(&self) -> &[Hash32] {
        self.tree.leaves()
    }

    /// The index of the first `size` atoms.
    pub fn index(&self, size: u64) -> Result<MemoryIndex> {
        self.check_size(size)?;
        let mut builder = crate::index::IndexBuilder::new();
        for (atom, id) in self.atoms.iter().zip(&self.ids).take(size as usize) {
            builder.push_with_id(atom, *id)?;
        }
        Ok(builder.finish())
    }

    /// The head of the first `size` atoms (builds their index).
    pub fn head(&self, size: u64) -> Result<Head> {
        let index = self.index(size)?;
        Ok(Head {
            size,
            root: self.tree.root_at(size),
            index_root: index.root(),
        })
    }
}

/// Magic bytes of an atom file.
const LOG_MAGIC: &[u8; 8] = b"SIONMLOG";
/// Magic bytes of a key file.
const KEY_MAGIC: &[u8; 8] = b"SIONMKEY";
/// The only file format version this crate reads and writes.
pub const STORE_VERSION: u8 = 1;
/// Name of the atom file in a store directory.
pub const ATOMS_FILE: &str = "atoms.log";
/// Name of the key file in a store directory.
pub const KEYS_FILE: &str = "keys.bin";
/// Largest encoded atom a store accepts.
const MAX_RECORD_BYTES: u32 = 1 << 24;
const HEADER_BYTES: u64 = 8 + 1 + 32;

/// A writer's log and keys on disk, so a command-line tool can keep a
/// robot's memory across invocations.
///
/// The directory holds two files:
///
/// - `atoms.log`: `b"SIONMLOG"`, a version byte (1), the writer's 32-byte
///   account id, then one record per atom: a `u32` little-endian length,
///   the atom's Borsh encoding, and its 32-byte id as a check. The file is
///   only appended to. On opening, every record is decoded, checked against
///   its id and re-validated by appending it to a [`WriterLog`]; a torn
///   final record (a crash mid-write) is cut off, and anything else that
///   does not check refuses to open.
/// - `keys.bin`: `b"SIONMKEY"`, a version byte (1), then the Borsh encoding
///   of the [`KeyRing`]: epoch keys and the ids of destroyed ones. It is
///   rewritten whole (to a temporary file, then renamed) and, on Unix,
///   readable by its owner only. Destroying a key rewrites the file without
///   it; whether the old bytes are gone from the medium is up to the file
///   system and the drive, a trust assumption to name wherever FORGET is
///   relied on.
///
/// Unknown versions are refused.
#[derive(Debug)]
pub struct LogStore {
    dir: PathBuf,
    file: File,
    log: WriterLog,
    keys: KeyRing,
}

impl LogStore {
    /// Creates a store for `writer` in `dir` (created if missing; it must
    /// not already hold a log).
    pub fn create(dir: impl AsRef<Path>, writer: AccountId) -> Result<LogStore> {
        let dir = dir.as_ref().to_path_buf();
        fs::create_dir_all(&dir)?;
        let path = dir.join(ATOMS_FILE);
        let mut file = OpenOptions::new()
            .read(true)
            .append(true)
            .create_new(true)
            .open(&path)
            .map_err(|e| MemoryError::Store(format!("cannot create {}: {e}", path.display())))?;
        let mut header = Vec::with_capacity(HEADER_BYTES as usize);
        header.extend_from_slice(LOG_MAGIC);
        header.push(STORE_VERSION);
        header.extend_from_slice(&writer.0 .0);
        file.write_all(&header)?;
        file.sync_all()?;
        let store = LogStore {
            dir,
            file,
            log: WriterLog::new(writer),
            keys: KeyRing::new(),
        };
        store.save_keys()?;
        Ok(store)
    }

    /// Opens an existing store.
    pub fn open(dir: impl AsRef<Path>) -> Result<LogStore> {
        let dir = dir.as_ref().to_path_buf();
        let path = dir.join(ATOMS_FILE);
        let mut file = OpenOptions::new()
            .read(true)
            .append(true)
            .open(&path)
            .map_err(|e| MemoryError::Store(format!("cannot open {}: {e}", path.display())))?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        let (log, good) = parse_log(&bytes)?;
        if good < bytes.len() as u64 {
            file.set_len(good)?;
            file.sync_all()?;
        }
        file.seek(SeekFrom::End(0))?;
        let keys = load_keys(&dir.join(KEYS_FILE))?;
        Ok(LogStore {
            dir,
            file,
            log,
            keys,
        })
    }

    /// Opens the store in `dir`, or creates one for `writer`; an existing
    /// store must belong to `writer`.
    pub fn open_or_create(dir: impl AsRef<Path>, writer: AccountId) -> Result<LogStore> {
        if dir.as_ref().join(ATOMS_FILE).exists() {
            let store = LogStore::open(dir)?;
            if store.log.writer() != writer {
                return Err(MemoryError::WrongWriter {
                    expected: writer,
                    actual: store.log.writer(),
                });
            }
            Ok(store)
        } else {
            LogStore::create(dir, writer)
        }
    }

    /// The directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The log.
    pub fn log(&self) -> &WriterLog {
        &self.log
    }

    /// The keys.
    pub fn keys(&self) -> &KeyRing {
        &self.keys
    }

    /// Appends an atom to the file (synced) and the log; returns its sequence number.
    pub fn append(&mut self, atom: MemoryAtom) -> Result<u64> {
        let seq = self.write(atom)?;
        self.file.sync_data()?;
        Ok(seq)
    }

    /// Appends atoms in order with one sync at the end (for imports); returns
    /// the new length. If an atom is refused, the ones before it stay
    /// appended (and are synced) and the error is returned.
    pub fn append_all(&mut self, atoms: impl IntoIterator<Item = MemoryAtom>) -> Result<u64> {
        let mut outcome = Ok(());
        for atom in atoms {
            if let Err(error) = self.write(atom) {
                outcome = Err(error);
                break;
            }
        }
        self.file.sync_data()?;
        outcome.map(|()| self.log.len())
    }

    fn write(&mut self, atom: MemoryAtom) -> Result<u64> {
        self.log.check(&atom)?;
        let body = encode(&atom);
        let length = u32::try_from(body.len())
            .ok()
            .filter(|&len| len <= MAX_RECORD_BYTES)
            .ok_or_else(|| MemoryError::Store("atom too large for a record".into()))?;
        let mut record = Vec::with_capacity(4 + body.len() + 32);
        record.extend_from_slice(&length.to_le_bytes());
        record.extend_from_slice(&body);
        record.extend_from_slice(&atom.id().0 .0);
        self.file.write_all(&record)?;
        self.log.append(atom)
    }

    /// Changes the keys with `change` and saves them.
    pub fn update_keys<T>(&mut self, change: impl FnOnce(&mut KeyRing) -> Result<T>) -> Result<T> {
        let out = change(&mut self.keys)?;
        self.save_keys()?;
        Ok(out)
    }

    fn save_keys(&self) -> Result<()> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(KEY_MAGIC);
        bytes.push(STORE_VERSION);
        bytes.extend_from_slice(&self.keys.to_bytes());
        let path = self.dir.join(KEYS_FILE);
        let temp = self.dir.join(format!("{KEYS_FILE}.tmp"));
        {
            let mut options = OpenOptions::new();
            options.write(true).create(true).truncate(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temp)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
        }
        fs::rename(&temp, &path)?;
        Ok(())
    }
}

/// Parses an atom file; returns the log and the length of its good prefix.
fn parse_log(bytes: &[u8]) -> Result<(WriterLog, u64)> {
    if bytes.len() < HEADER_BYTES as usize || &bytes[..8] != LOG_MAGIC {
        return Err(MemoryError::Store("not a memory log".into()));
    }
    if bytes[8] != STORE_VERSION {
        return Err(MemoryError::Store(format!(
            "unknown log version {} (this build reads {STORE_VERSION})",
            bytes[8]
        )));
    }
    let mut writer = [0u8; 32];
    writer.copy_from_slice(&bytes[9..41]);
    let mut log = WriterLog::new(AccountId(Hash32(writer)));
    let mut at = HEADER_BYTES as usize;
    while at < bytes.len() {
        let rest = &bytes[at..];
        if rest.len() < 4 {
            break;
        }
        let length = u32::from_le_bytes(rest[..4].try_into().expect("four bytes"));
        if length > MAX_RECORD_BYTES {
            return Err(MemoryError::Store(format!(
                "record at byte {at} is too large"
            )));
        }
        let end = 4 + length as usize + 32;
        if rest.len() < end {
            break;
        }
        let atom: MemoryAtom = decode(&rest[4..4 + length as usize])
            .map_err(|e| MemoryError::Store(format!("record at byte {at}: {e}")))?;
        if atom.id().0 .0[..] != rest[4 + length as usize..end] {
            return Err(MemoryError::Store(format!(
                "record at byte {at} fails its check"
            )));
        }
        log.append(atom)
            .map_err(|e| MemoryError::Store(format!("record at byte {at}: {e}")))?;
        at += end;
    }
    Ok((log, at as u64))
}

fn load_keys(path: &Path) -> Result<KeyRing> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(KeyRing::new()),
        Err(e) => return Err(e.into()),
    };
    if bytes.len() < 9 || &bytes[..8] != KEY_MAGIC {
        return Err(MemoryError::Store("not a memory key file".into()));
    }
    if bytes[8] != STORE_VERSION {
        return Err(MemoryError::Store(format!(
            "unknown key file version {} (this build reads {STORE_VERSION})",
            bytes[8]
        )));
    }
    KeyRing::from_bytes(&bytes[9..])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atom::{AtomClass, MemoryKind, Statement, Value};
    use crate::geom::Interval;
    use crate::index::testing::random_log;
    use crate::seal::EpochKey;
    use proptest::prelude::*;
    use sion_core::merkle;

    fn writer() -> AccountId {
        AccountId::derive(b"log-test")
    }

    fn atom(seq: u64) -> MemoryAtom {
        MemoryAtom::builder(
            writer(),
            seq,
            MemoryKind::Episodic,
            AtomClass::AgentMemory,
            Statement::claim("user", "said", Value::Text(format!("message {seq}"))),
        )
        .valid(Interval::starting(seq * 10))
        .build()
        .unwrap()
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "sion-memory-{name}-{}-{}",
            std::process::id(),
            sion_core::hash::hash_domain("t", name.as_bytes()).short()
        ));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn append_enforces_writer_sequence_and_validity() {
        let mut log = WriterLog::new(writer());
        assert!(log.is_empty());
        assert_eq!(log.append(atom(0)).unwrap(), 0);
        assert!(matches!(
            log.append(atom(0)),
            Err(MemoryError::WrongSeq { .. })
        ));
        let mut stranger = atom(1);
        stranger.writer = AccountId::derive(b"other");
        assert!(matches!(
            log.append(stranger),
            Err(MemoryError::WrongWriter { .. })
        ));
        let mut invalid = atom(1);
        invalid.confidence = 1_001;
        assert!(matches!(
            log.append(invalid),
            Err(MemoryError::InvalidAtom(_))
        ));
        log.append(atom(1)).unwrap();
        assert_eq!(log.len(), 2);
        assert_eq!(log.writer(), writer());
        let id = log.ids()[1];
        assert_eq!(log.seq_of(&id), Some(1));
        assert_eq!(log.atom_by_id(&id), log.atom(1));
        assert!(log.atom(5).is_none());
    }

    #[test]
    fn leaves_and_roots_follow_the_ledger_contract() {
        let mut log = WriterLog::new(writer());
        let mut heads = vec![(0, merkle::empty_root())];
        for seq in 0..20 {
            log.append(atom(seq)).unwrap();
            let leaves: Vec<Hash32> = log
                .atoms()
                .iter()
                .map(|a| merkle::leaf_hash(&a.id().0 .0))
                .collect();
            assert_eq!(log.leaves(), leaves.as_slice());
            assert_eq!(log.root(), merkle::root(&leaves));
            heads.push((log.len(), log.root()));
        }
        for &(old, old_root) in &heads {
            assert_eq!(log.root_at(old).unwrap(), old_root);
            let proof = log.consistency_proof(old).unwrap();
            merkle::verify_consistency(&old_root, old, &log.root(), log.len(), &proof).unwrap();
            for &(new, new_root) in heads.iter().filter(|(n, _)| *n >= old) {
                let proof = log.consistency_proof_between(old, new).unwrap();
                merkle::verify_consistency(&old_root, old, &new_root, new, &proof).unwrap();
            }
        }
        for seq in 0..20 {
            let path = log.inclusion_proof(seq).unwrap();
            merkle::verify_inclusion(&log.root(), 20, seq, &log.ids()[seq as usize].leaf(), &path)
                .unwrap();
            let path = log.inclusion_proof_at(seq, seq + 1).unwrap();
            merkle::verify_inclusion(
                &log.root_at(seq + 1).unwrap(),
                seq + 1,
                seq,
                &log.leaves()[seq as usize],
                &path,
            )
            .unwrap();
        }
        assert!(log.root_at(21).is_err());
        assert!(log.inclusion_proof(20).is_err());
        assert!(log.consistency_proof_between(3, 21).is_err());
    }

    #[test]
    fn heads_bind_the_index_of_their_prefix() {
        let atoms = random_log(31, 60);
        let mut log = WriterLog::new(atoms[0].writer);
        for a in &atoms {
            log.append(a.clone()).unwrap();
        }
        let head = log.head(40).unwrap();
        assert_eq!(head.size, 40);
        assert_eq!(head.root, log.root_at(40).unwrap());
        assert_eq!(
            head.index_root,
            MemoryIndex::build(&atoms[..40]).unwrap().root()
        );
        assert_eq!(log.index(40).unwrap().root(), head.index_root);
        assert!(log.head(61).is_err());
    }

    #[test]
    fn known_at_is_the_first_covering_commit() {
        let commits = [(Pulse(5), 2), (Pulse(9), 2), (Pulse(12), 7)];
        assert_eq!(known_at(0, &commits).unwrap(), Some(Pulse(5)));
        assert_eq!(known_at(1, &commits).unwrap(), Some(Pulse(5)));
        assert_eq!(known_at(2, &commits).unwrap(), Some(Pulse(12)));
        assert_eq!(known_at(6, &commits).unwrap(), Some(Pulse(12)));
        assert_eq!(known_at(7, &commits).unwrap(), None);
        assert_eq!(known_at(0, &[]).unwrap(), None);
        assert!(known_at(0, &[(Pulse(5), 3), (Pulse(4), 4)]).is_err());
        assert!(known_at(0, &[(Pulse(5), 3), (Pulse(6), 2)]).is_err());
        let history = CommitHistory::new(
            commits
                .iter()
                .map(|&(pulse, size)| Commit { pulse, size })
                .collect(),
        )
        .unwrap();
        assert_eq!(history.committed_size(), 7);
        assert_eq!(history.as_of(Pulse(10)).committed_size(), 2);
        assert_eq!(history.as_of(Pulse(4)).commits(), &[]);
        assert_eq!(history.commits().len(), 3);
    }

    #[test]
    fn store_persists_atoms_and_keys() {
        let dir = temp_dir("persist");
        let key = EpochKey::from_bytes(Hash32([1; 32]), 0, [9; 32]);
        {
            let mut store = LogStore::create(&dir, writer()).unwrap();
            for seq in 0..5 {
                store.append(atom(seq)).unwrap();
            }
            store.update_keys(|keys| keys.insert(key.clone())).unwrap();
            assert_eq!(store.dir(), dir.as_path());
            assert!(
                LogStore::create(&dir, writer()).is_err(),
                "never overwrites a log"
            );
        }
        let mut store = LogStore::open(&dir).unwrap();
        assert_eq!(store.log().len(), 5);
        assert_eq!(store.log().writer(), writer());
        assert!(store.keys().get(&key.id()).is_ok());
        store.append(atom(5)).unwrap();
        // A batch stops at the first refused atom and keeps the ones before it.
        let batch = vec![atom(6), atom(7), atom(9), atom(10)];
        assert!(matches!(
            store.append_all(batch),
            Err(MemoryError::WrongSeq { .. })
        ));
        assert_eq!(store.log().len(), 8);
        assert_eq!(store.append_all([atom(8)]).unwrap(), 9);
        let root = store.log().root();
        drop(store);
        let store = LogStore::open_or_create(&dir, writer()).unwrap();
        assert_eq!(store.log().root(), root);
        assert!(matches!(
            LogStore::open_or_create(&dir, AccountId::derive(b"other")),
            Err(MemoryError::WrongWriter { .. })
        ));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn store_cuts_torn_tails_and_refuses_corruption() {
        let dir = temp_dir("torn");
        {
            let mut store = LogStore::create(&dir, writer()).unwrap();
            for seq in 0..3 {
                store.append(atom(seq)).unwrap();
            }
        }
        let path = dir.join(ATOMS_FILE);
        let full = fs::read(&path).unwrap();
        // A crash mid-write leaves part of a record: it is cut off.
        let mut torn = full.clone();
        torn.extend_from_slice(&[200, 0, 0, 0, 1, 2, 3]);
        fs::write(&path, &torn).unwrap();
        let store = LogStore::open(&dir).unwrap();
        assert_eq!(store.log().len(), 3);
        drop(store);
        assert_eq!(fs::read(&path).unwrap(), full);
        // A flipped byte inside a record is refused.
        let mut corrupt = full.clone();
        let at = corrupt.len() - 40;
        corrupt[at] ^= 0xff;
        fs::write(&path, &corrupt).unwrap();
        assert!(matches!(LogStore::open(&dir), Err(MemoryError::Store(_))));
        // An unknown version is refused.
        let mut future = full.clone();
        future[8] = 2;
        fs::write(&path, &future).unwrap();
        let err = LogStore::open(&dir).unwrap_err();
        assert!(err.to_string().contains("unknown log version"));
        // Not a log at all.
        fs::write(&path, b"hello").unwrap();
        assert!(LogStore::open(&dir).is_err());
        // An unknown key-file version is refused.
        fs::write(&path, &full).unwrap();
        let mut keys = fs::read(dir.join(KEYS_FILE)).unwrap();
        keys[8] = 9;
        fs::write(dir.join(KEYS_FILE), &keys).unwrap();
        assert!(LogStore::open(&dir).is_err());
        let _ = fs::remove_dir_all(&dir);
        assert!(LogStore::open(&dir).is_err(), "no store, no log");
    }

    proptest! {
        #![proptest_config(crate::testing::config(48, 0x5eed_1065))]

        #[test]
        fn leaf_contract_and_consistency_hold(seed in any::<u64>(), n in 1usize..90, cut in any::<u64>()) {
            let atoms = random_log(seed, n);
            let mut log = WriterLog::new(atoms[0].writer);
            for a in &atoms {
                log.append(a.clone()).unwrap();
            }
            let leaves: Vec<Hash32> = atoms.iter().map(|a| merkle::leaf_hash(&a.id().0 .0)).collect();
            prop_assert_eq!(log.root(), merkle::root(&leaves));
            let old = cut % (n as u64 + 1);
            let old_head = log.head(old).unwrap();
            let new_head = log.head(n as u64).unwrap();
            let proof = log.consistency_proof(old).unwrap();
            prop_assert_eq!(&proof, &merkle::consistency_proof(&leaves, old as usize).unwrap());
            prop_assert!(merkle::verify_consistency(&old_head.root, old_head.size, &new_head.root, new_head.size, &proof).is_ok());
        }
    }
}
