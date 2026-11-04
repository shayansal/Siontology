//! The operator's hooks: commands of their own that run around their
//! agents' tool calls and runs, kept in `$SION_HOME/agents/hooks.json`
//! (`sion agent hooks`).
//!
//! Each hook is for one event, and, for the tool events, the tools whose
//! names its patterns match (`*` stands for any characters; none, for all):
//!
//! - `before-tool`: before a call runs (once the policy lets it, and any
//!   approval is given). Exit 0 lets it run; exit 2 blocks it, and what the
//!   hook wrote to its error output (or else its output) is the model's
//!   reason. A hook that fails otherwise (another status, no start, out of
//!   time) blocks the call too: a guard that breaks does not open.
//! - `after-tool`: after a call ran. What it prints, on exit 0, is added
//!   to what the model is shown of the call.
//! - `run-start`: before a run of a session (each message of a chat). What
//!   it prints, on exit 0, is added to the person's latest message, marked
//!   as the hook's.
//! - `run-end`: after a run, with how it ended.
//!
//! A hook reads the event as JSON on its standard input (`event`,
//! `session`, and `tool` and `input`, `outcome`, `said` or `ended` as the
//! event has them), runs as the operator, unsandboxed, in the session's
//! workspace if it has one, and is stopped after its timeout (30 seconds
//! by default). Failures of hooks that do not block are written to
//! `$SION_HOME/agents/hooks.log`. The file is refused if anyone but its
//! owner may change it.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::CliError;

/// When a hook runs.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, clap::ValueEnum,
)]
#[serde(rename_all = "kebab-case")]
pub enum Event {
    /// Before a tool call runs: it may block it.
    BeforeTool,
    /// After a tool call ran: it may add a note to its result.
    AfterTool,
    /// Before a run of a session: it may add a note to the person's message.
    RunStart,
    /// After a run of a session.
    RunEnd,
}

impl Event {
    fn name(self) -> &'static str {
        match self {
            Event::BeforeTool => "before-tool",
            Event::AfterTool => "after-tool",
            Event::RunStart => "run-start",
            Event::RunEnd => "run-end",
        }
    }
}

/// The most seconds a hook may be given.
pub const MAX_TIMEOUT: u64 = 600;

/// The most characters of a hook's output kept.
pub const MAX_SAID: usize = 4_000;

fn default_timeout() -> u64 {
    30
}

/// One hook.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hook {
    /// When it runs.
    pub event: Event,
    /// The tools it is for, by patterns of their names (`*` for any
    /// characters); none, every tool.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<String>,
    /// The command and its arguments.
    pub command: Vec<String>,
    /// Seconds before it is stopped.
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

impl Hook {
    /// Whether it is for `tool`.
    pub fn takes(&self, tool: &str) -> bool {
        self.tools.is_empty() || self.tools.iter().any(|pattern| matches(pattern, tool))
    }

    /// Checks that it can be kept.
    pub fn check(&self) -> Result<(), String> {
        if self.command.is_empty() || self.command[0].trim().is_empty() {
            return Err("a hook needs a command".into());
        }
        if self.timeout_secs == 0 || self.timeout_secs > MAX_TIMEOUT {
            return Err(format!("a hook's timeout is 1 to {MAX_TIMEOUT} seconds"));
        }
        if !self.tools.is_empty() && matches!(self.event, Event::RunStart | Event::RunEnd) {
            return Err("--tool goes with before-tool and after-tool hooks".into());
        }
        Ok(())
    }

    /// One line about it.
    pub fn describe(&self) -> String {
        let tools = if self.tools.is_empty() {
            String::new()
        } else {
            format!(" for {}", self.tools.join(", "))
        };
        format!(
            "{}{tools}: {} ({}s)",
            self.event.name(),
            self.command.join(" "),
            self.timeout_secs
        )
    }
}

/// Whether `pattern` matches `name`: `*` stands for any characters.
pub fn matches(pattern: &str, name: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == name;
    }
    let (first, last) = (parts[0], parts[parts.len() - 1]);
    if !name.starts_with(first) || name.len() < first.len() + last.len() || !name.ends_with(last) {
        return false;
    }
    let mut rest = &name[first.len()..name.len() - last.len()];
    for part in &parts[1..parts.len() - 1] {
        match rest.find(part) {
            Some(at) => rest = &rest[at + part.len()..],
            None => return false,
        }
    }
    true
}

/// The file hooks are kept in: `$SION_HOME/agents/hooks.json`.
pub fn path() -> PathBuf {
    crate::profiles::path().with_file_name("hooks.json")
}

fn log_path() -> PathBuf {
    crate::profiles::path().with_file_name("hooks.log")
}

