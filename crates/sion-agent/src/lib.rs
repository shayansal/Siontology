//! An agent runtime for Siontology: the loop that lets a language model do
//! work with tools, under rules a person sets and limits the ledger
//! enforces.
//!
//! The runtime is ports and adapters. It defines what it needs, a
//! [`Model`] and [`Tool`]s, and knows nothing of nodes or wallets: the
//! `sion` command plugs the ledger in as tools (balances, payments under an
//! Agent Charter, memory), and anything else can plug in its own.
//!
//! What a session guarantees:
//!
//! - **Effects and autonomy.** Every tool declares the most it can do
//!   ([`Effect`]: read, write, external, spend). The session's
//!   [`Autonomy`] level decides what runs, what waits for a person's
//!   approval (which expires) and what is refused; grants narrow the tools
//!   further. Money is bounded twice: here, and by the agent's charter on
//!   the ledger, which no runtime setting can loosen.
//! - **Taint.** Content from outside (a web page, another agent) taints the
//!   session for a day: until then, external calls and spending need
//!   approval, so an injected instruction cannot act on its own.
//! - **No accidental repeats.** Every call that adds to the world (a
//!   payment, a message) is in a [`journal`](journal::Journal) under an
//!   idempotency key. Repeating an identical call returns the recorded
//!   outcome, saying so; a call whose outcome is unknown (a timeout after a
//!   payment was sent) is never repeated until someone settles it. Calls
//!   meant to run again when repeated (a shell command, writing a file
//!   back) are not journaled.
//! - **Sandboxed commands.** The shell and skills' scripts run in a
//!   [`sandbox`] (bubblewrap or a Docker image pinned by digest) that can
//!   write only the workspace and reaches no network unless the operator
//!   allows it.
//! - **Tools from MCP servers** ([`mcp`]), offered only while each tool's
//!   definition matches the one an operator reviewed.
//! - **Bounded runs.** A budget of turns that grows with progress up to a
//!   cap; an identical call that keeps failing is blocked; long
//!   conversations are compacted; a model that fails is cooled down and the
//!   next one is asked.
//! - **Pause and resume.** A session's [`State`] serialises: a run that
//!   needs approval stops, and resumes where it stopped once a person
//!   decides. A kill switch stops a run before its next call.
//! - **Traces.** A session exports as a trajectory, for review, evaluation
//!   and training ([`trace`]).
//! - **Watched runs.** A run can be watched as it goes ([`Seen`]): a model
//!   is asked, the text it writes as it writes it (providers stream it over
//!   Server-Sent Events), the tools it calls and runs, a call that waits.
//!   That is what shows a person that the agent is typing, and its reply as
//!   it is written.
//!
//! Skills ([`skills`]) are how an agent gets better at its work: it writes
//! down what worked, and a skill is measured against an evaluation set
//! ([`evals`]) and improved by evolving variants against it ([`evolve`]).
//!
//! An agent's past is searchable ([`search`]: BM25 over its sessions'
//! messages), and what it did can be consolidated into notes worth keeping
//! ([`consolidate`]: a model proposes, gates decide).

pub mod browser;
pub mod builtin;
pub mod cdp;
pub mod consolidate;
pub mod desktop;
pub mod dream;
pub mod egress;
pub mod encoding;
pub mod evals;
pub mod evolve;
pub mod graph;
pub mod journal;
pub mod mcp;
pub mod mcp_http;
pub mod model;
pub mod oauth;
pub mod permissions;
pub mod policy;
pub mod providers;
pub mod quarantine;
pub mod sandbox;
pub mod schedule;
pub mod scripted;
pub mod search;
pub mod session;
pub mod skills;
pub mod tools;
pub mod trace;
pub mod verify;

pub use model::{
    Block, Delta, Message, Model, ModelError, Picture, Request, Response, Role, Stop, ToolSpec,
    Usage,
};
pub use policy::{Autonomy, Decision, Policy, Standing};
pub use session::{Ended, Limits, Runtime, Seen, State};
pub use tools::{Dedup, Effect, Outcome, Tool, Toolbox};
