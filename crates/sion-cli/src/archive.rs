//! A memory log's archive, kept by storage providers and restored from
//! them (`sion memory archive`, `sion memory restore`).
//!
//! An archive holds every atom of the log at the size the ledger holds,
//! encrypted for the log's writer: ChaCha20-Poly1305 under a key derived
//! from the writer's wallet seed and the log's id, so a provider keeps bytes
//! it cannot read. Its header, in the clear, says which log it is of and at
//! what size and root:
//!
//! ```text
//! "sion-memory-archive/v1" ‖ log (32) ‖ size (u64 LE) ‖ root (32) ‖ nonce (12) ‖ ciphertext
//! ```
//!
//! with the header as the cipher's associated data. Archiving first commits
//! what the local store holds beyond the ledger's head, then names the
//! archive's storage root as the head's `manifest` (a commit at the same
//! size, which keeps the root), and posts a storage deal for the archive,
//! so the ledger itself says which bytes the writer means to keep and a
//! provider proves every round that it keeps them. Later commits carry the
//! manifest forward.
//!
//! Restoring takes an archive from a file or from the provider that keeps
//! it (`POST /v1/deals/<deal>/retrieve`, signed by the deal's client), and
//! checks it from the ledger alone: its storage root is the manifest the
//! log's head names, it opens with the wallet's key, its atoms rebuild the
//! root its header names, and the ledger committed the log at that size
//! with that root. Only then does it replace the local store.
//!
//! An archive holds atoms, never the store's epoch keys: payloads sealed
//! under an epoch key stay sealed, so forgetting (destroying a key) holds
//! for archives too, and a restore keeps the key file it finds.

use std::path::{Path, PathBuf};

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use serde_json::{json, Value as Json};
use sion_core::encode::{decode, encode};
use sion_core::hash::{hash_parts, Hash32};
use sion_core::ids::{ObjectId, Pulse};
use sion_exec::execute::Event;
use sion_exec::storage::{DataTree, DealState};
use sion_memory::{LogStore, MemoryAtom};
use zeroize::Zeroizing;

use crate::error::CliError;
use crate::ledger::{Ledger, LedgerExt};
use crate::memory::{committed, memory_error, open_store, store_dir};
use crate::session::Session;

/// What an archive starts with.
pub const MAGIC: &[u8] = b"sion-memory-archive/v1";

/// The header's length: the magic, the log, the size, the root and the
/// nonce.
pub const HEADER: usize = MAGIC.len() + 32 + 8 + 32 + 12;

/// The cipher's tag.
const TAG: usize = 16;

/// The key that encrypts `log`'s archives for the wallet with `seed`.
pub fn archive_key(seed: &[u8; 32], log: &ObjectId) -> Zeroizing<[u8; 32]> {
    Zeroizing::new(hash_parts("sion/memory/archive-key/v1", &[seed, &log.0 .0]).0)
}

/// An archive's header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    /// The log.
    pub log: ObjectId,
    /// How many atoms it holds.
    pub size: u64,
    /// The log's root at that size.
    pub root: Hash32,
}

fn cipher(seed: &[u8; 32], log: &ObjectId) -> ChaCha20Poly1305 {
    ChaCha20Poly1305::new(Key::from_slice(archive_key(seed, log).as_slice()))
}

/// Seals `atoms` (the whole log at the header's size and root) for the
/// wallet with `seed`.
pub fn seal(
    seed: &[u8; 32],
    header: &Header,
    atoms: &[MemoryAtom],
    nonce: [u8; 12],
) -> Result<Vec<u8>, CliError> {
    let mut out = Vec::with_capacity(HEADER);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&header.log.0 .0);
    out.extend_from_slice(&header.size.to_le_bytes());
    out.extend_from_slice(&header.root.0);
    out.extend_from_slice(&nonce);
    let plain = Zeroizing::new(encode(&atoms.to_vec()));
    let sealed = cipher(seed, &header.log)
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: &plain,
                aad: &out,
            },
        )
        .map_err(|_| CliError::Usage("the archive could not be encrypted".into()))?;
    out.extend_from_slice(&sealed);
    Ok(out)
}

/// Reads an archive's header.
pub fn header(bytes: &[u8]) -> Result<Header, CliError> {
    if bytes.len() < HEADER + TAG || !bytes.starts_with(MAGIC) {
        return Err(CliError::Usage("not a memory archive".into()));
    }
    let at = MAGIC.len();
    let field =
        |from: usize, to: usize| -> [u8; 32] { bytes[from..to].try_into().expect("32 bytes") };
    Ok(Header {
        log: ObjectId(Hash32(field(at, at + 32))),
        size: u64::from_le_bytes(bytes[at + 32..at + 40].try_into().expect("8 bytes")),
        root: Hash32(field(at + 40, at + 72)),
    })
}

