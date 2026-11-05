# sion-cli

The `sion` command and its library. It is the interface to the rest of Siontology: it runs a devnet or the nodes and gateways of a network, holds wallets, signs and submits every kind of transaction, verifies a Pulse as a light client, and serves the ledger to AI agents over MCP. It is also where the agent runtime of [`sion-agent`](../sion-agent/README.md) meets the ledger: the crate gives a run the ledger as tools, keeps its sessions, and connects profiles to schedules, HTTP, messengers and voice. Two ideas to know. `main.rs` parses the command line with `clap` and hands each command to a module; the modules are also a library, so tests and the app use them directly. And a wallet signs what a command shows as a sentence (`describe`) before it sends it.

Part of [Siontology](../../README.md) · [Architecture](../../ARCHITECTURE.md)
· Status: **Built** for the ledger's commands (the status file's "CLI, HTTP API, MCP server for agents"); the agent's commands belong to the agent runtime, which is **Partly built**, tested with a scripted model and stand-ins for providers and messengers (see
[implementation status](../../docs/implementation-status.md)).

78,254 lines of Rust under `src/` at the time of writing (`find crates/sion-cli/src -name '*.rs' -print0 | xargs -0 cat | wc -l`), the largest crate in the workspace.

## From `sion <command>` to the module

`main.rs` handles the commands that need no node first (`doctor`, `setup`, `update`, `deploy`, `devnet`, `genesis`, `node`, `gateway`, `keys`, `bench`, `mcp mail|calendar|documents`), then opens a `Session` on the node and dispatches the rest. Bare `sion` talks with your agent (`talk::bare`).

