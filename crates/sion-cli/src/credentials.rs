//! The credentials `sion` itself uses: models' keys and addresses, and
//! speech services' keys, kept on this machine so that `sion` and the
//! services it runs find them without the environment (`sion setup` keeps
//! the model's). They are sealed like this machine's other stores
//! (`agents/credentials.bin`, ChaCha20-Poly1305 under `agents/secrets.key`).
//! A variable set in the environment wins over a kept one. Skills' scripts
//! never receive these: theirs are `sion agent secret`'s. Whoever can read
//! the key file can read them (`agent:kept-credentials`), as whoever can
//! read a process's environment can read the variables it would otherwise
//! be given.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::error::CliError;

/// The store's file, in [`crate::secrets::dir`].
pub const FILE: &str = "credentials.bin";
/// What starts the file, and what its encryption is bound to.
pub const MAGIC: &[u8] = b"sion-agent-credentials/v1\n";

/// The variables that may be kept: the models' and the speech services'.
/// AWS's access keys are not among them: AWS's own files keep those.
pub const NAMES: [&str; 36] = [
    "SION_LOCAL_MODEL_URL",
    "SION_LOCAL_MODEL_KEY",
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_BASE_URL",
    "OPENAI_API_KEY",
    "OPENAI_BASE_URL",
    "GEMINI_API_KEY",
    "GOOGLE_API_KEY",
    "GEMINI_BASE_URL",
    "GROQ_API_KEY",
    "GROQ_BASE_URL",
    "OPENROUTER_API_KEY",
    "OPENROUTER_BASE_URL",
    "DEEPSEEK_API_KEY",
    "DEEPSEEK_BASE_URL",
    "MOONSHOT_API_KEY",
    "MOONSHOT_BASE_URL",
    "XAI_API_KEY",
    "XAI_BASE_URL",
    "TOGETHER_API_KEY",
    "TOGETHER_BASE_URL",
    "FIREWORKS_API_KEY",
    "FIREWORKS_BASE_URL",
    "MISTRAL_API_KEY",
    "MISTRAL_BASE_URL",
    "CEREBRAS_API_KEY",
    "CEREBRAS_BASE_URL",
    "AZURE_OPENAI_API_KEY",
    "AZURE_OPENAI_ENDPOINT",
    "AWS_BEARER_TOKEN_BEDROCK",
    "AWS_REGION",
    "GOOGLE_CLOUD_PROJECT",
    "GOOGLE_CLOUD_LOCATION",
    "DEEPGRAM_API_KEY",
    "ELEVENLABS_API_KEY",
    "ELEVENLABS_API_BASE",
];

/// What the file holds.
#[derive(Default, Serialize, Deserialize)]
struct Kept {
    values: BTreeMap<String, String>,
}

impl Drop for Kept {
    fn drop(&mut self) {
        for value in self.values.values_mut() {
            zeroize::Zeroize::zeroize(value);
        }
    }
}

/// The store in one directory.
#[derive(Clone, Debug)]
pub struct Credentials {
    dir: PathBuf,
}

impl Credentials {
    /// The store in `dir`.
    pub fn at(dir: impl Into<PathBuf>) -> Credentials {
        Credentials { dir: dir.into() }
    }

    /// This machine's (`$SION_HOME/agents`).
    pub fn here() -> Credentials {
        Credentials::at(crate::secrets::dir())
    }

    /// Its file.
    pub fn path(&self) -> PathBuf {
        self.dir.join(FILE)
    }

    fn load(&self) -> Result<Kept, CliError> {
        crate::secrets::open_json(&self.dir, FILE, MAGIC)
    }

    /// The value kept for `name`, if one is.
    pub fn get(&self, name: &str) -> Result<Option<Zeroizing<String>>, CliError> {
        Ok(self
            .load()?
            .values
            .get(name)
            .map(|v| Zeroizing::new(v.clone())))
    }

    /// The names kept.
    pub fn names(&self) -> Result<Vec<String>, CliError> {
        Ok(self.load()?.values.keys().cloned().collect())
    }

    /// Keeps `value` for `name`, one of [`NAMES`]; returns whether it
    /// replaced another.
    pub fn keep(&self, name: &str, value: &str) -> Result<bool, CliError> {
        check(name)?;
        let value = value.trim();
        if value.is_empty() || value.contains(['\n', '\r']) {
            return Err(CliError::Usage(format!(
                "{name}: the value is empty, or more than one line"
            )));
        }
        let mut kept = self.load()?;
        let replaced = kept
            .values
            .insert(name.to_string(), value.to_string())
            .is_some();
        crate::secrets::seal_json(&self.dir, FILE, MAGIC, &kept)?;
        Ok(replaced)
    }

    /// Forgets `name`; returns whether it was kept.
    pub fn forget(&self, name: &str) -> Result<bool, CliError> {
        let mut kept = self.load()?;
        let had = kept.values.remove(name).is_some();
        if had {
            crate::secrets::seal_json(&self.dir, FILE, MAGIC, &kept)?;
        }
        Ok(had)
    }
}

/// Whether `name` may be kept.
fn check(name: &str) -> Result<(), CliError> {
    if NAMES.contains(&name) {
        Ok(())
    } else {
        Err(CliError::Usage(format!(
            "{name} is not one sion keeps: {}",
            NAMES.join(", ")
        )))
    }
}

