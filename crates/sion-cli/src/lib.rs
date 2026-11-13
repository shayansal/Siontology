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
pub mod acp;
pub mod acp_cli;
pub mod agent_import;
pub mod agent_mcp;
pub mod agent_memory;
pub mod agent_storage;
pub mod agent_x402;
pub mod agents;
pub mod ap2;
pub mod archive;
pub mod batcher;
pub mod bench;
pub mod bench_cli;
pub mod bridge;
pub mod browser_profiles;
pub mod c2pa;
pub mod calendar;
pub mod channels;
pub mod chatgpt;
pub mod clouds;
pub mod code_improve;
pub mod cohorts;
pub mod consolidate;
pub mod credentials;
pub mod delegation;
pub mod demos;
pub mod deploy;
pub mod describe;
pub mod dingtalk;
pub mod doctor;
pub mod documents;
pub mod drafts;
pub mod email;
pub mod embeddings;
pub mod erc8004;
pub mod error;
pub mod feeds;
pub mod feishu;
pub mod files;
pub mod gateway;
pub mod github;
pub mod github_hooks;
pub mod github_tools;
pub mod google_chat;
pub mod graphs;
pub mod home_assistant;
pub mod hooks;
pub mod imessage;
pub mod immune;
pub mod improve_loop;
pub mod irc;
pub mod jwt;
pub mod ledger;
pub mod lessons;
pub mod listening;
pub mod live;
pub mod logins;
pub mod mattermost;
pub mod mcp;
pub mod mcp_auth;
pub mod mcp_serve;
pub mod meeting;
pub mod meeting_bot;
pub mod meeting_voice;
pub mod memory;
pub mod messengers;
pub mod model_routes;
pub mod network;
pub mod nextcloud;
pub mod nostr;
pub mod ntfy;
pub mod openai_api;
pub mod pairing;
pub mod phone;
pub mod practice;
pub mod profiles;
pub mod projects;
pub mod qq;
pub mod realtime;
pub mod recall;
pub mod record;
pub mod rent_cli;
pub mod rentals;
pub mod reputation;
pub mod runtime;
pub mod schedules;
pub mod screens;
pub mod sealed;
pub mod secrets;
pub mod self_improve;
pub mod service;
pub mod session;
pub mod setup;
pub mod showing;
pub mod simplex;
pub mod skill_import;
pub mod skill_improve;
pub mod skill_registries;
pub mod skills;
pub mod sound;
pub mod stake;
pub mod storage;
pub mod stream;
pub mod synology;
pub mod talk;
pub mod tasks;
pub mod teach;
pub mod teams;
pub mod training;
pub mod transcribe;
pub mod trust;
pub mod up;
pub mod update;
pub mod verify;
pub mod voice;
pub mod vouching;
pub mod web_identity;
pub mod webhooks;
pub mod wecom;
pub mod weixin;
pub mod work;
pub mod worktrees;
pub mod x402;
pub mod x_api;
pub mod x_dm;
pub mod x_mentions;
pub mod zalo;

pub use error::CliError;
pub use ledger::{HttpLedger, Ledger, LedgerExt, TxState};
pub use session::Session;