| Command | Implemented in |
|---|---|
| `sion devnet` | `main.rs` (`run_devnet`), over `sion_node::Devnet` |
| `sion genesis`, `node`, `gateway`, `keys validator`, `keys gateway` | `network.rs` |
| `sion keys new`, `keys show` | `main.rs`, `session.rs` |
| `sion status`, `forced`, `censor`, `tx`, `pulse`, `events` | `main.rs` over `ledger.rs`; sentences from `describe.rs` |
| `sion balance`, `faucet`, `send`, `bond`, `claim`, `accept`, `challenge`, `rule`, `charter`, `market` | `main.rs`, with `session.rs` building and submitting |
| `sion receipt`, `verify` | `verify.rs` (light-client checks) |
| `sion stream` | `stream.rs` (payer book, payee desk) |
| `sion task` | `tasks.rs` |
| `sion stake` | `stake.rs` |
| `sion memory` (`open`, `remember`, `commit`, `recall`, `show`, `consolidate`, `dream`, `archive`, `restore`) | `memory.rs`, `consolidate.rs`, `archive.rs`, `recall.rs` |
| `sion skill` | `skills.rs` (releases, audits, install, bounties), `skill_improve.rs` (eval, evolve), `skill_import.rs` (import, starter), `skill_registries.rs` (search, ClawHub, skills.sh, well-known indexes), `teach.rs` (teach) |
| `sion storage` | `storage.rs` |
| `sion demo` | `demos.rs` |
| `sion bench` | `bench.rs` |
| `sion mcp` | `mcp.rs` (the ledger's tools); `mcp mail`, `calendar`, `documents` through `mcp_serve.rs` |
| `sion x402` | `x402.rs` |
| `sion setup`, `doctor`, `update`, `deploy` | `setup.rs`, `doctor.rs`, `update.rs`, `deploy.rs` |

`sion agent <command>` is routed in `agents.rs` (`local` for commands that need no node, then `run`):

| Command | Implemented in |
|---|---|
| `register`, `spawn`, `pay-batch`, `show`, `status`, `card`, `card-template`, `suspend`, `resume`, `revoke`, `reclaim`, `narrow` | `agents.rs`; keyless batches signed by `batcher.rs` |
| `cohort`, `materialize` | `agents.rs`, `cohorts.rs` |
| `run`, `continue`, `decisions`, `settle`, `trace`, `kill-switch` | `agents.rs` over `runtime.rs` |
| `judge`, `evolve` | `agents.rs` over `sion_agent::judge` and `sion_agent::evolve`; `skill_improve.rs` |
| `sessions` (and `sessions search`) | `agents.rs`, `recall.rs` |
| `profile` | `profiles.rs` |
| `routes` | `model_routes.rs` |
| `import` | `agent_import.rs` |
| `x-mentions` | `x_mentions.rs`, over `x_api.rs` |
| `meeting` | `meeting.rs`, `meeting_bot.rs` |
| `hooks` | `hooks.rs` |
| `browser` (login, list, remove) | `browser_profiles.rs` |
| `rent` (serve, use, show) | `rent_cli.rs`, `rentals.rs` |
| `secret`, `credential`, `login` | `secrets.rs`, `credentials.rs`, `logins.rs` |
| `chatgpt`, `weixin`, `pairing` | `chatgpt.rs`, `weixin.rs`, `pairing.rs` |
| `mcp` (add, list, review, login, logout, remove) | `agent_mcp.rs`, `mcp_auth.rs` |
| `graph` | `graphs.rs` |
| `schedule`, `scheduler` | `schedules.rs` |
| `work`, `waiting` | `work.rs` (with `github.rs`, `github_hooks.rs`, `github_tools.rs`); `waiting` in `agents.rs` |
| `up`, `service` | `up.rs`, `service.rs` |
| `serve` | `channels.rs`, with `openai_api.rs` and `a2a.rs` |
| `talk` | `talk.rs`, `voice.rs`, `listening.rs`, `sound.rs`, `realtime.rs` |
| `telegram`, `bridge` | `channels.rs`, `messengers.rs`, `bridge.rs` and one module per messenger |
| `gateway`, `host` | `gateway.rs` |

## What is in it

| Group | Modules | What they hold |
|---|---|---|
| Ledger client and sessions | `ledger`, `session`, `describe`, `verify`, `error` | The `Ledger` trait (a node over HTTP or an in-process devnet); wallets and building, submitting and waiting for transactions; sentences for transactions and receipts; light-client verification of a Pulse. |
| Ledger commands | `agents`, `cohorts`, `batcher`, `tasks`, `stake`, `stream`, `storage`, `memory`, `x402`, `bench`, `demos` | One module per family of commands, as in the tables. |
| Runtime glue | `runtime`, `channels`, `bridge`, `profiles`, `delegation`, `worktrees`, `hooks`, `agent_memory`, `agent_storage`, `screens`, `browser_profiles`, `files` | `runtime` gives a run the ledger as tools and keeps sessions; `channels` is the HTTP channel and approvals; `bridge` carries messenger chats into sessions; `delegation` lets an agent spawn subagents from a session. |
| Messengers | `messengers`, `live`, `webhooks`, `drafts`, `showing`, `pairing`, `jwt`, and `dingtalk`, `feishu`, `google_chat`, `imessage`, `irc`, `mattermost`, `nextcloud`, `nostr`, `ntfy`, `qq`, `simplex`, `synology`, `teams`, `wecom`, `weixin`, `x_api`, `x_dm`, `x_mentions`, `zalo`, `phone`, `email` | The `Messenger` interface and one module per service; WebSocket and webhook intake; typing indicators and replies as they are written; X's mentions answered with their sources, and its posts searched. |
| MCP and the first-party servers | `mcp`, `mcp_serve`, `email`, `calendar`, `documents`, `agent_mcp`, `mcp_auth`, `home_assistant` | The ledger's MCP server; a shared server loop for the mail (IMAP and SMTP), calendar (CalDAV) and documents servers; configuration and OAuth login for MCP servers an agent uses. |
| Voice | `voice`, `talk`, `listening`, `sound`, `realtime`, `meeting`, `meeting_bot` | Voice notes heard and answered aloud, by services or on the machine (whisper.cpp, Piper, a local server); a terminal conversation; a conversation that listens all the time, with a wake phrase, a model of speech, turns told by what was said, and echo cancellation; one through a realtime speech model that asks the agent; meeting notes by phone, on the machine or as a guest by the meeting's link, with speakers told apart if asked. |
| Work and GitHub | `work`, `github`, `github_hooks`, `github_tools` | The work runner, GitHub as a watch, its webhooks, and the tools a work session reads a repository with. |
| Skills | `skills`, `skill_improve`, `skill_import`, `skill_registries`, `teach` | Releases, audits and installation; evaluation, evolution and bounties; import from folders, repositories and registries; skills taught by doing a task in a browser. |
| Running on a server | `up`, `deploy`, `service`, `gateway` | One process that keeps a list running; sion copied to a server over SSH; systemd and launchd units; routing channels across machines. |
| Setup and update | `setup`, `doctor`, `update`, `credentials`, `secrets`, `logins`, `chatgpt`, `agent_import`, `model_routes` | One-person setup, a check of the machine, signed self-update, sealed credentials; OpenClaw and Hermes agents imported; a model chosen for each task. |
| Renting and records | `rentals`, `rent_cli`, `reputation` | Listing an agent for rent by the token, sealed requests to its enclave, metered tokens it signs, payment by stream; agents' records from the ledger's events, and finding one for a task. |
| Interop | `a2a`, `openai_api` | Agent2Agent cards and tasks; an OpenAI-compatible endpoint. |

## Main types and functions

- [`Ledger`, `HttpLedger`, `LedgerExt`](src/ledger.rs): the node a command talks to.
- [`Session`](src/session.rs): wallets, transaction building, submitting and waiting.
- [`runtime`](src/runtime.rs): `sion agent run`, the ledger as tools, saved sessions, the kill switch.
- [`messengers::Messenger`](src/messengers.rs): one interface for every chat service.
- [`bridge::run`](src/bridge.rs): messenger chats answered by a profile.
- [`profiles`](src/profiles.rs): how to run an agent, kept under a name.
- [`mcp_serve::serve`](src/mcp_serve.rs): serves a tool set over MCP on standard input and output.
- [`verify`](src/verify.rs): light-client verification of a Pulse.
- [`describe`](src/describe.rs): transactions and receipts as sentences.
- [`demos`](src/demos.rs): scripted end-to-end demonstrations.

## How it fits

Depends on: `sion-core`, `sion-crypto`, `sion-assurance`, `sion-clearing`, `sion-consensus`, `sion-exec`, `sion-da`, `sion-light`, `sion-prover`, `sion-spec`, `sion-node`, `sion-memory`, `sion-http` and `sion-agent`; `sion-net` for tests only. Outside dependencies include `clap`, `ureq`, `tiny_http`, `chacha20poly1305`, `k256`, `qrcode` and `nnnoiseless` (RNNoise's network, for telling speech from other sound). Used by: `sion-app-core`. The crate holds no consensus or state-machine rules; it builds transactions and asks nodes. The messenger modules reach services over their own APIs, which tests replace with stand-ins.

## Tests

273 `#[test]` functions (counted with `grep -c "#\[test\]"`): 221 under `src/` and 52 in `tests/`. Most `tests/` files are end-to-end runs on a real in-process devnet with a scripted model: `agent_runtime.rs`, `agent_tools.rs`, `agent_memory.rs`, `agent_recall.rs`, `agent_reach.rs`, `agent_work.rs` and `agent_takes_work.rs` (against a stand-in for GitHub), `skill_supply.rs`, `skill_improve.rs`, `streams.rs`, `memory_archive.rs`, `bridging.rs`, `channels_more.rs`, `channels_across.rs`, `channels_voice.rs` (11 tests, with stand-in microphones and transcribers), `agent_worktrees.rs` (subagents in worktrees, programs that call tools and the operator's hooks; skipped where bubblewrap does not work), `meeting.rs` and `meeting_bot.rs` (a guest joining stand-in meeting pages with Chromium, skipped where it is not found), `mcp_http.rs` (7), `network.rs` (processes), `always_on.rs` (`sion agent up` and `sion deploy` through a stand-in for SSH), `setup.rs`, `files.rs` and `chatgpt.rs`. No microphone, loudspeaker, messenger or model provider is reached.

Two tests in `tests/demos.rs` are `#[ignore]`: they run every demo and take minutes (one about a minute), so CI runs them nightly; locally use `cargo test --workspace -- --include-ignored`.

```text
cargo test -p sion-cli
```

## Read more

- [Reference: CLI](../../docs/book/src/reference/cli.md), [MCP](../../docs/book/src/reference/mcp.md)
- [Run Siontology on your computer](../../docs/book/src/getting-started/your-computer.md)
- [Agent identities from the command line](../../docs/book/src/guides/agents.md)
- [Your agent](../../docs/book/src/agent/index.md), [A run, within its limits](../../docs/book/src/agent/run.md), [Channels](../../docs/book/src/agent/channels.md), [Messengers](../../docs/book/src/agent/messengers.md), [Voice](../../docs/book/src/agent/voice.md), [Work it takes on](../../docs/book/src/agent/work.md), [Running on a server](../../docs/book/src/agent/server.md)
- [Crate map](../../docs/book/src/architecture/crates.md)