/// Opens an archive with the wallet's `seed`: its header and atoms.
pub fn open(seed: &[u8; 32], bytes: &[u8]) -> Result<(Header, Vec<MemoryAtom>), CliError> {
    let header = header(bytes)?;
    let plain = cipher(seed, &header.log)
        .decrypt(
            Nonce::from_slice(&bytes[HEADER - 12..HEADER]),
            Payload {
                msg: &bytes[HEADER..],
                aad: &bytes[..HEADER],
            },
        )
        .map(Zeroizing::new)
        .map_err(|_| {
            CliError::Usage(
                "the archive does not open with this wallet's key, or it was altered".into(),
            )
        })?;
    let atoms: Vec<MemoryAtom> = decode(&plain)
        .map_err(|e| CliError::Usage(format!("the archive's atoms do not decode: {e}")))?;
    if atoms.len() as u64 != header.size {
        return Err(CliError::Usage(format!(
            "the archive's header says {} atoms, it holds {}",
            header.size,
            atoms.len()
        )));
    }
    Ok((header, atoms))
}

/// Where archives are written by default: `$SION_HOME/memory/archives`.
pub fn archives_dir() -> PathBuf {
    store_dir(&ObjectId::from_hash(Hash32::ZERO))
        .parent()
        .map(|memory| memory.join("archives"))
        .unwrap_or_else(|| PathBuf::from(".sion/memory/archives"))
}

/// An archive made and named on the ledger.
#[derive(Clone, Debug)]
pub struct Made {
    /// Where it was written.
    pub path: PathBuf,
    /// Its header.
    pub header: Header,
    /// Its storage tree: the root the manifest names and a deal commits to.
    pub tree: DataTree,
    /// The receipts of the commits it took, as sentences.
    pub receipts: Vec<String>,
}

/// Makes `log`'s archive at the size the ledger holds and names its
/// storage root as the head's manifest: commits what the local store holds
/// beyond the head first. Writes it to `out`, or under [`archives_dir`].
pub fn make(
    session: &mut Session<'_>,
    ledger: &dyn Ledger,
    wallet: &str,
    log: &ObjectId,
    out: Option<&Path>,
) -> Result<Made, CliError> {
    let writer = session.id(wallet);
    let mut receipts = Vec::new();
    let held = open_store(log, writer)?.log().len();
    if held > committed(ledger, log)?.size {
        let view = crate::memory::commit(session, ledger, wallet, log, false)?;
        receipts.push(receipt(session, &view));
    }
    let head = committed(ledger, log)?;
    if head.size == 0 {
        return Err(CliError::Usage(
            "the log holds no atoms: there is nothing to archive".into(),
        ));
    }
    if head.sealed {
        return Err(CliError::Usage(
            "the log is sealed: its head takes no more commits, so no manifest can be named".into(),
        ));
    }
    let store = open_store(log, writer)?;
    if store.log().len() < head.size {
        return Err(CliError::Usage(format!(
            "the local store holds {} atoms, fewer than the {} the ledger has: restore it first",
            store.log().len(),
            head.size
        )));
    }
    let atoms = &store.log().atoms()[..head.size as usize];
    if store.log().root_at(head.size).map_err(memory_error)? != head.root {
        return Err(CliError::Usage(
            "the local store does not hold the head the ledger holds".into(),
        ));
    }
    let header = Header {
        log: *log,
        size: head.size,
        root: head.root,
    };
    let mut nonce = [0u8; 12];
    rand_core::RngCore::fill_bytes(&mut rand_core::OsRng, &mut nonce);
    let bytes = seal(&crate::session::wallet_seed(wallet), &header, atoms, nonce)?;
    drop(store);
    let path = match out {
        Some(path) => path.to_path_buf(),
        None => archives_dir().join(format!("{}-{}.bin", log.0.to_hex(), head.size)),
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| CliError::Usage(e.to_string()))?;
    }
    let partial = path.with_extension("partial");
    std::fs::write(&partial, &bytes)
        .and_then(|()| std::fs::rename(&partial, &path))
        .map_err(|e| CliError::Usage(format!("{}: {e}", path.display())))?;
    let tree = DataTree::of_data(&bytes);
    let view = crate::memory::set_manifest(session, ledger, wallet, log, tree.root())?;
    receipts.push(receipt(session, &view));
    Ok(Made {
        path,
        header,
        tree,
        receipts,
    })
}

