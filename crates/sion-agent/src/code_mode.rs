//! Calling tools from code. The model may write a short Python program
//! (`run_code`) that calls the session's tools that only read, as
//! functions: `tools.read_file(path="a.txt")`, or `call("read_file",
//! path="a.txt")`. The program runs in the sandbox with nothing of the
//! machine in view (no workspace, no network): what it reads, it reads
//! through the tools. Each call it makes comes back to the runtime over a
//! pipe and is checked and made as a call of the model's own would be (its
//! policy, its journal, its taint, quarantine), and what it prints is the
//! result. A loop over fifty files is one turn, and only what the program
//! prints enters the conversation.
//!
//! The pipe is the program's standard input and output: a call is a line
//! that begins with a token the runtime made for the run, the JSON of the
//! tool and its input; the answer comes back as a line of JSON. Every other
//! line it prints is its output. A tool that does more than read, or would
//! wait for a person, is not called from code: the program is told so, and
//! the model makes that call itself.

use std::io::{BufRead, BufReader, Write};
use std::process::Child;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::model::ToolSpec;
use crate::sandbox::{Network, Sandbox};

/// The tool that runs a program which calls tools.
pub const RUN_CODE: &str = "run_code";

/// The most calls one program may make.
pub const MAX_CODE_CALLS: usize = 200;

/// The most characters of a program's output kept as its result.
pub const MAX_CODE_OUTPUT: usize = 24_000;

/// The most bytes of a program.
pub const MAX_PROGRAM: usize = 64 << 10;

/// The most tool names the description of `run_code` lists.
const LISTED: usize = 40;

/// The spec of [`RUN_CODE`], naming the tools a program may call
/// (`callable`, by name).
pub fn spec(callable: &[String]) -> ToolSpec {
    let mut names: Vec<&str> = callable.iter().map(String::as_str).take(LISTED).collect();
    if callable.len() > LISTED {
        names.push("…");
    }
    ToolSpec {
        name: RUN_CODE.into(),
        description: format!(
            "Run a Python 3 program that calls tools that only read, as functions: \
             tools.<name>(<input as keyword arguments>) or call(\"<name>\", ...), each returning \
             the tool's text and raising ToolError if the tool fails. Use it to call tools many \
             times, or to filter and combine what they return, in one step: only what the program \
             prints comes back. The program sees nothing but the tools (no files, no network); \
             at most {MAX_CODE_CALLS} calls, {MAX_CODE_OUTPUT} characters printed. A tool that \
             changes something or reaches outside is not called from code: call it yourself. \
             Tools it may call: {}.",
            names.join(", ")
        ),
        input_schema: json!({
            "type": "object",
            "properties": {"code": {"type": "string", "description": "The Python program."}},
            "required": ["code"]
        }),
    }
}

/// What a running program said last.
#[derive(Clone, Debug, PartialEq)]
pub enum Said {
    /// It calls `tool` with `input`, and waits for the answer.
    Call {
        /// The tool.
        tool: String,
        /// Its input.
        input: Value,
    },
    /// It ended: what it printed, and the error it ended on, if any (a
    /// traceback).
    Done {
        /// What it printed.
        output: String,
        /// How it failed, if it did.
        error: Option<String>,
    },
    /// It could not be run, or stopped answering in time.
    Failed(String),
}

/// A program running, which calls tools.
pub trait Running: Send {
    /// What it says next, waiting at most until `deadline`.
    fn next(&mut self, deadline: Instant) -> Said;

    /// Answers its last call: the tool's text, and whether the tool
    /// failed.
    fn answer(&mut self, content: &str, is_error: bool) -> Result<(), String>;

    /// Stops it.
    fn stop(&mut self);
}

/// What runs programs that call tools.
pub trait CodeRunner: Sync {
    /// Starts `program`.
    fn start(&self, program: &str) -> Result<Box<dyn Running>, String>;

    /// How long a program may run, its calls included.
    fn time_limit(&self) -> Duration;
}

/// What the program runs inside: a Python prelude that reads the program
/// from its first line of input, gives it `tools`, `call` and `ToolError`,
/// and says on the pipe how it ended. `token` begins every line meant for
/// the runtime.
pub fn prelude(token: &str) -> String {
    format!(
        r#"import json as _json, sys as _sys, traceback as _traceback
_TOKEN = {token:?}
_out, _in = _sys.stdout, _sys.stdin

class ToolError(Exception):
    pass

def _say(what):
    _out.write(_TOKEN + _json.dumps(what) + "\n")
    _out.flush()

def call(tool, **input):
    _say({{"tool": tool, "input": input}})
    line = _in.readline()
    if not line:
        raise SystemExit(3)
    answer = _json.loads(line)
    if answer.get("error"):
        raise ToolError(answer.get("content", ""))
    return answer.get("content", "")

class _Tools:
    def __getattr__(self, name):
        if name.startswith("__"):
            raise AttributeError(name)
        return lambda **input: call(name, **input)

tools = _Tools()
_program = _json.loads(_in.readline())
_space = {{"__name__": "__main__", "tools": tools, "call": call, "ToolError": ToolError}}
try:
    exec(compile(_program, "<run_code>", "exec"), _space)
except SystemExit as _e:
    if _e.code not in (None, 0):
        _say({{"error": "the program exited with " + str(_e.code)}})
except BaseException:
    _say({{"error": _traceback.format_exc(limit=8)}})
_out.flush()
_say({{"done": True}})
"#
    )
}