/// The hooks kept; none if there is no file. A file anyone but its owner
/// may change is refused.
pub fn load() -> Result<Vec<Hook>, CliError> {
    let path = path();
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(CliError::Usage(format!("{}: {e}", path.display()))),
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path)
            .map_err(|e| CliError::Usage(format!("{}: {e}", path.display())))?
            .permissions()
            .mode();
        if mode & 0o022 != 0 {
            return Err(CliError::Usage(format!(
                "{} may be changed by others (mode {:o}): its hooks are not run; chmod go-w it",
                path.display(),
                mode & 0o777
            )));
        }
    }
    let hooks: Vec<Hook> = serde_json::from_str(&text)
        .map_err(|e| CliError::Usage(format!("{}: {e}", path.display())))?;
    for hook in &hooks {
        hook.check()
            .map_err(|why| CliError::Usage(format!("{}: {why}", path.display())))?;
    }
    Ok(hooks)
}

/// Keeps `hooks`, readable and writable by the operator alone.
pub fn save(hooks: &[Hook]) -> Result<(), CliError> {
    let text = serde_json::to_string_pretty(hooks).map_err(|e| CliError::Usage(e.to_string()))?;
    crate::secrets::write_private(&path(), text.as_bytes())
}

/// How a hook's run went.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Ran {
    /// Its exit status, if it exited.
    pub status: Option<i32>,
    /// What it printed.
    pub out: String,
    /// What it wrote to its error output.
    pub err: String,
    /// Why it did not run to its end, if it did not (it could not start,
    /// or ran out of time).
    pub failed: Option<String>,
}

impl Ran {
    /// What it said: its error output, or else its output, trimmed and cut.
    fn said(&self) -> String {
        let said = if self.err.trim().is_empty() {
            &self.out
        } else {
            &self.err
        };
        said.trim().chars().take(MAX_SAID).collect()
    }

    fn why_failed(&self) -> String {
        match (&self.failed, self.status) {
            (Some(why), _) => why.clone(),
            (None, Some(status)) => {
                let said = self.said();
                if said.is_empty() {
                    format!("it exited with {status}")
                } else {
                    format!("it exited with {status}: {said}")
                }
            }
            (None, None) => "it was stopped by a signal".into(),
        }
    }
}

/// The most bytes of a hook's output read.
const MAX_READ: u64 = 64 << 10;

/// Runs `hook` with `input` on its standard input, in `dir` if given.
pub fn run(hook: &Hook, input: &Value, dir: Option<&Path>) -> Ran {
    let mut command = Command::new(&hook.command[0]);
    command
        .args(&hook.command[1..])
        .env("SION_HOOK_EVENT", hook.event.name())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(session) = input["session"].as_str() {
        command.env("SION_SESSION", session);
    }
    if let Some(dir) = dir.filter(|dir| dir.is_dir()) {
        command.current_dir(dir);
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) => {
            return Ran {
                failed: Some(format!("{} could not start: {e}", hook.command[0])),
                ..Ran::default()
            }
        }
    };
    let read = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut text = Vec::new();
            if let Some(pipe) = pipe {
                let _ = pipe.take(MAX_READ).read_to_end(&mut text);
            }
            String::from_utf8_lossy(&text).into_owned()
        })
    };
    let out = read(
        child
            .stdout
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    let err = read(
        child
            .stderr
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    if let Some(mut stdin) = child.stdin.take() {
        // A hook that does not read its input is not held up by it.
        let _ = stdin.write_all(input.to_string().as_bytes());
    }
    let deadline = Instant::now() + Duration::from_secs(hook.timeout_secs);
    let (status, failed) = loop {
        match child.try_wait() {
            Ok(Some(status)) => break (status.code(), None),
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                break (
                    None,
                    Some(format!("it ran past its {} seconds", hook.timeout_secs)),
                );
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(e) => break (None, Some(e.to_string())),
        }
    };
    if failed.is_some() {
        // What it started may hold its output open: not waited for.
        return Ran {
            status,
            failed,
            ..Ran::default()
        };
    }
    Ran {
        status,
        out: out.join().unwrap_or_default(),
        err: err.join().unwrap_or_default(),
        failed,
    }
}

/// Notes the failure of a hook that does not block, in `hooks.log`.
fn note_failure(hook: &Hook, why: &str) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let line = format!(
        "{} {} {}: {why}\n",
        crate::runtime::utc(now),
        hook.event.name(),
        hook.command.join(" ")
    );
    let path = log_path();
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = file.write_all(line.as_bytes());
    }
}

