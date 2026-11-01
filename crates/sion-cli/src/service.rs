//! Long-running commands as services of the user's own, so that a bridge,
//! a server, the scheduler or a host starts with the session, and starts
//! again if it stops: a systemd user unit on Linux, a launchd agent on
//! macOS.
//!
//! `sion agent service install support -- agent telegram support --pairing`
//! writes `sion-support.service` (or `org.siontology.sion.support.plist`)
//! running this `sion` with those arguments and this `SION_HOME`. The
//! variables `--env` names (a bot's token) are copied from the environment
//! into a file only the user reads: systemd's `EnvironmentFile`, or the
//! agent's own file on macOS. `--start` starts it now; `remove` stops it
//! and takes its files away.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::CliError;

/// `sion agent service …`
#[derive(Debug, clap::Subcommand)]
pub enum ServiceCommand {
    /// Install a service that runs `sion` with the arguments after `--`
    /// (e.g. `sion agent service install support -- agent telegram support
    /// --pairing`).
    Install {
        /// Its name: letters, digits, `-` and `_`.
        name: String,
        /// A variable it is given, by name, from this environment (a token):
        /// kept in a file only your user reads. May be repeated.
        #[arg(long = "env", value_name = "NAME")]
        env: Vec<String>,
        /// Start it now, and at each login.
        #[arg(long)]
        start: bool,
        /// What it runs: `sion` and these arguments.
        #[arg(last = true, required = true)]
        args: Vec<String>,
    },
    /// The services installed.
    List,
    /// Stop a service and take its files away.
    Remove {
        /// Its name.
        name: String,
    },
}

/// Which service manager, and where its files go.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Manager {
    /// systemd's user manager: units in this directory, environment files
    /// in the other.
    Systemd { units: PathBuf, env: PathBuf },
    /// launchd: agents in this directory, logs in the other.
    Launchd { agents: PathBuf, logs: PathBuf },
}

impl Manager {
    /// This machine's, from `HOME` (and `XDG_CONFIG_HOME` on Linux).
    pub fn here() -> Result<Manager, CliError> {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| CliError::Usage("HOME is not set".into()))?;
        if cfg!(target_os = "macos") {
            return Ok(Manager::Launchd {
                agents: home.join("Library/LaunchAgents"),
                logs: home.join("Library/Logs/sion"),
            });
        }
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| home.join(".config"));
        Ok(Manager::Systemd {
            units: config.join("systemd/user"),
            env: config.join("sion/services"),
        })
    }

    /// The service file for `name`.
    pub fn file(&self, name: &str) -> PathBuf {
        match self {
            Manager::Systemd { units, .. } => units.join(format!("sion-{name}.service")),
            Manager::Launchd { agents, .. } => agents.join(format!("{}.plist", label(name))),
        }
    }
}

/// launchd's label for `name`.
fn label(name: &str) -> String {
    format!("org.siontology.sion.{name}")
}

fn usage(e: impl std::fmt::Display) -> CliError {
    CliError::Usage(e.to_string())
}

/// Whether `name` may name a service.
pub fn check_name(name: &str) -> Result<(), CliError> {
    if name.is_empty()
        || name.len() > 64
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(CliError::Usage(format!(
            "{name:?}: a service's name is letters, digits, - and _"
        )));
    }
    Ok(())
}

