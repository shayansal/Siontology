//! Everything a person's agents run, kept running by one process: `sion
//! agent up`. Its list (`$SION_HOME/up.json`) names each thing it runs, a
//! name and the arguments of `sion` that run it: the chain, a messenger's
//! bridge, the work runner, the scheduler. `sion agent up init` fills the
//! list from what `sion setup` made; `add` and `remove` change it.
//!
//! Each runs as a process of its own, its output in
//! `$SION_HOME/logs/<name>.log` (the log before it kept as `.log.1` once
//! it grows past [`LOG_BYTES`]). One that stops is started again, after a
//! pause that doubles each time it stops soon after starting
//! ([`BACKOFF_MOST`] at most) and starts over once it has run a minute.
//! Each is given a pipe that `sion agent up` holds: when `sion agent up`
//! stops, however it stops, the pipe closes and they stop too, so that
//! none is left running without it. What runs, since when and how often
//! it was started again is kept in `$SION_HOME/up.status.json` (`sion
//! agent up list`).
//!
//! One service of the machine's (systemd, launchd, a container) runs `sion
//! agent up`, and with it everything: on a server, that keeps the agents
//! working while the person's own computer is off ([`crate::deploy`]).

use std::collections::BTreeMap;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::CliError;

/// The variable a supervised process is started with: it stops when its
/// standard input closes.
pub const SUPERVISED: &str = "SION_SUPERVISED";

/// How large a log grows before the next start keeps it aside.
pub const LOG_BYTES: u64 = 10 << 20;

/// The longest pause before starting again something that keeps stopping.
pub const BACKOFF_MOST: Duration = Duration::from_secs(60);

/// How long something must run for its pause to start over.
pub const STEADY: Duration = Duration::from_secs(60);

/// `sion agent up …`
#[derive(Debug, clap::Subcommand)]
pub enum UpCommand {
    /// Fill the list from what `sion setup` made: the chain, the
    /// scheduler, Telegram if it was set up, and the work runner of the
    /// profile.
    Init {
        /// The profile whose work runner runs (default: setup's).
        #[arg(long)]
        profile: Option<String>,
        /// Take GitHub's webhooks in the work runner (GITHUB_WEBHOOK_SECRET).
        #[arg(long)]
        webhooks: bool,
    },
    /// Add something to run: a name, and the arguments of `sion` after
    /// `--` (e.g. `sion agent up add slack -- agent bridge me --via slack
    /// --pairing`).
    Add {
        /// Its name: letters, digits, `-` and `_`.
        name: String,
        /// The arguments of `sion`.
        #[arg(last = true, required = true)]
        args: Vec<String>,
    },
    /// Take something off the list.
    Remove {
        /// Its name.
        name: String,
    },
    /// What the list holds, and how each one runs.
    List,
    /// Rewrite the paths a home copied from another machine holds
    /// (profiles' workspaces and skills, setup's record), from that
    /// machine's home to this one's.
    Rehome {
        /// The home directory on the machine the files came from.
        #[arg(long)]
        from: String,
    },
}

/// One thing the list runs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// Its name.
    pub name: String,
    /// The arguments of `sion`.
    pub args: Vec<String>,
}

/// The list.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct List {
    /// What it runs, in order.
    pub entries: Vec<Entry>,
}

fn path() -> PathBuf {
    crate::setup::home().join("up.json")
}

fn status_path() -> PathBuf {
    crate::setup::home().join("up.status.json")
}

fn logs() -> PathBuf {
    crate::setup::home().join("logs")
}

fn usage(e: impl std::fmt::Display) -> CliError {
    CliError::Usage(e.to_string())
}

/// The list as kept.
pub fn load() -> Result<List, CliError> {
    match std::fs::read_to_string(path()) {
        Ok(text) => {
            serde_json::from_str(&text).map_err(|e| usage(format!("{}: {e}", path().display())))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(List::default()),
        Err(e) => Err(usage(e)),
    }
}

fn save(list: &List) -> Result<(), CliError> {
    let file = path();
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(usage)?;
    }
    let partial = file.with_extension("json.partial");
    std::fs::write(
        &partial,
        serde_json::to_string_pretty(list).unwrap_or_default(),
    )
    .and_then(|()| std::fs::rename(&partial, &file))
    .map_err(usage)
}