fn receipt(session: &Session<'_>, view: &sion_node::TxView) -> String {
    format!(
        "Pulse {}: {}",
        view.pulse.0,
        crate::describe::receipt(&session.names(), &view.effects)
    )
}

/// The Pulse at which the ledger committed `log` at `size` with `root`, if
/// it did, from its `MemoryCommitted` events since the log was opened.
pub fn committed_at(
    ledger: &dyn Ledger,
    log: &ObjectId,
    opened: Pulse,
    size: u64,
    root: &Hash32,
) -> Result<Option<Pulse>, CliError> {
    const PAGE: usize = 500;
    let mut from = opened;
    loop {
        let events = ledger.events(from, PAGE)?;
        for view in &events {
            if let Event::MemoryCommitted {
                log: l,
                size: s,
                root: r,
                ..
            } = &view.event
            {
                if l == log && *s == size && r == root {
                    return Ok(Some(view.pulse));
                }
            }
        }
        let Some(last) = events.last().map(|v| v.pulse) else {
            return Ok(None);
        };
        if events.len() < PAGE {
            return Ok(None);
        }
        // A full page may end inside a Pulse: read that Pulse again, unless
        // the whole page was one Pulse (then go on after it).
        from = if last == from {
            Pulse(last.0 + 1)
        } else {
            last
        };
    }
}

/// The deal that keeps the archive with storage root `root` for `client`:
/// the latest posted one a provider accepted.
pub fn deal_for(
    ledger: &dyn Ledger,
    client: &sion_core::ids::AccountId,
    root: &Hash32,
) -> Result<ObjectId, CliError> {
    let mut found: Vec<(Pulse, ObjectId)> = crate::storage::deals(ledger, false)?
        .into_iter()
        .filter(|(_, deal)| {
            deal.client == *client && deal.root == *root && deal.state != DealState::Open
        })
        .map(|(id, deal)| (deal.posted_at, id))
        .collect();
    found.sort();
    found.pop().map(|(_, id)| id).ok_or_else(|| {
        CliError::Usage(format!(
            "no accepted storage deal of yours keeps the archive {}",
            root.short()
        ))
    })
}