/// The value this machine keeps for `name`, read when asked (so that one
/// kept a moment ago is found); none if the store cannot be read (`sion
/// doctor` says why).
fn kept(name: &str) -> Option<Zeroizing<String>> {
    if !NAMES.contains(&name) {
        return None;
    }
    Credentials::here().get(name).ok().flatten()
}

/// The variable `name`: the environment's if it is set there and not
/// empty, else the one kept on this machine.
pub fn var(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| kept(name).map(|v| v.to_string()))
}

/// Whether `name` comes from what this machine keeps rather than the
/// environment.
pub fn is_kept(name: &str) -> bool {
    std::env::var(name).map_or(true, |v| v.trim().is_empty()) && kept(name).is_some()
}

/// `sion agent credential …`
#[derive(Debug, clap::Subcommand)]
pub enum CredentialCommand {
    /// Keep a model's or a speech service's credential on this machine: its
    /// value is read from standard input (not shown when typed), or taken
    /// from an environment variable.
    Add {
        /// Its name: ANTHROPIC_API_KEY, OPENAI_API_KEY, OPENAI_BASE_URL,
        /// GEMINI_API_KEY and the like.
        name: String,
        /// Take the value from this environment variable instead.
        #[arg(long, value_name = "VARIABLE")]
        from_env: Option<String>,
    },
    /// The names kept (never their values).
    List,
    /// Forget one.
    Remove {
        /// Its name.
        name: String,
    },
}

/// Runs a credential command.
pub fn run(command: CredentialCommand) -> Result<(), CliError> {
    let store = Credentials::here();
    match command {
        CredentialCommand::Add { name, from_env } => {
            check(&name)?;
            let value = match from_env {
                Some(variable) => Zeroizing::new(std::env::var(&variable).map_err(|_| {
                    CliError::Usage(format!("{variable} is not set (or is not UTF-8)"))
                })?),
                None => crate::secrets::read_value(&name)?,
            };
            let replaced = store.keep(&name, &value)?;
            println!(
                "{} {name}, kept sealed in {}",
                if replaced { "replaced" } else { "kept" },
                store.path().display()
            );
            Ok(())
        }
        CredentialCommand::List => {
            let names = store.names()?;
            if names.is_empty() {
                println!(
                    "none kept (sion setup keeps the model's; or sion agent credential add NAME)"
                );
            }
            for name in names {
                let shadowed = std::env::var(&name).is_ok_and(|v| !v.trim().is_empty());
                println!(
                    "{name}{}",
                    if shadowed {
                        "  (the environment's is used)"
                    } else {
                        ""
                    }
                );
            }
            Ok(())
        }
        CredentialCommand::Remove { name } => {
            if store.forget(&name)? {
                println!("forgot {name}");
                Ok(())
            } else {
                Err(CliError::Usage(format!("{name} is not kept")))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// The directory a test's store lives in (removed first).
    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("sion-credentials-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// Whether a file's bytes contain `needle`.
    fn contains(path: &Path, needle: &[u8]) -> bool {
        std::fs::read(path).is_ok_and(|bytes| bytes.windows(needle.len()).any(|w| w == needle))
    }

    #[test]
    fn credentials_are_kept_sealed_by_name_and_forgotten() {
        let store = Credentials::at(scratch("round-trip"));
        assert!(store.names().unwrap().is_empty());
        assert!(!store
            .keep("ANTHROPIC_API_KEY", "sk-ant-test-0001\n")
            .unwrap());
        assert!(store.keep("ANTHROPIC_API_KEY", "sk-ant-test-0002").unwrap());
        store
            .keep("OPENAI_BASE_URL", "http://127.0.0.1:11434/v1")
            .unwrap();
        assert_eq!(
            store.names().unwrap(),
            ["ANTHROPIC_API_KEY", "OPENAI_BASE_URL"]
        );
        assert_eq!(
            store
                .get("ANTHROPIC_API_KEY")
                .unwrap()
                .as_deref()
                .map(String::as_str),
            Some("sk-ant-test-0002")
        );
        // Neither a value nor a name is in the clear on disk, and only its
        // owner reads the file.
        for clear in [&b"sk-ant-test"[..], b"ANTHROPIC_API_KEY", b"11434"] {
            assert!(!contains(&store.path(), clear));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(store.path())
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o077, 0, "{mode:o}");
        }
        assert!(store.forget("OPENAI_BASE_URL").unwrap());
        assert!(!store.forget("OPENAI_BASE_URL").unwrap());
        assert_eq!(store.names().unwrap(), ["ANTHROPIC_API_KEY"]);
    }

    #[test]
    fn only_the_variables_sion_reads_are_kept_and_values_are_one_line() {
        let store = Credentials::at(scratch("refused"));
        assert!(store.keep("AWS_SECRET_ACCESS_KEY", "x").is_err());
        assert!(store.keep("TELEGRAM_BOT_TOKEN", "x").is_err());
        assert!(store.keep("OPENAI_API_KEY", "  ").is_err());
        assert!(store.keep("OPENAI_API_KEY", "a\nb").is_err());
        assert!(store.names().unwrap().is_empty());
    }
}
