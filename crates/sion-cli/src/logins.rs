//! The operator's logins for agents' browsers: `sion agent login
//! add|list|remove`.
//!
//! A login is a user name and a password bound to one origin
//! (`https://accounts.example.com`). It is kept encrypted beside the
//! secrets (`agents/logins.bin`, under the same key, see
//! [`crate::secrets`]). An agent with the logins names one in
//! `browser_fill_login`; the browser fills it only into a page of its
//! origin, and the model never reads it: what the browser shows after has
//! the password cut out.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::error::CliError;

/// What the store's file starts with, and what its encryption is bound to.
pub const MAGIC: &[u8] = b"sion-agent-logins/v1\n";

/// The file in the store's directory.
pub const FILE: &str = "logins.bin";

/// One login.
#[derive(Clone, Serialize, Deserialize)]
pub struct Entry {
    /// The origin it is for.
    pub origin: String,
    /// The user name.
    pub username: String,
    /// The password.
    pub password: String,
    /// When it was added (seconds since the Unix epoch).
    pub added: u64,
}

impl Drop for Entry {
    fn drop(&mut self) {
        zeroize::Zeroize::zeroize(&mut self.password);
    }
}

impl std::fmt::Debug for Entry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Entry")
            .field("origin", &self.origin)
            .finish_non_exhaustive()
    }
}

/// The store's contents.
#[derive(Default, Serialize, Deserialize)]
pub struct Store {
    /// By name.
    pub logins: BTreeMap<String, Entry>,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.logins.keys()).finish()
    }
}

/// Whether `name` can name a login: 1 to 32 lowercase letters, digits and
/// hyphens.
pub fn valid_name(name: &str) -> bool {
    (1..=32).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// The origin of `url` as browsers write it: `scheme://host[:port]`,
/// lower-cased, without a default port.
pub fn origin_of(url: &str) -> Result<String, CliError> {
    let (scheme, rest) = url
        .split_once("://")
        .ok_or_else(|| CliError::Usage(format!("{url} is not a URL: give https://host")))?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "https" && scheme != "http" {
        return Err(CliError::Usage(format!(
            "{url}: a login's origin is http or https"
        )));
    }
    let authority = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if authority.is_empty() || authority.contains('@') {
        return Err(CliError::Usage(format!("{url} has no host")));
    }
    let authority = match (scheme.as_str(), authority.rsplit_once(':')) {
        ("https", Some((host, "443"))) | ("http", Some((host, "80"))) => host.to_string(),
        _ => authority,
    };
    Ok(format!("{scheme}://{authority}"))
}

/// A store in a directory.
#[derive(Clone, Debug)]
pub struct Logins {
    dir: PathBuf,
}

impl Logins {
    /// The store in `dir`.
    pub fn at(dir: impl Into<PathBuf>) -> Logins {
        Logins { dir: dir.into() }
    }

    /// This machine's store.
    pub fn here() -> Logins {
        Logins::at(crate::secrets::dir())
    }

    /// Everything in it.
    pub fn load(&self) -> Result<Store, CliError> {
        crate::secrets::open_json(&self.dir, FILE, MAGIC)
    }

    /// Adds or replaces a login. Returns whether it replaced one.
    pub fn add(
        &self,
        name: &str,
        origin: &str,
        username: &str,
        password: &str,
    ) -> Result<bool, CliError> {
        if !valid_name(name) {
            return Err(CliError::Usage(format!(
                "{name:?} is not a login's name: 1 to 32 lowercase letters, digits and hyphens"
            )));
        }
        if password.is_empty() || password.len() > 4096 || username.len() > 1024 {
            return Err(CliError::Usage(
                "a password has 1 to 4096 bytes, a user name at most 1024".into(),
            ));
        }
        let origin = origin_of(origin)?;
        let mut store = self.load()?;
        let added = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let replaced = store
            .logins
            .insert(
                name.to_string(),
                Entry {
                    origin,
                    username: username.to_string(),
                    password: password.to_string(),
                    added,
                },
            )
            .is_some();
        crate::secrets::seal_json(&self.dir, FILE, MAGIC, &store)?;
        Ok(replaced)
    }

    /// Removes a login. Returns whether there was one.
    pub fn remove(&self, name: &str) -> Result<bool, CliError> {
        let mut store = self.load()?;
        let removed = store.logins.remove(name).is_some();
        if removed {
            crate::secrets::seal_json(&self.dir, FILE, MAGIC, &store)?;
        }
        Ok(removed)
    }
}

/// The logins as a session's browser sees them, read once.
#[derive(Default)]
pub struct Snapshot {
    logins: BTreeMap<String, (String, String, Zeroizing<String>)>,
}

impl std::fmt::Debug for Snapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Snapshot({} logins)", self.logins.len())
    }
}

