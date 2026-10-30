//! The operator's secrets: values that skills' scripts receive by name
//! (`sion-secrets` in a skill's metadata), kept encrypted at rest and never
//! shown to a model.
//!
//! The store is `agents/secrets.bin` in the `sion` home: its entries,
//! encrypted with ChaCha20-Poly1305 under a random key kept in
//! `agents/secrets.key`, readable only by its owner. A script that
//! declares a secret gets it as an environment variable, and its value is
//! cut out of what the script prints before the model sees it. What the
//! store does not protect against is named in the trust model
//! (`agent:secret-store`): whoever can read the key file can read the
//! secrets, and a script that receives one can send it wherever its skill
//! may connect.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::error::CliError;
use crate::session::wallet_dir;

/// What starts the store's file, and what its encryption is bound to.
pub const MAGIC: &[u8] = b"sion-agent-secrets/v1\n";

/// The most bytes of one secret.
pub const MAX_VALUE: usize = 16 << 10;

/// The most secrets in the store.
pub const MAX_SECRETS: usize = 256;

/// One secret.
#[derive(Clone, Serialize, Deserialize)]
pub struct Entry {
    /// Its value.
    pub value: String,
    /// When it was added (seconds since the Unix epoch).
    pub added: u64,
}

impl Drop for Entry {
    fn drop(&mut self) {
        zeroize::Zeroize::zeroize(&mut self.value);
    }
}

impl std::fmt::Debug for Entry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Entry({} bytes, added {})", self.value.len(), self.added)
    }
}

/// The store's contents.
#[derive(Default, Serialize, Deserialize)]
pub struct Store {
    /// The secrets, by name.
    pub secrets: BTreeMap<String, Entry>,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.secrets.keys()).finish()
    }
}

/// Where this machine's store is: `$SION_HOME/agents`.
pub fn dir() -> PathBuf {
    wallet_dir()
        .parent()
        .map(|home| home.join("agents"))
        .unwrap_or_else(|| PathBuf::from(".sion/agents"))
}

fn io(path: &Path, e: std::io::Error) -> CliError {
    CliError::Usage(format!("{}: {e}", path.display()))
}

/// Writes `bytes` to `path` atomically, readable only by its owner.
fn write_private(path: &Path, bytes: &[u8]) -> Result<(), CliError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| io(parent, e))?;
    }
    let partial = path.with_extension("partial");
    let _ = std::fs::remove_file(&partial);
    {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&partial).map_err(|e| io(&partial, e))?;
        std::io::Write::write_all(&mut file, bytes).map_err(|e| io(&partial, e))?;
        file.sync_all().map_err(|e| io(&partial, e))?;
    }
    std::fs::rename(&partial, path).map_err(|e| io(path, e))
}

/// The key of the stores in `dir` (`secrets.key`); made when `create` and
/// there is none yet.
fn key_at(dir: &Path, create: bool) -> Result<Option<Zeroizing<[u8; 32]>>, CliError> {
    let path = dir.join("secrets.key");
    match std::fs::read(&path) {
        Ok(bytes) => {
            let bytes = Zeroizing::new(bytes);
            let key: [u8; 32] = bytes
                .as_slice()
                .try_into()
                .map_err(|_| CliError::Usage(format!("{} is not a 32-byte key", path.display())))?;
            Ok(Some(Zeroizing::new(key)))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && create => {
            let mut key = Zeroizing::new([0u8; 32]);
            rand_core::RngCore::fill_bytes(&mut rand_core::OsRng, key.as_mut());
            write_private(&path, key.as_slice())?;
            Ok(Some(key))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(io(&path, e)),
    }
}

/// Writes `value`, as JSON, to `dir/file`, encrypted with
/// ChaCha20-Poly1305 under the key of the stores in `dir` (made if there
/// is none) and bound to `magic`, which starts the file.
pub fn seal_json(
    dir: &Path,
    file: &str,
    magic: &[u8],
    value: &impl Serialize,
) -> Result<(), CliError> {
    let key = key_at(dir, true)?.expect("made when missing");
    let plain =
        Zeroizing::new(serde_json::to_vec(value).map_err(|e| CliError::Usage(e.to_string()))?);
    let mut nonce = [0u8; 12];
    rand_core::RngCore::fill_bytes(&mut rand_core::OsRng, &mut nonce);
    let sealed = ChaCha20Poly1305::new(Key::from_slice(key.as_slice()))
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: plain.as_slice(),
                aad: magic,
            },
        )
        .map_err(|_| CliError::Usage("the store could not be encrypted".into()))?;
    let mut bytes = magic.to_vec();
    bytes.extend_from_slice(&nonce);
    bytes.extend_from_slice(&sealed);
    write_private(&dir.join(file), &bytes)
}

