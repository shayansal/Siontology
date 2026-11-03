//! A person's agents on a server of theirs, in one command: `sion deploy
//! user@host`. Over SSH (the `ssh` program, or `SION_SSH`), it:
//!
//! 1. asks the server what it is (`uname -sm`) and where its home is;
//! 2. puts this `sion` there, as `~/.local/bin/sion`, when the server is
//!    the same kind of machine (Linux on the same processor); otherwise it
//!    says to install a release there first;
//! 3. with `--copy-home`, copies this machine's `$SION_HOME` to the
//!    server's `~/.sion` (wallets and their keys, profiles, kept
//!    credentials, the chain's state) and rewrites the paths in it to the
//!    server's home (`sion agent up rehome`); otherwise runs `sion setup`
//!    there, in the terminal, for agents of the server's own;
//! 4. fills the server's list of what runs (`sion agent up init`), and
//!    installs `sion agent up` as a service of the user's there, started
//!    now and at boot (`loginctl enable-linger`, so that it runs with no
//!    one logged in).
//!
//! Copying a home moves its keys to the server: whoever controls the server
//! controls the agents' wallets (`deploy:copied-keys`). The agents here
//! should stop once the server's run, so that one copy answers the
//! messengers and works the watches.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use crate::error::CliError;

/// `sion deploy …`
#[derive(Debug, Clone, clap::Args)]
pub struct DeployArgs {
    /// The server: `user@host`, as SSH reaches it.
    pub destination: String,
    /// SSH's port on the server.
    #[arg(long)]
    pub port: Option<u16>,
    /// The key SSH signs in with.
    #[arg(long)]
    pub identity: Option<PathBuf>,
    /// Copy this machine's home (wallets and keys included) instead of
    /// running setup on the server.
    #[arg(long)]
    pub copy_home: bool,
    /// The profile whose work runner runs there (default: setup's).
    #[arg(long)]
    pub profile: Option<String>,
    /// Take GitHub's webhooks in the work runner there.
    #[arg(long)]
    pub webhooks: bool,
    /// Install the service, but do not start it.
    #[arg(long)]
    pub no_start: bool,
    /// Do not run setup on the server (with no --copy-home: set it up there
    /// yourself).
    #[arg(long)]
    pub no_setup: bool,
}

fn usage(e: impl std::fmt::Display) -> CliError {
    CliError::Usage(e.to_string())
}

/// The command that runs `remote` on the server, given `input` if any.
fn ssh(args: &DeployArgs, remote: &str, tty: bool) -> Command {
    let program = std::env::var("SION_SSH").unwrap_or_else(|_| "ssh".into());
    let mut command = Command::new(program);
    if let Some(port) = args.port {
        command.args(["-p", &port.to_string()]);
    }
    if let Some(identity) = &args.identity {
        command.arg("-i").arg(identity);
    }
    if tty {
        command.arg("-t");
    }
    command.arg("--").arg(&args.destination).arg(remote);
    command
}