/// Restores `log`'s local store, written by `wallet`, from `bytes`, checked
/// against the ledger; with `replace`, over the store there is.
pub fn restore(
    session: &mut Session<'_>,
    ledger: &dyn Ledger,
    wallet: &str,
    log: &ObjectId,
    bytes: &[u8],
    replace: bool,
) -> Result<Json, CliError> {
    let head = committed(ledger, log)?;
    if head.writer != session.id(wallet) {
        return Err(CliError::Usage(format!(
            "log {} is written by another account",
            log.0.short()
        )));
    }
    let tree = DataTree::of_data(bytes);
    if tree.root() != head.manifest {
        return Err(CliError::Usage(format!(
            "this archive's storage root {} is not the manifest {} the log's head names on the ledger",
            tree.root().short(),
            head.manifest.short()
        )));
    }
    let (header, atoms) = open(&crate::session::wallet_seed(wallet), bytes)?;
    if header.log != *log {
        return Err(CliError::Usage("the archive is of another log".into()));
    }
    if header.size > head.size {
        return Err(CliError::Usage(format!(
            "the archive holds {} atoms, more than the {} the ledger has",
            header.size, head.size
        )));
    }
    let dir = store_dir(log);
    if dir.exists() && !replace {
        return Err(CliError::Usage(format!(
            "{} holds a store already: give --replace to restore over it",
            dir.display()
        )));
    }
    let temp = dir.with_extension("restoring");
    let _ = std::fs::remove_dir_all(&temp);
    let rebuilt = (|| -> Result<Hash32, CliError> {
        let mut store = LogStore::open_or_create(&temp, head.writer).map_err(memory_error)?;
        store.append_all(atoms).map_err(memory_error)?;
        store.log().root_at(header.size).map_err(memory_error)
    })();
    let fail = |error: CliError| {
        let _ = std::fs::remove_dir_all(&temp);
        Err(error)
    };
    match rebuilt {
        Ok(root) if root == header.root => {}
        Ok(_) => {
            return fail(CliError::Usage(
                "the archive's atoms do not rebuild the root its header names".into(),
            ))
        }
        Err(error) => return fail(error),
    }
    // The head the archive holds is one the ledger committed: the log's head
    // itself, or an earlier one if the log grew after the archive was made.
    // An earlier head is found among the node's events, which reach back
    // only as far as the node keeps history.
    let found = if header.size == head.size {
        if header.root != head.root {
            return fail(CliError::Usage(
                "the archive's root is not the root the ledger holds at its size".into(),
            ));
        }
        Some(head.committed_at)
    } else {
        committed_at(ledger, log, head.opened_at, header.size, &header.root)?
    };
    let checked = match found {
        Some(at) => format!(
            "the archive is the one the log's head names as its manifest, opens with this wallet's key, and rebuilds the head the ledger committed at Pulse {}",
            at.0
        ),
        None => format!(
            "the archive is the one the log's head names as its manifest and opens with this wallet's key; the commit of its head ({} atoms) is older than the history this node keeps, so only the manifest, which only the writer can commit, vouches for it",
            header.size
        ),
    };
    // The store's keys stay: an archive never holds them.
    let keys = dir.join(sion_memory::log::KEYS_FILE);
    if keys.exists() {
        std::fs::copy(&keys, temp.join(sion_memory::log::KEYS_FILE))
            .map_err(|e| CliError::Usage(format!("{}: {e}", keys.display())))?;
    }
    if dir.exists() {
        let old = dir.with_extension("replaced");
        let _ = std::fs::remove_dir_all(&old);
        std::fs::rename(&dir, &old).map_err(|e| CliError::Usage(e.to_string()))?;
        std::fs::rename(&temp, &dir).map_err(|e| CliError::Usage(e.to_string()))?;
        let _ = std::fs::remove_dir_all(&old);
    } else {
        std::fs::rename(&temp, &dir).map_err(|e| CliError::Usage(e.to_string()))?;
    }
    let mut out = json!({
        "log": log,
        "restored_atoms": header.size,
        "root": header.root,
        "atoms_on_ledger": head.size,
        "checked": checked,
    });
    if header.size < head.size {
        let missing = head.size - header.size;
        out["missing"] = json!(format!(
            "the ledger holds {missing} {} the archive does not: the log takes no new atoms until {} restored",
            if missing == 1 { "atom" } else { "atoms" },
            if missing == 1 { "it is" } else { "they are" }
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sion_core::ids::AccountId;
    use sion_memory::{AtomClass, MemoryKind, Statement, Value};

    fn atoms(n: u64) -> Vec<MemoryAtom> {
        let writer = AccountId::derive(b"writer");
        (0..n)
            .map(|seq| {
                MemoryAtom::builder(
                    writer,
                    seq,
                    MemoryKind::Semantic,
                    AtomClass::AgentMemory,
                    Statement::claim("acme", "paid-at", Value::Text(format!("acme-{seq}"))),
                )
                .observed_at(seq)
                .build()
                .unwrap()
            })
            .collect()
    }

    #[test]
    fn an_archive_opens_only_with_its_key_and_unaltered() {
        let seed = [7u8; 32];
        let header = Header {
            log: ObjectId::derive(b"log"),
            size: 3,
            root: Hash32::ZERO,
        };
        let bytes = seal(&seed, &header, &atoms(3), [1u8; 12]).unwrap();
        let (read, opened) = open(&seed, &bytes).unwrap();
        assert_eq!((read, opened.len()), (header, 3));
        assert_eq!(opened, atoms(3));
        // Another wallet's key, or another log's, does not open it.
        assert!(open(&[8u8; 32], &bytes).is_err());
        let mut moved = bytes.clone();
        moved[MAGIC.len()] ^= 1;
        assert!(open(&seed, &moved).is_err());
        // Neither does a changed byte, in the ciphertext or the header.
        let mut altered = bytes.clone();
        let last = altered.len() - 1;
        altered[last] ^= 1;
        assert!(open(&seed, &altered).is_err());
        let mut resized = bytes.clone();
        resized[MAGIC.len() + 32] ^= 1;
        assert!(open(&seed, &resized).is_err());
        let mut rerooted = bytes;
        rerooted[MAGIC.len() + 40] ^= 1;
        assert!(open(&seed, &rerooted).is_err());
        assert!(super::header(&[0u8; HEADER + TAG]).is_err());
        assert!(super::header(MAGIC).is_err());
    }

    #[test]
    fn keys_differ_by_wallet_and_log() {
        let (a, b) = (ObjectId::derive(b"a"), ObjectId::derive(b"b"));
        assert_ne!(*archive_key(&[1; 32], &a), *archive_key(&[1; 32], &b));
        assert_ne!(*archive_key(&[1; 32], &a), *archive_key(&[2; 32], &a));
    }
}