/// Reads what [`seal_json`] wrote to `dir/file`; the default when there
/// is no file.
pub fn open_json<T: serde::de::DeserializeOwned + Default>(
    dir: &Path,
    file: &str,
    magic: &[u8],
) -> Result<T, CliError> {
    let path = dir.join(file);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(T::default()),
        Err(e) => return Err(io(&path, e)),
    };
    let key = key_at(dir, false)?.ok_or_else(|| {
        CliError::Usage(format!(
            "{} has no key beside it ({}): it cannot be read",
            path.display(),
            dir.join("secrets.key").display()
        ))
    })?;
    let body = bytes
        .strip_prefix(magic)
        .filter(|b| b.len() >= 12)
        .ok_or_else(|| {
            CliError::Usage(format!("{} is not a store of this kind", path.display()))
        })?;
    let (nonce, sealed) = body.split_at(12);
    let plain = ChaCha20Poly1305::new(Key::from_slice(key.as_slice()))
        .decrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: sealed,
                aad: magic,
            },
        )
        .map_err(|_| {
            CliError::Usage(format!(
                "{} does not open with its key: it was changed, or the key was",
                path.display()
            ))
        })?;
    let plain = Zeroizing::new(plain);
    serde_json::from_slice(&plain).map_err(|e| CliError::Usage(format!("{}: {e}", path.display())))
}

/// A secret store: `secrets.bin` and its key, `secrets.key`, in a
/// directory.
#[derive(Clone, Debug)]
pub struct Vault {
    dir: PathBuf,
}

impl Vault {
    /// The store in `dir`.
    pub fn at(dir: impl Into<PathBuf>) -> Vault {
        Vault { dir: dir.into() }
    }

    /// This machine's store ([`dir`]).
    pub fn here() -> Vault {
        Vault::at(dir())
    }

    /// The file holding the store's key.
    pub fn key_path(&self) -> PathBuf {
        self.dir.join("secrets.key")
    }

    /// The file holding the encrypted secrets.
    pub fn store_path(&self) -> PathBuf {
        self.dir.join("secrets.bin")
    }

    /// The store as it is on disk (empty if there is none).
    pub fn load(&self) -> Result<Store, CliError> {
        open_json(&self.dir, "secrets.bin", MAGIC)
    }

    /// Writes the store, encrypted under a fresh nonce.
    pub fn save(&self, store: &Store) -> Result<(), CliError> {
        seal_json(&self.dir, "secrets.bin", MAGIC, store)
    }

    /// Adds (or replaces) the secret `name`. Returns whether it replaced
    /// one.
    pub fn add(&self, name: &str, value: &str) -> Result<bool, CliError> {
        if !sion_agent::permissions::valid_secret_name(name) {
            return Err(CliError::Usage(format!(
                "{name:?} is not a secret's name: capital letters, digits and underscores, as an \
                 environment variable"
            )));
        }
        if value.is_empty() || value.len() > MAX_VALUE {
            return Err(CliError::Usage(format!(
                "a secret has 1 to {MAX_VALUE} bytes"
            )));
        }
        let mut store = self.load()?;
        if store.secrets.len() >= MAX_SECRETS && !store.secrets.contains_key(name) {
            return Err(CliError::Usage(format!(
                "the store holds at most {MAX_SECRETS} secrets"
            )));
        }
        let added = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let replaced = store
            .secrets
            .insert(
                name.to_string(),
                Entry {
                    value: value.to_string(),
                    added,
                },
            )
            .is_some();
        self.save(&store)?;
        Ok(replaced)
    }