impl Snapshot {
    /// `logins` as they are now; empty if they cannot be read.
    pub fn of(logins: &Logins) -> Snapshot {
        Snapshot {
            logins: logins
                .load()
                .map(|store| {
                    store
                        .logins
                        .iter()
                        .map(|(name, e)| {
                            (
                                name.clone(),
                                (
                                    e.origin.clone(),
                                    e.username.clone(),
                                    Zeroizing::new(e.password.clone()),
                                ),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default(),
        }
    }
}

impl sion_agent::browser::Credentials for Snapshot {
    fn get(&self, name: &str) -> Option<sion_agent::browser::Credential> {
        self.logins.get(name).map(
            |(origin, username, password)| sion_agent::browser::Credential {
                origin: origin.clone(),
                username: username.clone(),
                password: password.to_string(),
            },
        )
    }

    fn list(&self) -> Vec<(String, String)> {
        self.logins
            .iter()
            .map(|(name, (origin, _, _))| (name.clone(), origin.clone()))
            .collect()
    }
}

/// `sion agent login …`
#[derive(Debug, clap::Subcommand)]
pub enum LoginCommand {
    /// Add a login, or replace one: its password is read from standard
    /// input (not shown when typed), or taken from an environment variable.
    Add {
        /// Its name (lowercase letters, digits and hyphens), by which an
        /// agent asks for it.
        name: String,
        /// The origin it is for: https://accounts.example.com. It is filled
        /// only into pages of that origin.
        #[arg(long)]
        origin: String,
        /// The user name.
        #[arg(long)]
        username: String,
        /// Take the password from this environment variable instead.
        #[arg(long, value_name = "VARIABLE")]
        password_env: Option<String>,
    },
    /// The logins' names, origins and user names (never their passwords).
    List,
    /// Remove a login.
    Remove {
        /// Its name.
        name: String,
    },
}

/// Runs a login command.
pub fn run(command: LoginCommand) -> Result<(), CliError> {
    let logins = Logins::here();
    match command {
        LoginCommand::Add {
            name,
            origin,
            username,
            password_env,
        } => {
            let password = match password_env {
                Some(variable) => Zeroizing::new(std::env::var(&variable).map_err(|_| {
                    CliError::Usage(format!("{variable} is not set (or is not UTF-8)"))
                })?),
                None => crate::secrets::read_value("password")?,
            };
            let replaced = logins.add(&name, &origin, &username, &password)?;
            println!(
                "{} login {name} for {} as {username}, kept encrypted",
                if replaced { "replaced" } else { "added" },
                origin_of(&origin)?
            );
        }
        LoginCommand::List => {
            let store = logins.load()?;
            if store.logins.is_empty() {
                println!("no logins (add one: sion agent login add NAME --origin https://… --username …)");
            }
            for (name, entry) in &store.logins {
                println!("{name}  {}  as {}", entry.origin, entry.username);
            }
        }
        LoginCommand::Remove { name } => {
            if logins.remove(&name)? {
                println!("removed login {name}");
            } else {
                return Err(CliError::Usage(format!("there is no login {name}")));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sion_agent::browser::Credentials;

    #[test]
    fn origins_are_written_as_browsers_write_them() {
        assert_eq!(
            origin_of("https://Accounts.Example.com/login?x").unwrap(),
            "https://accounts.example.com"
        );
        assert_eq!(
            origin_of("https://example.com:443").unwrap(),
            "https://example.com"
        );
        assert_eq!(
            origin_of("http://127.0.0.1:8080/a").unwrap(),
            "http://127.0.0.1:8080"
        );
        assert!(origin_of("ftp://example.com").is_err());
        assert!(origin_of("https://user@example.com").is_err());
        assert!(origin_of("example.com").is_err());
    }

    #[test]
    fn logins_are_kept_encrypted_and_read_by_name() {
        let dir = std::env::temp_dir().join(format!("sion-logins-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let logins = Logins::at(&dir);
        assert!(!logins
            .add("shop", "https://shop.example.com/login", "ana", "pa55-w0rd")
            .unwrap());
        let raw = std::fs::read(dir.join(FILE)).unwrap();
        assert!(raw.starts_with(MAGIC));
        assert!(!raw.windows(9).any(|w| w == b"pa55-w0rd"));
        let snapshot = Snapshot::of(&logins);
        let login = snapshot.get("shop").unwrap();
        assert_eq!(
            (login.origin.as_str(), login.username.as_str()),
            ("https://shop.example.com", "ana")
        );
        assert_eq!(login.password, "pa55-w0rd");
        assert_eq!(
            snapshot.list(),
            vec![("shop".to_string(), "https://shop.example.com".to_string())]
        );
        assert!(logins
            .add("Bad Name", "https://x.example", "u", "p")
            .is_err());
        assert!(logins.remove("shop").unwrap());
        assert!(Snapshot::of(&logins).get("shop").is_none());
    }
}
