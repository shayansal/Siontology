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
//! - **No accidental repeats.** Every call that changes something is in a
//!   [`journal`](journal::Journal) under an idempotency key. Repeating an
//!   identical call returns the recorded outcome; a call whose outcome is
//!   unknown (a timeout after a payment was sent) is never repeated until
//!   someone settles it.
//! - **Bounded runs.** A budget of turns that grows with progress up to a
//!   cap; an identical call that keeps failing is blocked; long
//!   conversations are compacted; a model that fails is cooled down and the
//!   next one is asked.
//! - **Pause and resume.** A session's [`State`] serialises: a run that
//!   needs approval stops, and resumes where it stopped once a person
//!   decides. A kill switch stops a run before its next call.
//! - **Traces.** A session exports as a trajectory, for review, evaluation
//!   and training ([`trace`]).
//!
//! Skills ([`skills`]) are how an agent gets better at its work: it writes
//! down what worked, and a skill is measured against an evaluation set
//! ([`evals`]) and improved by evolving variants against it ([`evolve`]).

pub mod builtin;
pub mod evals;
pub mod evolve;
pub mod journal;
pub mod model;
pub mod policy;
pub mod providers;
pub mod schedule;
pub mod scripted;
pub mod session;
pub mod skills;
pub mod tools;
pub mod trace;

pub use model::{
    Block, Message, Model, ModelError, Request, Response, Role, Stop, ToolSpec, Usage,
};
pub use policy::{Autonomy, Decision, Policy, Standing};
pub use session::{Ended, Limits, Runtime, State};
pub use tools::{Effect, Outcome, Tool, Toolbox};