    /// Removes the secret `name`. Returns whether there was one.
    pub fn remove(&self, name: &str) -> Result<bool, CliError> {
        let mut store = self.load()?;
        let removed = store.secrets.remove(name).is_some();
        if removed {
            self.save(&store)?;
        }
        Ok(removed)
    }

    /// The secrets' names and when each was added, never their values.
    pub fn names(&self) -> Result<Vec<(String, u64)>, CliError> {
        Ok(self
            .load()?
            .secrets
            .iter()
            .map(|(name, entry)| (name.clone(), entry.added))
            .collect())
    }
}

/// The secrets as a session's tools see them: values by name, read once
/// when the session starts.
#[derive(Default)]
pub struct Snapshot {
    values: BTreeMap<String, Zeroizing<String>>,
}

impl std::fmt::Debug for Snapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Snapshot({} secrets)", self.values.len())
    }
}

impl Snapshot {
    /// This machine's store as it is now; empty if it cannot be read.
    pub fn load() -> Snapshot {
        Snapshot::of(&Vault::here())
    }

    /// `vault` as it is now; empty if it cannot be read.
    pub fn of(vault: &Vault) -> Snapshot {
        let values = vault
            .load()
            .map(|store| {
                store
                    .secrets
                    .iter()
                    .map(|(name, entry)| (name.clone(), Zeroizing::new(entry.value.clone())))
                    .collect()
            })
            .unwrap_or_default();
        Snapshot { values }
    }
}

impl sion_agent::permissions::Secrets for Snapshot {
    fn get(&self, name: &str) -> Option<String> {
        self.values.get(name).map(|v| v.to_string())
    }
}

/// The bridge program a script filtered by host runs behind inside the
/// sandbox: `SION_SANDBOX_BRIDGE` if set, else this `sion` program and its
/// `sandbox-bridge` command.
pub fn bridge() -> Option<Vec<String>> {
    if let Ok(path) = std::env::var("SION_SANDBOX_BRIDGE") {
        if !path.is_empty() {
            return Some(vec![path]);
        }
    }
    let exe = std::env::current_exe().ok()?;
    let name = exe.file_name()?.to_string_lossy().into_owned();
    (name == "sion" || name.starts_with("sion-") || name.starts_with("sion."))
        .then(|| vec![exe.to_string_lossy().into_owned(), "sandbox-bridge".into()])
}

/// `sion agent secret …`
#[derive(Debug, clap::Subcommand)]
pub enum SecretCommand {
    /// Add a secret, or replace one: its value is read from standard input
    /// (not shown when typed), or taken from an environment variable.
    Add {
        /// Its name, as the environment variable a script receives it in
        /// (capital letters, digits and underscores).
        name: String,
        /// Take the value from this environment variable instead.
        #[arg(long, value_name = "VARIABLE")]
        from_env: Option<String>,
    },
    /// The secrets' names (never their values) and when each was added.
    List,
    /// Remove a secret.
    Remove {
        /// Its name.
        name: String,
    },
}

/// Reads a secret's value from standard input: a line typed at a terminal,
/// with echo off, or everything piped in, less one final line break.
fn read_value() -> Result<Zeroizing<String>, CliError> {
    use std::io::{IsTerminal, Read};
    let stdin = std::io::stdin();
    let mut value = Zeroizing::new(String::new());
    if stdin.is_terminal() {
        let stty = |arg: &str| {
            std::process::Command::new("stty")
                .arg(arg)
                .stdin(std::process::Stdio::inherit())
                .status()
                .is_ok_and(|s| s.success())
        };
        eprint!("value (not shown): ");
        if !stty("-echo") {
            eprintln!();
            return Err(CliError::Usage(
                "what is typed could not be hidden (stty -echo): pipe the value in, or give --from-env"
                    .into(),
            ));
        }
        let read = stdin.read_line(&mut value);
        stty("echo");
        eprintln!();
        read.map_err(|e| CliError::Usage(format!("standard input: {e}")))?;
    } else {
        stdin
            .lock()
            .take(MAX_VALUE as u64 + 2)
            .read_to_string(&mut value)
            .map_err(|e| CliError::Usage(format!("standard input: {e}")))?;
    }
    if value.ends_with('\n') {
        value.pop();
        if value.ends_with('\r') {
            value.pop();
        }
    }
    Ok(value)
}

