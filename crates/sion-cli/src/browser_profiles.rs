//! Browser profiles a person signs in with (`sion agent browser login
//! <name>`), for an agent's browser to use (`--browser-profile <name>`):
//! their cookies and sessions, so that the agent acts on those sites as
//! them. Each is a Chromium profile kept in
//! `$SION_HOME/agents/browsers/<name>`, readable by the operator alone,
//! used by one browser at a time.
//!
//! Signing in opens Chromium in a window of its own with the profile, and
//! the person signs in to what they want the agent to reach; closing the
//! window keeps it. The profile keeps its passwords and cookies with
//! Chromium's basic store, as the agent's browser reads them: on Linux,
//! that is not the desktop's keyring, and they are as safe as the folder.

use std::path::PathBuf;
use std::process::{Command, Stdio};

use crate::error::CliError;

/// Where browser profiles are kept.
pub fn root() -> PathBuf {
    crate::profiles::path().with_file_name("browsers")
}

/// Where the profile `name` is kept.
pub fn dir(name: &str) -> PathBuf {
    root().join(name)
}

/// Checks that `name` can name a profile: 1 to 32 lowercase letters, digits
/// and hyphens.
pub fn check_name(name: &str) -> Result<(), String> {
    if crate::profiles::valid_name(name) {
        Ok(())
    } else {
        Err(format!(
            "{name:?} is not a browser profile's name: 1 to 32 lowercase letters, digits and hyphens"
        ))
    }
}

/// `sion agent browser`.
#[derive(Debug, clap::Subcommand)]
pub enum BrowserCommand {
    /// Sign in with a browser profile for agents to use: Chromium opens in
    /// a window of its own; sign in to what the agent should reach, then
    /// close the window.
    Login {
        /// The profile's name (made if new).
        name: String,
        /// The page to open first.
        #[arg(long, default_value = "about:blank")]
        url: String,
    },
    /// List the browser profiles.
    List,
    /// Remove a browser profile, and everything it was signed in to here.
    Remove {
        /// The profile's name.
        name: String,
    },
}

fn running_as_root() -> bool {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata("/proc/self").is_ok_and(|m| m.uid() == 0)
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

/// Runs `sion agent browser`.
pub fn command(command: BrowserCommand) -> Result<(), CliError> {
    let usage = CliError::Usage;
    match command {
        BrowserCommand::Login { name, url } => {
            check_name(&name).map_err(usage)?;
            if cfg!(target_os = "linux")
                && std::env::var_os("DISPLAY").is_none()
                && std::env::var_os("WAYLAND_DISPLAY").is_none()
            {
                return Err(usage(
                    "signing in opens a browser window: run this where there is a display".into(),
                ));
            }
            if running_as_root() {
                return Err(usage(
                    "Chromium does not start its sandbox as root: sign in as an ordinary user"
                        .into(),
                ));
            }
            let program = sion_agent::browser::find_program()
                .ok_or_else(|| usage("no Chromium or Chrome found: set SION_BROWSER".into()))?;
            let profile = dir(&name);
            std::fs::create_dir_all(&profile)
                .map_err(|e| usage(format!("{}: {e}", profile.display())))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                for at in [root(), profile.clone()] {
                    let _ = std::fs::set_permissions(&at, std::fs::Permissions::from_mode(0o700));
                }
            }
            // Held while the window is open, as an agent's browser holds it.
            sion_agent::browser::take_profile(&profile).map_err(usage)?;
            println!(
                "(sign in to what the agent should reach, then close the window: the profile is {})",
                profile.display()
            );
            let status = Command::new(&program)
                .arg(format!("--user-data-dir={}", profile.display()))
                // Kept as the agent's browser keeps them, to read them.
                .args([
                    "--password-store=basic",
                    "--use-mock-keychain",
                    "--no-first-run",
                    "--no-default-browser-check",
                ])
                .arg(&url)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            sion_agent::browser::release_profile(&profile);
            status.map_err(|e| usage(format!("{}: {e}", program.display())))?;
            println!(
                "kept browser profile {name}: give it to an agent's browser with --browser --browser-profile {name}"
            );
            Ok(())
        }
        BrowserCommand::List => {
            let mut names: Vec<String> = std::fs::read_dir(root())
                .map(|entries| {
                    entries
                        .flatten()
                        .filter(|e| e.path().is_dir())
                        .map(|e| e.file_name().to_string_lossy().into_owned())
                        .collect()
                })
                .unwrap_or_default();
            names.sort();
            if names.is_empty() {
                println!("no browser profiles (sion agent browser login <name>)");
            }
            for name in names {
                let busy = sion_agent::browser::in_use(&dir(&name));
                println!("{name}{}", if busy { " (in use)" } else { "" });
            }
            Ok(())
        }
        BrowserCommand::Remove { name } => {
            check_name(&name).map_err(usage)?;
            let profile = dir(&name);
            if !profile.is_dir() {
                return Err(usage(format!("there is no browser profile {name}")));
            }
            if sion_agent::browser::in_use(&profile) {
                return Err(usage(format!(
                    "the browser profile {name} is in use: end the session using it first"
                )));
            }
            std::fs::remove_dir_all(&profile)
                .map_err(|e| usage(format!("{}: {e}", profile.display())))?;
            println!("removed browser profile {name}");
            Ok(())
        }
    }
}