/// A program running as a process: its standard input and output are the
/// pipe ([`prelude`]).
pub struct Process {
    child: Child,
    stop: Box<dyn FnMut(&mut Child) + Send>,
    lines: mpsc::Receiver<String>,
    token: String,
    output: String,
    error: Option<String>,
}

impl std::fmt::Debug for Process {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Process({})", self.child.id())
    }
}

impl Process {
    /// Takes `child`, started on [`prelude`] with `token`, gives it
    /// `program`, and reads what it says; `stop` stops it.
    pub fn start(
        mut child: Child,
        token: &str,
        program: &str,
        stop: Box<dyn FnMut(&mut Child) + Send>,
    ) -> Result<Process, String> {
        let stdout = child.stdout.take().ok_or("the program has no output")?;
        let (sender, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        let mut process = Process {
            child,
            stop,
            lines,
            token: token.to_string(),
            output: String::new(),
            error: None,
        };
        let first = serde_json::to_string(program).map_err(|e| e.to_string())?;
        process.write(&first)?;
        Ok(process)
    }

    fn write(&mut self, line: &str) -> Result<(), String> {
        let input = self
            .child
            .stdin
            .as_mut()
            .ok_or("the program takes no input")?;
        writeln!(input, "{line}")
            .and_then(|_| input.flush())
            .map_err(|e| format!("the program stopped listening: {e}"))
    }

    fn keep(&mut self, line: &str) {
        if self.output.chars().count() >= MAX_CODE_OUTPUT {
            return;
        }
        self.output.push_str(line);
        self.output.push('\n');
    }
}

impl Running for Process {
    fn next(&mut self, deadline: Instant) -> Said {
        loop {
            let wait = deadline.saturating_duration_since(Instant::now());
            let line = match self.lines.recv_timeout(wait) {
                Ok(line) => line,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    self.stop();
                    return Said::Failed("stopped: the program ran out of time".into());
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    let status = self.child.wait().ok().and_then(|s| s.code());
                    return match status {
                        // The prelude did not run: no Python, or no room.
                        Some(127) => Said::Failed("python3 is not in the sandbox".into()),
                        _ => Said::Done {
                            output: std::mem::take(&mut self.output),
                            error: self.error.take().or_else(|| {
                                Some(format!(
                                    "the program ended without saying so (status {})",
                                    status.map_or("unknown".to_string(), |s| s.to_string())
                                ))
                            }),
                        },
                    };
                }
            };
            let Some(said) = line.strip_prefix(&self.token) else {
                self.keep(&line);
                continue;
            };
            let Ok(said) = serde_json::from_str::<Value>(said) else {
                continue;
            };
            if let Some(tool) = said["tool"].as_str() {
                return Said::Call {
                    tool: tool.to_string(),
                    input: said["input"].clone(),
                };
            }
            if let Some(error) = said["error"].as_str() {
                self.error = Some(error.trim_end().to_string());
                continue;
            }
            if said["done"] == true {
                let _ = self.child.wait();
                return Said::Done {
                    output: std::mem::take(&mut self.output),
                    error: self.error.take(),
                };
            }
        }
    }

    fn answer(&mut self, content: &str, is_error: bool) -> Result<(), String> {
        let line = json!({"content": content, "error": is_error}).to_string();
        self.write(&line)
    }

    fn stop(&mut self) {
        (self.stop)(&mut self.child);
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            self.stop();
        }
    }
}

/// Runs programs in a sandbox, with no network and nothing of the machine
/// mounted: Python 3 (`python3`) from the sandbox's own system.
#[derive(Debug)]
pub struct SandboxRunner {
    /// The sandbox (its network is not used: programs get none).
    pub sandbox: Sandbox,
    /// The interpreter, `python3` by default.
    pub python: String,
}

impl SandboxRunner {
    /// A runner in `sandbox`.
    pub fn new(sandbox: &Sandbox) -> SandboxRunner {
        SandboxRunner {
            sandbox: Sandbox {
                network: Network::None,
                ..sandbox.clone()
            },
            python: "python3".into(),
        }
    }
}

/// A token no program can guess: 16 random bytes, in hex.
fn token() -> Result<String, String> {
    let mut bytes = [0u8; 16];
    sion_http::random(&mut bytes).map_err(|e| e.to_string())?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect::<String>() + ":")
}

impl CodeRunner for SandboxRunner {
    fn start(&self, program: &str) -> Result<Box<dyn Running>, String> {
        if program.len() > MAX_PROGRAM {
            return Err(format!("a program is at most {MAX_PROGRAM} bytes"));
        }
        let token = token()?;
        let argv = vec![
            self.python.clone(),
            "-u".into(),
            "-I".into(),
            "-c".into(),
            prelude(&token),
        ];
        let (child, name) = self.sandbox.spawn(&[], "/tmp", &argv, &[])?;
        let sandbox = self.sandbox.clone();
        let stop = Box::new(move |child: &mut Child| sandbox.stop(child, &name));
        Ok(Box::new(Process::start(child, &token, program, stop)?))
    }

    fn time_limit(&self) -> Duration {
        Duration::from_secs(self.sandbox.limits.timeout_secs.max(1))
    }
}