fn check_name(name: &str) -> Result<(), CliError> {
    if name.is_empty()
        || name.len() > 32
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(usage(format!(
            "{name:?}: a name of letters, digits, - and _"
        )));
    }
    Ok(())
}

/// Adds (or replaces) `name`.
pub fn add(name: &str, args: &[String]) -> Result<(), CliError> {
    check_name(name)?;
    if matches!(args.first().map(String::as_str), Some("agent"))
        && args.get(1).map(String::as_str) == Some("up")
    {
        return Err(usage("sion agent up does not run itself"));
    }
    let mut list = load()?;
    list.entries.retain(|e| e.name != name);
    list.entries.push(Entry {
        name: name.to_string(),
        args: args.to_vec(),
    });
    save(&list)
}

/// What `sion setup` made, as entries: the chain, the scheduler, Telegram,
/// and `profile`'s work runner (with webhooks if asked).
pub fn from_setup(profile: Option<&str>, webhooks: bool) -> Result<Vec<Entry>, CliError> {
    let made = crate::setup::made();
    let mut entries = Vec::new();
    if let Some(address) = &made.chain {
        entries.push(Entry {
            name: "chain".into(),
            args: crate::setup::chain_args(
                address,
                &crate::setup::home().join("chain"),
                crate::setup::IDLE_ROUND_MS,
            ),
        });
    }
    let profile = profile.map(str::to_string).or(made.profile.clone());
    entries.push(Entry {
        name: "scheduler".into(),
        args: vec!["agent".into(), "scheduler".into()],
    });
    if let Some(profile) = &profile {
        if made.telegram {
            entries.push(Entry {
                name: "telegram".into(),
                args: vec![
                    "agent".into(),
                    "telegram".into(),
                    profile.clone(),
                    "--pairing".into(),
                ],
            });
        }
        let mut work = vec!["agent".into(), "work".into(), "run".into(), profile.clone()];
        if webhooks {
            work.push("--listen".into());
        }
        entries.push(Entry {
            name: "work".into(),
            args: work,
        });
    }
    if entries.is_empty() {
        return Err(usage(
            "setup made nothing to run: run sion setup, or sion agent up add",
        ));
    }
    Ok(entries)
}

/// How each entry runs, as `sion agent up` last wrote it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Running {
    /// Its process, while it runs.
    pub pid: Option<u32>,
    /// When it last started (seconds since the epoch).
    pub since: u64,
    /// How often it was started again.
    pub restarts: u32,
    /// How it last stopped, if it did.
    pub last_stop: Option<String>,
}

/// One supervised process.
struct Kept {
    entry: Entry,
    child: Option<Child>,
    /// The end of its pipe that this process holds.
    pipe: Option<ChildStdin>,
    started: Instant,
    pause: Duration,
    again_at: Option<Instant>,
    running: Running,
}

/// Sets a log past [`LOG_BYTES`] aside as `<name>.log.1`, and starts it
/// over: copied, then emptied, so that what writes to it (appending) goes
/// on writing to it.
fn rotate_log(name: &str) {
    let file = logs().join(format!("{name}.log"));
    if std::fs::metadata(&file)
        .map(|m| m.len() > LOG_BYTES)
        .unwrap_or(false)
        && std::fs::copy(&file, logs().join(format!("{name}.log.1"))).is_ok()
    {
        if let Ok(log) = std::fs::OpenOptions::new().write(true).open(&file) {
            let _ = log.set_len(0);
        }
    }
}

fn log_for(name: &str) -> Result<std::fs::File, CliError> {
    let dir = logs();
    std::fs::create_dir_all(&dir).map_err(usage)?;
    let file = dir.join(format!("{name}.log"));
    if std::fs::metadata(&file)
        .map(|m| m.len() > LOG_BYTES)
        .unwrap_or(false)
    {
        let _ = std::fs::rename(&file, dir.join(format!("{name}.log.1")));
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&file)
        .map_err(usage)
}

impl Kept {
    fn start(&mut self, program: &std::path::Path) -> Result<(), CliError> {
        let log = log_for(&self.entry.name)?;
        let err = log.try_clone().map_err(usage)?;
        let mut child = Command::new(program)
            .args(&self.entry.args)
            .env(SUPERVISED, "1")
            .stdin(Stdio::piped())
            .stdout(log)
            .stderr(err)
            .spawn()
            .map_err(|e| usage(format!("{}: {e}", self.entry.name)))?;
        self.pipe = child.stdin.take();
        self.running.pid = Some(child.id());
        self.running.since = crate::work::now();
        self.child = Some(child);
        self.started = Instant::now();
        self.again_at = None;
        Ok(())
    }
}