/// One argument of `ExecStart`, quoted as systemd reads it: in double
/// quotes, with `\` and `"` escaped, and `$` and `%` doubled so that
/// systemd expands nothing in it.
pub fn systemd_word(word: &str) -> String {
    let mut out = String::from("\"");
    for c in word.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '$' => out.push_str("$$"),
            '%' => out.push_str("%%"),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A systemd user unit running `program` with `args`, its environment in
/// `env_file`.
pub fn systemd_unit(name: &str, program: &Path, args: &[String], env_file: &Path) -> String {
    let exec: Vec<String> = std::iter::once(program.display().to_string())
        .chain(args.iter().cloned())
        .map(|w| systemd_word(&w))
        .collect();
    format!(
        "# Written by `sion agent service install {name}`.\n\
         [Unit]\n\
         Description=sion {}\n\
         After=network-online.target\n\
         Wants=network-online.target\n\
         \n\
         [Service]\n\
         ExecStart={}\n\
         EnvironmentFile={}\n\
         Restart=on-failure\n\
         RestartSec=5\n\
         NoNewPrivileges=yes\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n",
        args.join(" ").replace('\n', " "),
        exec.join(" "),
        systemd_word(&env_file.display().to_string()),
    )
}

/// An environment file's line for `name` = `value`, as systemd reads it.
pub fn env_line(name: &str, value: &str) -> String {
    let quoted: String = value
        .chars()
        .flat_map(|c| match c {
            '\\' => vec!['\\', '\\'],
            '"' => vec!['\\', '"'],
            '\n' => vec!['\\', 'n'],
            c => vec![c],
        })
        .collect();
    format!("{name}=\"{quoted}\"\n")
}

fn xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// A launchd agent running `program` with `args` and `env`, logging to
/// `log`.
pub fn launchd_agent(
    name: &str,
    program: &Path,
    args: &[String],
    env: &[(String, String)],
    log: &Path,
) -> String {
    let words: String = std::iter::once(program.display().to_string())
        .chain(args.iter().cloned())
        .map(|w| format!("    <string>{}</string>\n", xml(&w)))
        .collect();
    let vars: String = env
        .iter()
        .map(|(n, v)| {
            format!(
                "    <key>{}</key>\n    <string>{}</string>\n",
                xml(n),
                xml(v)
            )
        })
        .collect();
    let log = xml(&log.display().to_string());
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <!-- Written by `sion agent service install {name}`. -->\n\
         <plist version=\"1.0\">\n<dict>\n\
         \x20 <key>Label</key>\n  <string>{}</string>\n\
         \x20 <key>ProgramArguments</key>\n  <array>\n{words}  </array>\n\
         \x20 <key>EnvironmentVariables</key>\n  <dict>\n{vars}  </dict>\n\
         \x20 <key>RunAtLoad</key>\n  <true/>\n\
         \x20 <key>KeepAlive</key>\n  <dict>\n    <key>SuccessfulExit</key>\n    <false/>\n  </dict>\n\
         \x20 <key>StandardOutPath</key>\n  <string>{log}</string>\n\
         \x20 <key>StandardErrorPath</key>\n  <string>{log}</string>\n\
         </dict>\n</plist>\n",
        xml(&label(name))
    )
}

/// Writes `text` to `path`, readable only by this user.
fn write_private(path: &Path, text: &str) -> Result<(), CliError> {
    crate::secrets::write_private(path, text.as_bytes())
}

/// What a service is given: `SION_HOME`, `SION_NODE` if set, and the
/// variables `names` names, from this environment.
fn environment(names: &[String]) -> Result<Vec<(String, String)>, CliError> {
    let home = crate::session::wallet_dir()
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| usage("no SION_HOME"))?;
    let mut env = vec![("SION_HOME".to_string(), home.display().to_string())];
    if let Ok(node) = std::env::var("SION_NODE") {
        env.push(("SION_NODE".into(), node));
    }
    for name in names {
        if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
            return Err(usage(format!("{name:?} is not a variable's name")));
        }
        let value = std::env::var(name).map_err(|_| {
            usage(format!(
                "{name} is not set here, so it cannot be given to the service"
            ))
        })?;
        env.push((name.clone(), value));
    }
    Ok(env)
}