/// The operator's hooks, as a session's runtime asks them.
#[derive(Clone, Debug, Default)]
pub struct OperatorHooks {
    /// The hooks.
    pub hooks: Vec<Hook>,
    /// Where they run: the session's workspace, if it has one.
    pub dir: Option<PathBuf>,
}

impl OperatorHooks {
    /// The hooks kept, to run in `dir`.
    pub fn load(dir: Option<&Path>) -> Result<OperatorHooks, CliError> {
        Ok(OperatorHooks {
            hooks: load()?,
            dir: dir.map(Path::to_path_buf),
        })
    }

    /// Whether there are any for tool calls.
    pub fn for_tools(&self) -> bool {
        self.hooks
            .iter()
            .any(|h| matches!(h.event, Event::BeforeTool | Event::AfterTool))
    }

    fn of(&self, event: Event) -> impl Iterator<Item = &Hook> {
        self.hooks.iter().filter(move |h| h.event == event)
    }

    /// Before a run of `session`, whose person said `said` last: what the
    /// hooks add, if anything.
    pub fn run_start(&self, session: &str, said: &str) -> Option<String> {
        let input = json!({"event": "run-start", "session": session, "said": said});
        let notes: Vec<String> = self
            .of(Event::RunStart)
            .filter_map(|hook| {
                let ran = run(hook, &input, self.dir.as_deref());
                if ran.status == Some(0) {
                    let note: String = ran.out.trim().chars().take(MAX_SAID).collect();
                    (!note.is_empty()).then_some(note)
                } else {
                    note_failure(hook, &ran.why_failed());
                    None
                }
            })
            .collect();
        (!notes.is_empty()).then(|| notes.join("\n"))
    }

    /// After a run of `session` ended as `ended`.
    pub fn run_end(&self, session: &str, ended: &sion_agent::Ended) {
        let input = json!({"event": "run-end", "session": session, "ended": ended});
        for hook in self.of(Event::RunEnd) {
            let ran = run(hook, &input, self.dir.as_deref());
            if ran.status != Some(0) {
                note_failure(hook, &ran.why_failed());
            }
        }
    }
}

impl sion_agent::hooks::ToolHooks for OperatorHooks {
    fn before(&self, session: &str, tool: &str, input: &Value) -> Option<String> {
        let event =
            json!({"event": "before-tool", "session": session, "tool": tool, "input": input});
        for hook in self.of(Event::BeforeTool).filter(|h| h.takes(tool)) {
            let ran = run(hook, &event, self.dir.as_deref());
            match (ran.status, &ran.failed) {
                (Some(0), None) => continue,
                (Some(2), None) => {
                    let said = ran.said();
                    return Some(if said.is_empty() {
                        "it gave no reason".into()
                    } else {
                        said
                    });
                }
                _ => {
                    return Some(format!(
                        "the hook {} failed, and a failed hook blocks: {}",
                        hook.command[0],
                        ran.why_failed()
                    ))
                }
            }
        }
        None
    }

    fn after(
        &self,
        session: &str,
        tool: &str,
        input: &Value,
        outcome: &sion_agent::Outcome,
    ) -> Option<String> {
        let event = json!({"event": "after-tool", "session": session, "tool": tool,
            "input": input, "outcome": {"content": outcome.content,
                "is_error": outcome.is_error, "from_outside": outcome.untrusted}});
        let notes: Vec<String> = self
            .of(Event::AfterTool)
            .filter(|h| h.takes(tool))
            .filter_map(|hook| {
                let ran = run(hook, &event, self.dir.as_deref());
                if ran.status == Some(0) {
                    let note: String = ran.out.trim().chars().take(MAX_SAID).collect();
                    (!note.is_empty()).then_some(note)
                } else {
                    note_failure(hook, &ran.why_failed());
                    None
                }
            })
            .collect();
        (!notes.is_empty()).then(|| notes.join("\n"))
    }
}

/// `sion agent hooks`.
#[derive(Debug, clap::Subcommand)]
pub enum HooksCommand {
    /// Add a hook: the command after `--` runs at the event, reading it as
    /// JSON on its standard input.
    Add {
        /// When it runs.
        #[arg(long, value_enum)]
        event: Event,
        /// For tool events, a tool it is for, by a pattern of names (`*`
        /// for any characters). Repeat for more; none, every tool.
        #[arg(long = "tool", value_name = "PATTERN")]
        tools: Vec<String>,
        /// Seconds before it is stopped.
        #[arg(long, default_value_t = 30)]
        timeout: u64,
        /// The command and its arguments.
        #[arg(last = true, required = true)]
        command: Vec<String>,
    },
    /// List the hooks, numbered.
    List,
    /// Remove a hook, by its number in the list.
    Remove {
        /// Its number.
        number: usize,
    },
}