fn write_status(kept: &[Kept]) {
    let status: BTreeMap<&str, &Running> = kept
        .iter()
        .map(|k| (k.entry.name.as_str(), &k.running))
        .collect();
    let all = json!({"pid": std::process::id(), "at": crate::work::now(), "entries": status});
    let _ = std::fs::write(
        status_path(),
        serde_json::to_string_pretty(&all).unwrap_or_default(),
    );
}

/// Runs the list until stopped, or for `seconds` if given (and then stops
/// what it runs).
pub fn run(seconds: Option<u64>) -> Result<(), CliError> {
    let list = load()?;
    if list.entries.is_empty() {
        return Err(usage(
            "nothing to run: sion agent up init, or sion agent up add <name> -- <arguments>",
        ));
    }
    let program = std::env::current_exe().map_err(usage)?;
    let mut kept: Vec<Kept> = list
        .entries
        .into_iter()
        .map(|entry| Kept {
            entry,
            child: None,
            pipe: None,
            started: Instant::now(),
            pause: Duration::from_secs(1),
            again_at: None,
            running: Running::default(),
        })
        .collect();
    for k in &mut kept {
        k.start(&program)?;
        eprintln!(
            "{}: started (sion {})",
            k.entry.name,
            k.entry.args.join(" ")
        );
    }
    write_status(&kept);
    let until = seconds.map(|s| Instant::now() + Duration::from_secs(s));
    let mut rotated = Instant::now();
    loop {
        if until.is_some_and(|u| Instant::now() >= u) {
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
        // Logs are kept within bounds while what writes them runs.
        if rotated.elapsed() >= Duration::from_secs(30) {
            rotated = Instant::now();
            for k in &kept {
                rotate_log(&k.entry.name);
            }
        }
        let mut changed = false;
        for k in &mut kept {
            if let Some(child) = &mut k.child {
                if let Ok(Some(status)) = child.try_wait() {
                    let ran = k.started.elapsed();
                    k.pause = if ran >= STEADY {
                        Duration::from_secs(1)
                    } else {
                        (k.pause * 2).min(BACKOFF_MOST)
                    };
                    k.child = None;
                    k.pipe = None;
                    k.running.pid = None;
                    k.running.last_stop = Some(status.to_string());
                    k.again_at = Some(Instant::now() + k.pause);
                    eprintln!(
                        "{}: stopped ({status}) after {} s; starting again in {} s",
                        k.entry.name,
                        ran.as_secs(),
                        k.pause.as_secs()
                    );
                    changed = true;
                }
            }
            if k.again_at.is_some_and(|at| Instant::now() >= at) {
                k.running.restarts += 1;
                if let Err(e) = k.start(&program) {
                    eprintln!("{}: {e}", k.entry.name);
                    k.again_at = Some(Instant::now() + BACKOFF_MOST);
                }
                changed = true;
            }
        }
        if changed {
            write_status(&kept);
        }
    }
    // Stopping: the pipes close, and each stops by itself; what does not
    // within a few seconds is killed.
    for k in &mut kept {
        k.pipe = None;
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    for k in &mut kept {
        if let Some(child) = &mut k.child {
            while Instant::now() < deadline {
                if matches!(child.try_wait(), Ok(Some(_))) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            if matches!(child.try_wait(), Ok(None)) {
                let _ = child.kill();
            }
            let _ = child.wait();
            k.running.pid = None;
        }
    }
    write_status(&kept);
    Ok(())
}

/// In a supervised process: stops it when its standard input closes,
/// which is when `sion agent up` stops.
pub fn watch_supervisor() {
    if std::env::var_os(SUPERVISED).is_none() {
        return;
    }
    std::thread::spawn(|| {
        let mut buffer = [0u8; 64];
        let mut input = std::io::stdin();
        loop {
            match input.read(&mut buffer) {
                Ok(0) | Err(_) => std::process::exit(0),
                Ok(_) => {}
            }
        }
    });
}

/// `text` (JSON) with the strings that are the home `from`, or paths in
/// it, made `to`'s; `None` if none is. `/home/ana` is not the start of
/// `/home/anabel`.
fn rewrite_home(text: &str, from: &str, to: &str) -> Option<String> {
    let old = regex::Regex::new(&format!("\"{}(/|\")", regex::escape(from))).ok()?;
    if !old.is_match(text) {
        return None;
    }
    Some(
        old.replace_all(text, |found: &regex::Captures<'_>| {
            format!("\"{to}{}", &found[1])
        })
        .into_owned(),
    )
}

/// Rewrites `from` (a home directory) to this machine's home in the files
/// of `$SION_HOME` that hold paths.
pub fn rehome(from: &str) -> Result<Vec<String>, CliError> {
    let to = std::env::var("HOME").map_err(|_| usage("HOME is not set"))?;
    let from = from.trim_end_matches('/');
    if from.is_empty() || from == to {
        return Ok(vec!["nothing to rewrite".into()]);
    }
    let home = crate::setup::home();
    let mut lines = Vec::new();
    let mut stack = vec![home.clone()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                // The chain's state holds no paths of the person's.
                if entry.file_name() != "chain" {
                    stack.push(path);
                }
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Some(rewritten) = rewrite_home(&text, from, &to) else {
                continue;
            };
            std::fs::write(&path, rewritten).map_err(usage)?;
            lines.push(format!(
                "rewrote {}",
                path.strip_prefix(&home).unwrap_or(&path).display()
            ));
        }
    }
    if lines.is_empty() {
        lines.push("nothing held the old home".into());
    }
    Ok(lines)
}

/// Runs an up command (`None`: run the list).
pub fn command(command: Option<UpCommand>, seconds: Option<u64>) -> Result<(), CliError> {
    match command {
        None => run(seconds),
        Some(UpCommand::Init { profile, webhooks }) => {
            let entries = from_setup(profile.as_deref(), webhooks)?;
            let mut list = load()?;
            for entry in entries {
                println!("{}: sion {}", entry.name, entry.args.join(" "));
                list.entries.retain(|e| e.name != entry.name);
                list.entries.push(entry);
            }
            save(&list)?;
            println!("run it all with: sion agent up (as a service: sion agent service install up --start -- agent up)");
            Ok(())
        }
        Some(UpCommand::Add { name, args }) => {
            add(&name, &args)?;
            println!("{name}: sion {}", args.join(" "));
            Ok(())
        }
        Some(UpCommand::Remove { name }) => {
            let mut list = load()?;
            let before = list.entries.len();
            list.entries.retain(|e| e.name != name);
            if list.entries.len() == before {
                return Err(usage(format!("{name} is not on the list")));
            }
            save(&list)?;
            println!("removed {name}");
            Ok(())
        }
        Some(UpCommand::List) => {
            let list = load()?;
            let status: serde_json::Value = std::fs::read_to_string(status_path())
                .ok()
                .and_then(|t| serde_json::from_str(&t).ok())
                .unwrap_or_default();
            if list.entries.is_empty() {
                println!("nothing on the list: sion agent up init");
            }
            for entry in &list.entries {
                let running = &status["entries"][&entry.name];
                let state = match running["pid"].as_u64() {
                    Some(pid) => format!(
                        "running (process {pid}, since {}, started again {} times)",
                        crate::runtime::utc(running["since"].as_u64().unwrap_or(0)),
                        running["restarts"].as_u64().unwrap_or(0)
                    ),
                    None if running.is_object() => format!(
                        "not running{}",
                        running["last_stop"]
                            .as_str()
                            .map(|s| format!(" ({s})"))
                            .unwrap_or_default()
                    ),
                    None => "not started".into(),
                };
                println!("{}: sion {} — {state}", entry.name, entry.args.join(" "));
            }
            Ok(())
        }
        Some(UpCommand::Rehome { from }) => {
            for line in rehome(&from)? {
                println!("{line}");
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_home_is_rewritten_as_a_whole_path() {
        let text = r#"{"a": "/home/ana", "b": "/home/ana/work", "c": "/home/anabel/x"}"#;
        assert_eq!(
            rewrite_home(text, "/home/ana", "/srv/ana").unwrap(),
            r#"{"a": "/srv/ana", "b": "/srv/ana/work", "c": "/home/anabel/x"}"#
        );
        assert_eq!(
            rewrite_home(r#"{"c": "/home/anabel"}"#, "/home/ana", "/srv/ana"),
            None
        );
    }
}
