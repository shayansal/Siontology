//! Pairing: someone a channel does not know writes to a profile, is given a
//! code, and is let in once the operator approves the code on this
//! machine (`sion agent pairing approve`).
//!
//! A code is 8 characters of consonants and digits, drawn evenly from the
//! operating system's randomness: no vowels (nor `3` or `4`, read as `E`
//! and `A`), so a code sent to someone spells no word, and no look-alikes
//! (`0`/`O`, `1`/`I`/`L`). It lasts an
//! hour; a sender is given at most one every ten minutes; a channel has at
//! most three waiting; and five wrong approvals in a row stop approvals
//! for an hour. Approval lets in that sender's chat on that channel, for
//! that profile only. The store is a file only the operator's user can
//! read (`agents/pairing.json`), changed by one process at a time (a
//! bridge giving a code while the operator approves another); codes are
//! never printed by the bridge, only sent to the sender.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::error::CliError;
use crate::schedules::now;

/// The characters of a code: no vowels, nor digits read as vowels, so no
/// code spells a word; no look-alikes.
pub const ALPHABET: &[u8; 25] = b"BCDFGHJKMNPQRSTVWXZ256789";

/// A code's length.
pub const CODE_LEN: usize = 8;

/// How long a code lasts, in seconds.
pub const CODE_SECS: u64 = 3600;

/// How often a sender is given a code, at most, in seconds.
pub const ASK_EVERY_SECS: u64 = 600;

/// The most codes waiting on one channel.
pub const MAX_PENDING: usize = 3;

/// Wrong approvals in a row that stop approvals.
pub const MAX_WRONG: u32 = 5;

/// How long approvals stop after them, in seconds.
pub const LOCKOUT_SECS: u64 = 3600;

/// A code waiting for approval.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    /// The channel and profile (`telegram:support`).
    pub channel: String,
    /// The chat to let in.
    pub chat: String,
    /// Who wrote, as the channel names them.
    pub from: String,
    /// What they wrote first (its start).
    pub said: String,
    /// When the code was made (seconds since the Unix epoch).
    pub made: u64,
}

/// Everything pairing keeps.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pairings {
    /// The chats let in, by channel.
    #[serde(default)]
    pub allowed: BTreeMap<String, BTreeSet<String>>,
    /// Codes waiting, by code.
    #[serde(default)]
    pub pending: BTreeMap<String, Request>,
    /// Wrong approvals in a row.
    #[serde(default)]
    pub wrong: u32,
    /// Until when approvals are stopped.
    #[serde(default)]
    pub locked_until: Option<u64>,
}

/// What a message from a chat the channel does not know gets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stranger {
    /// It was let in by an earlier approval.
    Allowed,
    /// It is given this code, to send to the sender.
    Code(String),
    /// Nothing: it was given a code lately, or the channel has as many
    /// waiting as it may.
    Quiet,
}

/// A lock on the store older than this was left by a process that stopped
/// while holding it (a change takes milliseconds), in seconds.
const STALE_LOCK_SECS: u64 = 30;

/// A new code.
pub fn new_code() -> String {
    // Bytes from 250 up are drawn again, so each character is as likely
    // as the others (250 is a multiple of 25).
    let even = 256 - 256 % ALPHABET.len();
    let mut code = String::with_capacity(CODE_LEN);
    while code.len() < CODE_LEN {
        let mut bytes = [0u8; 16];
        rand_core::RngCore::fill_bytes(&mut rand_core::OsRng, &mut bytes);
        for byte in bytes.map(usize::from) {
            if byte < even && code.len() < CODE_LEN {
                code.push(char::from(ALPHABET[byte % ALPHABET.len()]));
            }
        }
    }
    code
}