/// Runs `sion agent hooks`.
pub fn command(command: HooksCommand) -> Result<(), CliError> {
    match command {
        HooksCommand::Add {
            event,
            tools,
            timeout,
            command,
        } => {
            let hook = Hook {
                event,
                tools,
                command,
                timeout_secs: timeout,
            };
            hook.check().map_err(CliError::Usage)?;
            let mut hooks = load()?;
            println!("hook {}: {}", hooks.len() + 1, hook.describe());
            hooks.push(hook);
            save(&hooks)
        }
        HooksCommand::List => {
            let hooks = load()?;
            if hooks.is_empty() {
                println!("no hooks ({})", path().display());
            }
            for (at, hook) in hooks.iter().enumerate() {
                println!("{}. {}", at + 1, hook.describe());
            }
            Ok(())
        }
        HooksCommand::Remove { number } => {
            let mut hooks = load()?;
            if number == 0 || number > hooks.len() {
                return Err(CliError::Usage(format!(
                    "there is no hook {number}: sion agent hooks list numbers them"
                )));
            }
            let gone = hooks.remove(number - 1);
            save(&hooks)?;
            println!("removed {}", gone.describe());
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns_match_tool_names() {
        assert!(matches("shell", "shell") && !matches("shell", "shells"));
        assert!(matches("write_*", "write_file") && !matches("write_*", "read_file"));
        assert!(matches("*_file", "read_file") && matches("*", "anything"));
        assert!(matches("github_*_issue*", "github_create_issue_comment"));
        assert!(!matches("a*b*c", "acb"));
        let hook = Hook {
            event: Event::BeforeTool,
            tools: vec!["shell".into(), "mcp_*".into()],
            command: vec!["true".into()],
            timeout_secs: 30,
        };
        assert!(hook.takes("mcp_github_list") && !hook.takes("read_file"));
        assert!(hook.check().is_ok());
        let wrong = Hook {
            event: Event::RunEnd,
            ..hook.clone()
        };
        assert!(wrong.check().is_err(), "a run's hook is for no tool");
    }

    #[test]
    fn a_hook_blocks_with_its_reason_and_one_that_fails_blocks_too() {
        use sion_agent::hooks::ToolHooks;
        let script =
            |text: &str| -> Vec<String> { vec!["/bin/sh".into(), "-c".into(), text.into()] };
        let hooks = OperatorHooks {
            hooks: vec![
                Hook {
                    event: Event::BeforeTool,
                    tools: vec!["shell".into()],
                    // Reads the call, and refuses rm.
                    command: script(
                        "if grep -q '\"rm '; then echo 'no rm here' >&2; exit 2; fi; exit 0",
                    ),
                    timeout_secs: 10,
                },
                Hook {
                    event: Event::AfterTool,
                    tools: Vec::new(),
                    command: script("cat >/dev/null; echo \"checked by $SION_HOOK_EVENT\""),
                    timeout_secs: 10,
                },
            ],
            dir: None,
        };
        assert_eq!(
            hooks.before("s1", "shell", &json!({"command": "rm -rf build"})),
            Some("no rm here".into())
        );
        assert_eq!(hooks.before("s1", "shell", &json!({"command": "ls"})), None);
        assert_eq!(
            hooks.before("s1", "read_file", &json!({})),
            None,
            "not for it"
        );
        assert_eq!(
            hooks.after(
                "s1",
                "read_file",
                &json!({}),
                &sion_agent::Outcome::ok("text")
            ),
            Some("checked by after-tool".into())
        );
        let broken = OperatorHooks {
            hooks: vec![
                Hook {
                    event: Event::BeforeTool,
                    tools: Vec::new(),
                    command: script("exit 1"),
                    timeout_secs: 10,
                },
                Hook {
                    event: Event::BeforeTool,
                    tools: Vec::new(),
                    command: vec!["/no/such/hook".into()],
                    timeout_secs: 10,
                },
            ],
            dir: None,
        };
        let why = broken.before("s1", "shell", &json!({})).unwrap();
        assert!(
            why.contains("failed, and a failed hook blocks: it exited with 1"),
            "{why}"
        );
        let slow = OperatorHooks {
            hooks: vec![Hook {
                event: Event::BeforeTool,
                tools: Vec::new(),
                command: script("sleep 5"),
                timeout_secs: 1,
            }],
            dir: None,
        };
        let why = slow.before("s1", "shell", &json!({})).unwrap();
        assert!(why.contains("ran past its 1 seconds"), "{why}");
    }
}