/// Runs `remote` on the server and returns what it printed.
fn run_remote(args: &DeployArgs, remote: &str) -> Result<String, CliError> {
    let out = ssh(args, remote, false)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| usage(format!("ssh: {e}")))?;
    if !out.status.success() {
        return Err(usage(format!(
            "on {}: `{remote}` failed: {}",
            args.destination,
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Runs `remote` on the server with `input` as its standard input.
fn run_remote_with(args: &DeployArgs, remote: &str, input: &[u8]) -> Result<(), CliError> {
    let mut child = ssh(args, remote, false)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .map_err(|e| usage(format!("ssh: {e}")))?;
    child
        .stdin
        .take()
        .ok_or_else(|| usage("no pipe to ssh"))?
        .write_all(input)
        .map_err(usage)?;
    let status = child.wait().map_err(usage)?;
    if !status.success() {
        return Err(usage(format!("on {}: `{remote}` failed", args.destination)));
    }
    Ok(())
}

/// The processor names `uname -m` gives that mean the same as this
/// build's.
fn same_machine(uname: &str) -> bool {
    let mut words = uname.split_whitespace();
    let (Some(system), Some(machine)) = (words.next(), words.next()) else {
        return false;
    };
    let ours = match std::env::consts::ARCH {
        "x86_64" => ["x86_64", "amd64"],
        "aarch64" => ["aarch64", "arm64"],
        _ => return false,
    };
    std::env::consts::OS == "linux" && system == "Linux" && ours.contains(&machine)
}

/// Deploys: see the module's documentation. Returns what it did.
pub fn deploy(args: &DeployArgs) -> Result<Vec<String>, CliError> {
    let mut done = Vec::new();
    let said = run_remote(args, "uname -sm && echo \"$HOME\"")?;
    let mut lines = said.lines();
    let uname = lines.next().unwrap_or_default().trim().to_string();
    let remote_home = lines.next().unwrap_or_default().trim().to_string();
    if !same_machine(&uname) {
        return Err(usage(format!(
            "{} is {uname}, and this sion is built for {} {}: install a release there \
             (scripts/install.sh), then deploy again",
            args.destination,
            std::env::consts::OS,
            std::env::consts::ARCH
        )));
    }
    done.push(format!("{}: {uname}, home {remote_home}", args.destination));
    let exe = std::env::current_exe().map_err(usage)?;
    let bytes = std::fs::read(&exe).map_err(|e| usage(format!("{}: {e}", exe.display())))?;
    run_remote_with(
        args,
        "mkdir -p ~/.local/bin && cat > ~/.local/bin/sion.partial && chmod 755 ~/.local/bin/sion.partial && mv ~/.local/bin/sion.partial ~/.local/bin/sion",
        &bytes,
    )?;
    done.push(format!(
        "put sion there ({} MiB) as ~/.local/bin/sion",
        bytes.len() >> 20
    ));
    let sion = "~/.local/bin/sion";
    if args.copy_home {
        let home = crate::setup::home();
        let packed = Command::new("tar")
            .arg("-C")
            .arg(&home)
            .args([
                "--exclude=./logs",
                "--exclude=./up.status.json",
                "-czf",
                "-",
                ".",
            ])
            .stderr(Stdio::null())
            .output()
            .map_err(|e| usage(format!("tar: {e}")))?;
        if !packed.status.success() {
            return Err(usage(format!("{} could not be packed", home.display())));
        }
        run_remote_with(
            args,
            // tar restores the archive's own modes: the home is made the
            // user's alone again after it.
            "umask 077 && mkdir -p ~/.sion && chmod 700 ~/.sion && tar -C ~/.sion -xzf - \
             && chmod 700 ~/.sion && chmod -R go-rwx ~/.sion",
            &packed.stdout,
        )?;
        done.push(format!(
            "copied {} to the server's ~/.sion ({} KiB), its keys with it",
            home.display(),
            packed.stdout.len() >> 10
        ));
        if let Ok(local_home) = std::env::var("HOME") {
            let quoted = sion_agent::code::shell_quote(&local_home);
            let said = run_remote(args, &format!("{sion} agent up rehome --from {quoted}"))?;
            done.extend(said.lines().map(|l| format!("  {l}")));
        }
    } else if !args.no_setup {
        let status = ssh(args, &format!("{sion} setup"), true)
            .status()
            .map_err(|e| usage(format!("ssh: {e}")))?;
        if !status.success() {
            return Err(usage("setup on the server did not finish: run it there, then deploy again with --no-setup"));
        }
        done.push("set up the server's own agent".into());
    }
    let mut init = format!("{sion} agent up init");
    if let Some(profile) = &args.profile {
        init.push_str(&format!(
            " --profile {}",
            sion_agent::code::shell_quote(profile)
        ));
    }
    if args.webhooks {
        init.push_str(" --webhooks");
    }
    let said = run_remote(args, &init)?;
    done.extend(
        said.lines()
            .filter(|l| !l.starts_with("run it all"))
            .map(|l| format!("  runs {l}")),
    );
    let start = if args.no_start { "" } else { " --start" };
    let said = run_remote(
        args,
        &format!(
            "{sion} agent service install up{start} -- agent up && \
             (loginctl enable-linger \"$USER\" 2>/dev/null && echo 'it runs with no one logged in' \
             || echo 'lingering is not on: ask the administrator for loginctl enable-linger, or it stops at logout')"
        ),
    )?;
    done.extend(said.lines().map(|l| format!("  {l}")));
    done.push(format!(
        "on {}: `sion agent up list` says what runs, ~/.sion/logs holds what each said",
        args.destination
    ));
    if args.copy_home {
        done.push(
            "stop the agents on this machine, so that the server's copy is the one that answers"
                .into(),
        );
    }
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn machines_are_matched_by_system_and_processor() {
        let ours = format!("Linux {}", std::env::consts::ARCH);
        assert_eq!(same_machine(&ours), std::env::consts::OS == "linux");
        assert!(!same_machine("Darwin arm64") || std::env::consts::OS != "linux");
        assert!(!same_machine("Linux riscv64"));
        assert!(!same_machine(""));
    }
}
