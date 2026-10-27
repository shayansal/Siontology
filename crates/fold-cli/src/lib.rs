//! The `fold` command-line tool, as a library.
//!
//! - [`bench`]: measured costs of primitives, execution, proving and a devnet.
//! - [`ledger`]: talking to a node over HTTP or to an in-process devnet.
//! - [`session`]: wallets, building, submitting and waiting for transactions.
//! - [`demos`]: scripted end-to-end demonstrations.
//! - [`verify`]: light-client verification of a Pulse.
//! - [`mcp`]: a Model Context Protocol server for AI agents.

pub mod bench;
pub mod demos;
pub mod error;
pub mod ledger;
pub mod mcp;
pub mod session;
pub mod verify;

pub use error::CliError;
pub use ledger::{HttpLedger, Ledger, LedgerExt, TxState};
pub use session::Session;