/// A code as people may type it: without spaces or dashes, in capitals.
pub fn normalized(code: &str) -> String {
    code.chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

/// A code shown in two halves (`ABCD-EFGH`).
pub fn shown(code: &str) -> String {
    let (a, b) = code.split_at(code.len() / 2);
    format!("{a}-{b}")
}

impl Pairings {
    /// Forgets the codes that have run out.
    fn expire(&mut self, at: u64) {
        self.pending
            .retain(|_, request| at < request.made.saturating_add(CODE_SECS));
        if self.locked_until.is_some_and(|until| at >= until) {
            self.locked_until = None;
            self.wrong = 0;
        }
    }

    /// Whether `chat` was let in on `channel`.
    pub fn allows(&self, channel: &str, chat: &str) -> bool {
        self.allowed
            .get(channel)
            .is_some_and(|chats| chats.contains(chat))
    }

    /// What a message from `chat` (by `from`, saying `said`) on `channel`,
    /// which does not know it, gets at `at`.
    pub fn stranger(
        &mut self,
        channel: &str,
        chat: &str,
        from: &str,
        said: &str,
        at: u64,
    ) -> Stranger {
        self.expire(at);
        if self.allows(channel, chat) {
            return Stranger::Allowed;
        }
        let asked = self
            .pending
            .values()
            .filter(|r| r.channel == channel && r.chat == chat)
            .map(|r| r.made)
            .max();
        if asked.is_some_and(|made| at < made.saturating_add(ASK_EVERY_SECS)) {
            return Stranger::Quiet;
        }
        // A newer code replaces the sender's older one.
        self.pending
            .retain(|_, r| !(r.channel == channel && r.chat == chat));
        let waiting = self
            .pending
            .values()
            .filter(|r| r.channel == channel)
            .count();
        if waiting >= MAX_PENDING {
            return Stranger::Quiet;
        }
        let code = loop {
            let code = new_code();
            if !self.pending.contains_key(&code) {
                break code;
            }
        };
        self.pending.insert(
            code.clone(),
            Request {
                channel: channel.to_string(),
                chat: chat.to_string(),
                from: from.to_string(),
                said: said.chars().take(80).collect(),
                made: at,
            },
        );
        Stranger::Code(code)
    }

    /// Approves `code` at `at`: lets its chat in on its channel.
    pub fn approve(&mut self, code: &str, at: u64) -> Result<Request, String> {
        self.expire(at);
        if let Some(until) = self.locked_until {
            return Err(format!(
                "approvals are stopped for {} more minutes after {MAX_WRONG} wrong codes",
                until.saturating_sub(at).div_ceil(60)
            ));
        }
        match self.pending.remove(&normalized(code)) {
            Some(request) => {
                self.wrong = 0;
                self.allowed
                    .entry(request.channel.clone())
                    .or_default()
                    .insert(request.chat.clone());
                Ok(request)
            }
            None => {
                self.wrong += 1;
                if self.wrong >= MAX_WRONG {
                    self.locked_until = Some(at.saturating_add(LOCKOUT_SECS));
                }
                Err("no code like that is waiting (codes last an hour)".into())
            }
        }
    }

    /// Lets `chat` out of `channel`. Returns whether it was in.
    pub fn revoke(&mut self, channel: &str, chat: &str) -> bool {
        let removed = self
            .allowed
            .get_mut(channel)
            .is_some_and(|chats| chats.remove(chat));
        self.allowed.retain(|_, chats| !chats.is_empty());
        removed
    }
}

/// The store's file: `$SION_HOME/agents/pairing.json`.
pub fn path() -> PathBuf {
    crate::secrets::dir().join("pairing.json")
}

/// The store as it is.
pub fn load() -> Result<Pairings, CliError> {
    load_at(&path())
}

fn load_at(path: &Path) -> Result<Pairings, CliError> {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text)
            .map_err(|e| CliError::Usage(format!("{}: {e}", path.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Pairings::default()),
        Err(e) => Err(CliError::Usage(format!("{}: {e}", path.display()))),
    }
}

/// A lock on a store, held while it lives: a file made only where there
/// is none, removed when dropped.
struct Held(PathBuf);

impl Held {
    /// Waits for the lock at `path`, and takes it. One left by a process
    /// that stopped while holding it is taken over once stale (two
    /// processes taking over the same stale lock at once could both hold
    /// it: a crash and a race together).
    fn take(path: &Path) -> Result<Held, CliError> {
        let failed = |e: std::io::Error| CliError::Usage(format!("{}: {e}", path.display()));
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(failed)?;
        }
        let started = Instant::now();
        loop {
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
            {
                Ok(_) => return Ok(Held(path.to_path_buf())),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let age = std::fs::metadata(path)
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|at| at.elapsed().ok());
                    if age.is_some_and(|age| age.as_secs() >= STALE_LOCK_SECS) {
                        let _ = std::fs::remove_file(path);
                        continue;
                    }
                    if started.elapsed() > Duration::from_secs(STALE_LOCK_SECS + 5) {
                        return Err(CliError::Usage(format!(
                            "{} is held by another sion process",
                            path.display()
                        )));
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(e) => return Err(failed(e)),
            }
        }
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Changes the store at `path` with `change`, and keeps it if it changed,
/// readable only by this user: one change at a time across processes, so
/// none is lost.
fn change_at<T>(path: &Path, change: impl FnOnce(&mut Pairings) -> T) -> Result<T, CliError> {
    let _held = Held::take(&path.with_extension("lock"))?;
    let mut pairings = load_at(path)?;
    let before = pairings.clone();
    let out = change(&mut pairings);
    if pairings != before {
        let text =
            serde_json::to_string_pretty(&pairings).map_err(|e| CliError::Usage(e.to_string()))?;
        crate::secrets::write_private(path, text.as_bytes())?;
    }
    Ok(out)
}

/// What a message from a chat `channel` does not know gets, kept.
pub fn stranger(channel: &str, chat: &str, from: &str, said: &str) -> Result<Stranger, CliError> {
    change_at(&path(), |pairings| {
        pairings.stranger(channel, chat, from, said, now())
    })
}

/// `sion agent pairing …`
#[derive(Debug, clap::Subcommand)]
pub enum PairingCommand {
    /// The codes waiting for approval, and the chats let in.
    List,
    /// Let in the chat a code was given to, on its channel.
    Approve {
        /// The code (`ABCD-EFGH` or `ABCDEFGH`).
        code: String,
    },
    /// Let a chat out of a channel again.
    Revoke {
        /// The channel and profile, as `list` shows it (`telegram:support`).
        channel: String,
        /// The chat.
        chat: String,
    },
}

/// Runs a pairing command.
pub fn run(command: PairingCommand) -> Result<(), CliError> {
    match command {
        PairingCommand::List => {
            // Shown without the codes that ran out; they are forgotten at
            // the next change.
            let mut pairings = load()?;
            let at = now();
            pairings.expire(at);
            if pairings.pending.is_empty() {
                println!("no codes waiting");
            }
            for request in pairings.pending.values() {
                println!(
                    "waiting: {} chat {} ({}), {} minutes ago: {:?}",
                    request.channel,
                    request.chat,
                    request.from,
                    at.saturating_sub(request.made) / 60,
                    request.said
                );
            }
            for (channel, chats) in &pairings.allowed {
                for chat in chats {
                    println!("let in: {channel} chat {chat}");
                }
            }
        }
        PairingCommand::Approve { code } => {
            // A wrong code is kept too: it counts towards the lockout.
            let request = change_at(&path(), |pairings| pairings.approve(&code, now()))?
                .map_err(CliError::Usage)?;
            println!(
                "let in {} chat {} ({}): it is answered from its next message",
                request.channel, request.chat, request.from
            );
        }
        PairingCommand::Revoke { channel, chat } => {
            if change_at(&path(), |pairings| pairings.revoke(&channel, &chat))? {
                println!("let {channel} chat {chat} out");
            } else {
                return Err(CliError::Usage(format!(
                    "{channel} chat {chat} was not let in"
                )));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_spell_no_words_have_no_look_alikes_and_are_typed_loosely() {
        let mut seen = BTreeMap::new();
        for _ in 0..1000 {
            let code = new_code();
            assert_eq!(code.len(), CODE_LEN);
            assert!(code.bytes().all(|b| ALPHABET.contains(&b)), "{code}");
            for c in code.chars() {
                *seen.entry(c).or_insert(0u32) += 1;
            }
        }
        // 8,000 characters, 320 of each expected: each is drawn evenly.
        assert_eq!(seen.len(), ALPHABET.len());
        assert!(seen.values().all(|n| (200..450).contains(n)), "{seen:?}");
        assert!(!ALPHABET.iter().any(|b| b"AEIOUY0134L".contains(b)));
        assert_eq!(normalized(" abcd-efgh "), "ABCDEFGH");
        assert_eq!(shown("ABCDEFGH"), "ABCD-EFGH");
    }

    #[test]
    fn a_stranger_gets_a_code_that_lets_them_in_once_approved() {
        let mut p = Pairings::default();
        let Stranger::Code(code) = p.stranger("telegram:support", "42", "ana", "hello", 1000)
        else {
            panic!("no code");
        };
        // Asked again soon: nothing new.
        assert_eq!(
            p.stranger("telegram:support", "42", "ana", "hi?", 1100),
            Stranger::Quiet
        );
        // Ten minutes on, a new code replaces the old one.
        let Stranger::Code(newer) = p.stranger(
            "telegram:support",
            "42",
            "ana",
            "hi?",
            1000 + ASK_EVERY_SECS,
        ) else {
            panic!("no newer code");
        };
        assert_ne!(code, newer);
        assert!(p.approve(&code, 1700).is_err(), "the old code is gone");
        let request = p.approve(&shown(&newer).to_lowercase(), 1700).unwrap();
        assert_eq!(
            (request.channel.as_str(), request.chat.as_str()),
            ("telegram:support", "42")
        );
        assert!(p.allows("telegram:support", "42"));
        assert!(
            !p.allows("telegram:other", "42"),
            "only on that channel and profile"
        );
        assert_eq!(
            p.stranger("telegram:support", "42", "ana", "x", 1800),
            Stranger::Allowed
        );
        assert!(p.revoke("telegram:support", "42"));
        assert!(!p.allows("telegram:support", "42"));
    }

    #[test]
    fn codes_run_out_a_channel_has_few_waiting_and_guessing_stops() {
        let mut p = Pairings::default();
        for chat in ["1", "2", "3"] {
            assert!(matches!(
                p.stranger("slack:ops", chat, chat, "", 0),
                Stranger::Code(_)
            ));
        }
        assert_eq!(p.stranger("slack:ops", "4", "4", "", 10), Stranger::Quiet);
        // An hour on, the codes have run out, and room is made.
        assert!(matches!(
            p.stranger("slack:ops", "4", "4", "", CODE_SECS),
            Stranger::Code(_)
        ));
        assert_eq!(p.pending.len(), 1);
        for _ in 0..MAX_WRONG {
            assert!(p.approve("WRONGWRO", CODE_SECS + 1).is_err());
        }
        let code = p.pending.keys().next().unwrap().clone();
        let stopped = p.approve(&code, CODE_SECS + 2).unwrap_err();
        assert!(stopped.contains("stopped"), "{stopped}");
        // After the lockout, approving works again (the code still lasts).
        let mut fresh = p.clone();
        fresh.locked_until = Some(CODE_SECS + 10);
        assert!(fresh.approve(&code, CODE_SECS + 11).is_ok());
    }

    #[test]
    fn changes_made_at_once_are_all_kept_privately() {
        let dir = std::env::temp_dir().join(format!("sion-pairing-{}", crate::runtime::new_id()));
        let path = dir.join("pairing.json");
        let changers: Vec<_> = (0..4)
            .map(|_| {
                let path = path.clone();
                std::thread::spawn(move || {
                    for _ in 0..10 {
                        change_at(&path, |p| p.wrong += 1).unwrap();
                    }
                })
            })
            .collect();
        for changer in changers {
            changer.join().unwrap();
        }
        assert_eq!(load_at(&path).unwrap().wrong, 40, "no change was lost");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        // A lock left by a process that stopped is taken over once stale.
        let lock = path.with_extension("lock");
        let left = std::fs::File::create(&lock).unwrap();
        left.set_modified(std::time::SystemTime::now() - Duration::from_secs(STALE_LOCK_SECS + 1))
            .unwrap();
        change_at(&path, |p| p.wrong = 0).unwrap();
        assert!(!lock.exists());
        assert_eq!(load_at(&path).unwrap().wrong, 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