/// Installs `name` with `manager`: what it runs, with `env`. Returns the
/// files written.
pub fn install(
    manager: &Manager,
    name: &str,
    program: &Path,
    args: &[String],
    env: &[(String, String)],
) -> Result<Vec<PathBuf>, CliError> {
    check_name(name)?;
    let file = manager.file(name);
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(usage)?;
    }
    match manager {
        Manager::Systemd { env: dir, .. } => {
            let env_file = dir.join(format!("{name}.env"));
            let lines: String = env.iter().map(|(n, v)| env_line(n, v)).collect();
            write_private(&env_file, &lines)?;
            std::fs::write(&file, systemd_unit(name, program, args, &env_file)).map_err(usage)?;
            Ok(vec![file, env_file])
        }
        Manager::Launchd { logs, .. } => {
            std::fs::create_dir_all(logs).map_err(usage)?;
            let log = logs.join(format!("{name}.log"));
            // The agent holds the tokens: only this user reads it.
            write_private(&file, &launchd_agent(name, program, args, env, &log))?;
            Ok(vec![file])
        }
    }
}

/// The services installed with `manager`: each name and what it runs.
pub fn installed(manager: &Manager) -> Result<Vec<(String, String)>, CliError> {
    let (dir, prefix, suffix) = match manager {
        Manager::Systemd { units, .. } => (units, "sion-".to_string(), ".service"),
        Manager::Launchd { agents, .. } => (agents, label(""), ".plist"),
    };
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(usage(e)),
    };
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let file = entry.file_name().to_string_lossy().into_owned();
        let Some(name) = file
            .strip_prefix(&prefix)
            .and_then(|f| f.strip_suffix(suffix))
        else {
            continue;
        };
        let text = std::fs::read_to_string(entry.path()).unwrap_or_default();
        let runs = text
            .lines()
            .find_map(|l| l.strip_prefix("Description=sion "))
            .map(str::to_string)
            .unwrap_or_else(|| "(see its file)".into());
        found.push((name.to_string(), runs));
    }
    found.sort();
    Ok(found)
}

