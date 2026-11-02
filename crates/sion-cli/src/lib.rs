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
//! - [`network`]: keys, genesis, nodes and gateways of a network of
//!   processes.
//! - [`stream`]: payment streams: the payer's chains and the payee's desk.
//! - [`tasks`]: Task Charters: escrowed work with acceptance fixed in advance.
//! - [`x402`]: HTTP 402 payments settled on Siontology, for servers and agents.

// The MCP server's tool list is one json! literal, deeper than the default
// macro recursion limit allows.
#![recursion_limit = "256"]

pub mod a2a;
pub mod agent_mcp;
pub mod agent_memory;
pub mod agent_storage;
pub mod agents;
pub mod archive;
pub mod batcher;
pub mod bench;
pub mod bridge;
pub mod channels;
pub mod chatgpt;
pub mod cohorts;
pub mod consolidate;
pub mod credentials;
pub mod delegation;
pub mod demos;
pub mod describe;
pub mod dingtalk;
pub mod doctor;
pub mod drafts;
pub mod email;
pub mod error;
pub mod feishu;
pub mod files;
pub mod gateway;
pub mod github;
pub mod google_chat;
pub mod graphs;
pub mod home_assistant;
pub mod imessage;
pub mod irc;
pub mod jwt;
pub mod ledger;
pub mod live;
pub mod logins;
pub mod mattermost;
pub mod mcp;
pub mod mcp_auth;
pub mod memory;
pub mod messengers;
pub mod network;
pub mod nextcloud;
pub mod nostr;
pub mod ntfy;
pub mod openai_api;
pub mod pairing;
pub mod phone;
pub mod profiles;
pub mod qq;
pub mod recall;
pub mod runtime;
pub mod schedules;
pub mod screens;
pub mod secrets;
pub mod service;
pub mod session;
pub mod setup;
pub mod showing;
pub mod simplex;
pub mod skill_improve;
pub mod skills;
pub mod stake;
pub mod storage;
pub mod stream;
pub mod synology;
pub mod talk;
pub mod tasks;
pub mod teams;
pub mod update;
pub mod verify;
pub mod voice;
pub mod webhooks;
pub mod wecom;
pub mod weixin;
pub mod work;
pub mod x402;
pub mod x_dm;
pub mod zalo;

pub use error::CliError;
pub use ledger::{HttpLedger, Ledger, LedgerExt, TxState};
pub use session::Session;