/// Runs a secret command.
pub fn run(command: SecretCommand) -> Result<(), CliError> {
    match command {
        SecretCommand::Add { name, from_env } => {
            let value = match from_env {
                Some(variable) => Zeroizing::new(std::env::var(&variable).map_err(|_| {
                    CliError::Usage(format!("{variable} is not set (or is not UTF-8)"))
                })?),
                None => read_value()?,
            };
            let vault = Vault::here();
            let replaced = vault.add(&name, &value)?;
            println!(
                "{} secret {name} ({} bytes), kept encrypted in {}",
                if replaced { "replaced" } else { "added" },
                value.len(),
                vault.store_path().display()
            );
            Ok(())
        }
        SecretCommand::List => {
            let names = Vault::here().names()?;
            if names.is_empty() {
                println!("no secrets (add one: sion agent secret add NAME)");
            }
            for (name, added) in names {
                println!("{name}  added {}", sion_agent::schedule::rfc3339(added));
            }
            Ok(())
        }
        SecretCommand::Remove { name } => {
            if Vault::here().remove(&name)? {
                println!("removed secret {name}");
                Ok(())
            } else {
                Err(CliError::Usage(format!("there is no secret {name}")))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sion_agent::permissions::Secrets;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sion-secrets-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn secrets_are_kept_encrypted_and_read_back_by_name() {
        let vault = Vault::at(temp("round-trip"));
        assert!(vault.names().unwrap().is_empty());
        assert!(!vault.add("API_TOKEN", "tok-5f2a9c41e7").unwrap());
        assert!(vault.add("API_TOKEN", "tok-replaced-0001").unwrap());
        vault.add("OTHER", "x").unwrap();
        let names: Vec<String> = vault.names().unwrap().into_iter().map(|(n, _)| n).collect();
        assert_eq!(names, ["API_TOKEN", "OTHER"]);
        // Neither the value nor its name is in the clear on disk.
        let bytes = std::fs::read(vault.store_path()).unwrap();
        assert!(bytes.starts_with(MAGIC));
        for clear in [&b"tok-replaced"[..], b"API_TOKEN"] {
            assert!(!bytes.windows(clear.len()).any(|w| w == clear));
        }
        let snapshot = Snapshot::of(&vault);
        assert_eq!(
            snapshot.get("API_TOKEN").as_deref(),
            Some("tok-replaced-0001")
        );
        assert_eq!(snapshot.get("MISSING"), None);
        assert_eq!(format!("{snapshot:?}"), "Snapshot(2 secrets)");
        assert!(!format!("{:?}", vault.load().unwrap()).contains("tok-"));
        assert!(vault.remove("OTHER").unwrap());
        assert!(!vault.remove("OTHER").unwrap());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for path in [vault.key_path(), vault.store_path()] {
                let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
                assert_eq!(mode, 0o600, "{}", path.display());
            }
        }
    }

    #[test]
    fn a_changed_store_or_key_does_not_open() {
        let vault = Vault::at(temp("tamper"));
        vault.add("API_TOKEN", "tok-5f2a9c41e7").unwrap();
        let path = vault.store_path();
        let mut bytes = std::fs::read(&path).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        std::fs::write(&path, &bytes).unwrap();
        let err = vault.load().unwrap_err().to_string();
        assert!(err.contains("does not open with its key"), "{err}");
        // An empty snapshot, not an error, for a session.
        assert_eq!(Snapshot::of(&vault).get("API_TOKEN"), None);
        std::fs::remove_file(vault.key_path()).unwrap();
        let err = vault.load().unwrap_err().to_string();
        assert!(err.contains("has no key beside it"), "{err}");
    }

    #[test]
    fn names_and_sizes_are_checked() {
        let vault = Vault::at(temp("checks"));
        for bad in [
            "lower",
            "1LEADING",
            "WITH-DASH",
            "",
            "PATH",
            "LD_PRELOAD",
            "HTTPS_PROXY",
        ] {
            assert!(vault.add(bad, "v").is_err(), "{bad}");
        }
        assert!(vault.add("EMPTY", "").is_err());
        assert!(vault.add("BIG", &"x".repeat(MAX_VALUE + 1)).is_err());
    }
}
