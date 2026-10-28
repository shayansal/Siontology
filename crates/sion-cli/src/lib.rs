//! The `sion` command-line tool, as a library.
//!
//! - [`agents`]: agent identities, subagents, cards, suspension.
//! - [`bench`]: measured costs of primitives, execution, proving and a devnet.
//! - [`describe`]: transactions and receipts as sentences.
//! - [`ledger`]: talking to a node over HTTP or to an in-process devnet.
//! - [`session`]: wallets, building, submitting and waiting for transactions.
//! - [`demos`]: scripted end-to-end demonstrations.
//! - [`verify`]: light-client verification of a Pulse.
//! - [`mcp`]: a Model Context Protocol server for AI agents.
//! - [`stream`]: payment streams: the payer's chains and the payee's desk.
//! - [`tasks`]: Task Charters: escrowed work with acceptance fixed in advance.
//! - [`x402`]: HTTP 402 payments settled on Siontology, for servers and agents.

pub mod agents;
pub mod bench;
pub mod demos;
pub mod describe;
pub mod error;
pub mod ledger;
pub mod mcp;
pub mod session;
pub mod stake;
pub mod stream;
pub mod tasks;
pub mod verify;
pub mod x402;

pub use error::CliError;
pub use ledger::{HttpLedger, Ledger, LedgerExt, TxState};
pub use session::Session;