/// The user's id, for launchd's domain.
fn uid() -> Result<String, CliError> {
    let out = Command::new("id").arg("-u").output().map_err(usage)?;
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn run_manager(program: &str, args: &[&str]) -> Result<(), CliError> {
    let status = Command::new(program)
        .args(args)
        .status()
        .map_err(|e| usage(format!("{program}: {e}")))?;
    if !status.success() {
        return Err(usage(format!(
            "{program} {} failed ({status})",
            args.join(" ")
        )));
    }
    Ok(())
}

/// Starts `name`, now and at each login.
pub fn start(manager: &Manager, name: &str) -> Result<(), CliError> {
    match manager {
        Manager::Systemd { .. } => {
            run_manager("systemctl", &["--user", "daemon-reload"])?;
            run_manager(
                "systemctl",
                &["--user", "enable", "--now", &format!("sion-{name}.service")],
            )
        }
        Manager::Launchd { .. } => {
            let domain = format!("gui/{}", uid()?);
            run_manager(
                "launchctl",
                &[
                    "bootstrap",
                    &domain,
                    &manager.file(name).display().to_string(),
                ],
            )
        }
    }
}

/// Stops `name`, if it runs.
fn stop(manager: &Manager, name: &str) {
    let _ = match manager {
        Manager::Systemd { .. } => Command::new("systemctl")
            .args([
                "--user",
                "disable",
                "--now",
                &format!("sion-{name}.service"),
            ])
            .status(),
        Manager::Launchd { .. } => match uid() {
            Ok(uid) => Command::new("launchctl")
                .args(["bootout", &format!("gui/{uid}/{}", label(name))])
                .status(),
            Err(_) => return,
        },
    };
}

/// Runs a service command.
pub fn run(command: ServiceCommand) -> Result<(), CliError> {
    let manager = Manager::here()?;
    match command {
        ServiceCommand::Install {
            name,
            env,
            start: now,
            args,
        } => {
            let program = std::env::current_exe().map_err(usage)?;
            let env = environment(&env)?;
            let files = install(&manager, &name, &program, &args, &env)?;
            for file in &files {
                println!("wrote {}", file.display());
            }
            if now {
                start(&manager, &name)?;
                println!("service {name} started; it starts again at each login");
            } else {
                match &manager {
                    Manager::Systemd { .. } => {
                        println!("start it with: systemctl --user enable --now sion-{name}.service")
                    }
                    Manager::Launchd { .. } => println!(
                        "start it with: launchctl bootstrap gui/$(id -u) {}",
                        manager.file(&name).display()
                    ),
                }
            }
            Ok(())
        }
        ServiceCommand::List => {
            let services = installed(&manager)?;
            if services.is_empty() {
                println!("no services");
            }
            for (name, runs) in services {
                println!("{name}: sion {runs}");
            }
            Ok(())
        }
        ServiceCommand::Remove { name } => {
            check_name(&name)?;
            let file = manager.file(&name);
            if !file.exists() {
                return Err(usage(format!("no service {name}")));
            }
            stop(&manager, &name);
            std::fs::remove_file(&file).map_err(usage)?;
            if let Manager::Systemd { env, .. } = &manager {
                let _ = std::fs::remove_file(env.join(format!("{name}.env")));
                let _ = Command::new("systemctl")
                    .args(["--user", "daemon-reload"])
                    .status();
            }
            println!("removed service {name}");
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn units_quote_what_they_run_and_keep_tokens_apart() {
        let unit = systemd_unit(
            "support",
            Path::new("/opt/sion/bin/sion"),
            &[
                "agent".into(),
                "telegram".into(),
                "support".into(),
                "--allow".into(),
                "4$2 %h \"x\"".into(),
            ],
            Path::new("/home/ana/.config/sion/services/support.env"),
        );
        assert!(unit.contains(
            "ExecStart=\"/opt/sion/bin/sion\" \"agent\" \"telegram\" \"support\" \"--allow\" \"4$$2 %%h \\\"x\\\"\"\n"
        ), "{unit}");
        assert!(unit.contains("EnvironmentFile=\"/home/ana/.config/sion/services/support.env\""));
        assert!(unit.contains("Restart=on-failure"));
        assert_eq!(env_line("TOKEN", "a\"b\\c"), "TOKEN=\"a\\\"b\\\\c\"\n");
        let agent = launchd_agent(
            "support",
            Path::new("/usr/local/bin/sion"),
            &["agent".into(), "serve".into(), "<&>".into()],
            &[("SION_HOME".into(), "/Users/ana/.sion".into())],
            Path::new("/Users/ana/Library/Logs/sion/support.log"),
        );
        assert!(agent.contains("<string>org.siontology.sion.support</string>"));
        assert!(agent.contains("<string>&lt;&amp;&gt;</string>"));
        assert!(agent.contains("<key>SION_HOME</key>\n    <string>/Users/ana/.sion</string>"));
        assert!(check_name("support-1").is_ok());
        assert!(check_name("../x").is_err() && check_name("").is_err());
    }

    #[test]
    fn services_are_installed_listed_and_private() {
        let dir = std::env::temp_dir().join(format!("sion-service-{}", crate::runtime::new_id()));
        for manager in [
            Manager::Systemd {
                units: dir.join("systemd/user"),
                env: dir.join("sion/services"),
            },
            Manager::Launchd {
                agents: dir.join("LaunchAgents"),
                logs: dir.join("Logs/sion"),
            },
        ] {
            let env = vec![("TELEGRAM_BOT_TOKEN".to_string(), "123:abc".to_string())];
            let files = install(
                &manager,
                "support",
                Path::new("/usr/local/bin/sion"),
                &["agent".into(), "telegram".into(), "support".into()],
                &env,
            )
            .unwrap();
            let holding = files.last().unwrap();
            assert!(std::fs::read_to_string(holding)
                .unwrap()
                .contains("123:abc"));
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = std::fs::metadata(holding).unwrap().permissions().mode() & 0o777;
                assert_eq!(mode, 0o600, "{}", holding.display());
            }
            if let Manager::Systemd { .. } = manager {
                assert!(
                    !std::fs::read_to_string(&files[0])
                        .unwrap()
                        .contains("123:abc"),
                    "the unit holds no token"
                );
                assert_eq!(
                    installed(&manager).unwrap(),
                    [("support".to_string(), "agent telegram support".to_string())]
                );
            } else {
                assert_eq!(installed(&manager).unwrap().len(), 1);
            }
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
