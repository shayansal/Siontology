//! What an ACP service keeps across a restart: one JSON file, written whole
//! after every change (to a temporary file, synced, then renamed over the
//! old one, so a crash leaves the old state or the new, never half of
//! one), readable only by its owner.
//!
//! Card numbers are never kept here: the vault and the 3-D Secure sessions
//! hold them in memory only ([`super::psp`], [`super::three_ds`]).

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::error::CliError;

/// A state file.
#[derive(Clone, Debug)]
pub struct Store {
    path: PathBuf,
}

impl Store {
    /// The state kept at `path`.
    pub fn new(path: impl Into<PathBuf>) -> Store {
        Store { path: path.into() }
    }

    /// Where it is kept.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// What was kept, or the default if nothing was.
    pub fn load<T: DeserializeOwned + Default>(&self) -> Result<T, CliError> {
        match std::fs::read(&self.path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| {
                CliError::Usage(format!(
                    "{} is not this service's state: {e}",
                    self.path.display()
                ))
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
            Err(e) => Err(CliError::Usage(format!("{}: {e}", self.path.display()))),
        }
    }

    /// Keeps `value`, replacing what was kept.
    pub fn save<T: Serialize>(&self, value: &T) -> Result<(), CliError> {
        let io = |e: std::io::Error| CliError::Usage(format!("{}: {e}", self.path.display()));
        if let Some(parent) = self.path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(io)?;
        }
        let bytes = serde_json::to_vec(value).map_err(|e| CliError::Usage(e.to_string()))?;
        let temporary = self
            .path
            .with_extension(format!("tmp-{}", std::process::id()));
        {
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create(true).truncate(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temporary).map_err(io)?;
            file.write_all(&bytes).map_err(io)?;
            file.sync_all().map_err(io)?;
        }
        std::fs::rename(&temporary, &self.path).map_err(io)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn what_is_kept_reads_back_and_a_missing_file_is_nothing_kept() {
        let dir = std::env::temp_dir().join(format!("sion-acp-state-{}", std::process::id()));
        let store = Store::new(dir.join("state.json"));
        let empty: BTreeMap<String, u64> = store.load().unwrap();
        assert!(empty.is_empty());
        let kept = BTreeMap::from([("a".to_string(), 1u64)]);
        store.save(&kept).unwrap();
        assert_eq!(store.load::<BTreeMap<String, u64>>().unwrap(), kept);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(store.path())
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o077, 0, "only its owner reads it");
        }
        std::fs::write(store.path(), b"not json").unwrap();
        assert!(store.load::<BTreeMap<String, u64>>().is_err());
        let _ = std::fs::remove_dir_all(dir);
    }
}
