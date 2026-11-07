# sion-agent

The agent runtime of Siontology: the loop that lets a language model work with tools, under rules a person sets and limits the ledger enforces. It is ports and adapters. The crate defines what it needs, a `Model` and `Tool`s, and knows nothing of nodes or wallets; the `sion` command plugs the ledger in as tools (balances, payments under an Agent Charter, memory). Two ideas matter to a user. Every tool declares the most it can do (`Effect`: read, write, external, spend), and a session's `Autonomy` level (L0 to L6) decides what runs, what waits for a person and what is refused. And money is bounded twice, here and by the agent's [charter](../../docs/book/src/concepts/agent-charters.md) on the ledger, which no runtime setting can loosen.

Part of [Siontology](../../README.md) · [Architecture](../../ARCHITECTURE.md)
· Status: **Partly built** (the runtime is tested with a scripted model and stand-ins for providers' and services' APIs, see
[implementation status](../../docs/implementation-status.md)).

About 39,400 lines of Rust under `src/` at the time of writing (`find crates/sion-agent/src -name '*.rs' -print0 | xargs -0 cat | wc -l`).

## The steps of a run

`Runtime::run` (`session.rs`) takes a `State` and works until the model is done, the turns run out, or a person must decide. Each step and where it lives:

| Step | What happens | Where |
|---|---|---|
| Ask the model | Builds a `Request` (the fixed system prompt, the messages, the tools shown) and asks the models in order of preference; a model that fails is cooled down for 30 seconds (`Limits::cooldown`) and the next one is asked. Providers stream text as it is written. | `session.rs` (`ask_with`), `model.rs`, `providers.rs`, `scripted.rs` |
| Check each call | Looks the tool up, decides whether the call runs, waits or is refused, and consults the journal first. | `session.rs` (`check`), `tools.rs`, `policy.rs` |
| Policy and autonomy | `Autonomy::decide` maps an `Effect` to run, ask or refuse; `Standing` holds grants, approvals and taint. A skill's `allowed-tools` can pre-approve calls. | `policy.rs`, `permissions.rs` |
| Run the tool | Calls that only read run side by side in a turn. Shell and scripts run in the sandbox. | `session.rs` (`run_calls`), `builtin.rs`, `sandbox.rs` |
| Journal | A call that adds to the world is recorded under an idempotency key before it runs. An identical repeat returns the recorded outcome, saying so (`REPLAYED`). A call whose outcome is unknown is not repeated until `Journal::settle`. Calls meant to run again (shell, file writes) are not journaled. | `journal.rs`, `tools.rs` (`Dedup`) |
| Taint | An `Outcome` marked `from_outside` taints the session for a day; until then external calls and spending need approval. A confidential label holds calls the same way. | `policy.rs` (`Standing::taint`), `tools.rs`, `session.rs` (`finish`) |
| Quarantine | If asked for, content from outside is held under a handle and read by a model with no tools, whose answers are held to a schema. | `quarantine.rs` |
| Imitated notes | The runtime's own bracketed notes are marked where a tool's result imitates them. | `imitation.rs` |
| Approvals | A call that needs a person ends the run as `Ended::NeedsApproval`. `Runtime::approve_as` and `decline_as` decide it, with who decided and a scope; approvals expire. | `session.rs` (`Pending`, `Decided`, `Scope`), `policy.rs` |
| Budgets | Turns start at `Limits::turns` and grow with progress up to `max_turns`. An optional token budget is checked before each model call, with a reserve for a last answer. | `session.rs` (`Limits`, `wrap_up`) |
| Circling | A call that keeps giving the same result is flagged, then the run ends as `Ended::Stuck`; a call that keeps failing is blocked. | `session.rs` (`circling`) |
| Compaction | Long conversations are compacted: old results are cut first, and the summary begins with what the session did that lasts. | `session.rs` (`mask_old_results`, `compact_now`, `summarize`) |
| Verify | A command the operator gives checks the task each time the model says it is done. | `verify.rs`, `Runtime::verify` |
| Pause and resume | `State` serialises. A run that needs approval stops and resumes once a person decides. The kill switch (`Runtime::stopped`) stops a run before its next call. | `session.rs` (`State`) |
| Trajectory | A session exports as one chat-format JSON line, a digest, or an OpenTelemetry form. | `trace.rs` |
| Watching | `Seen` events tell a watcher what the model writes and which tools run or wait. | `session.rs` (`Seen`) |

## What is in it

| Group | Modules | What they hold |
|---|---|---|
| Run core | `session`, `policy`, `journal`, `tools`, `verify`, `trace`, `quarantine`, `imitation`, `permissions` | The loop and everything it enforces, as in the table above; a large toolbox's search (BM25), and what is loaded from it. |
| Around calls | `code_mode`, `hooks` | Programs in Python that call the session's tools that only read, in the sandbox; the operator's hooks before and after calls, which may block one. |
| Models and providers | `model`, `providers`, `cloud`, `scripted` | Provider-independent request and response types; the Anthropic Messages API, OpenAI-compatible chat, Google Gemini and OpenAI Responses over `sion-http`, AWS Bedrock's Converse API (Signature Version 4) and Google Vertex AI (tokens minted from a service account's key or a person's sign-in), each given back the reasoning it gave and told how hard to think; a model that plays a script. |
| Tools: builtin | `builtin`, `code` | Workspace files, a shell in the sandbox, web pages; edits, patches, file search and the project's checker and tests. |
| Tools: web and screens | `websearch`, `browser`, `cdp`, `takeover`, `desktop`, `canvas`, `teach` | Web search services; Chromium driven over the DevTools Protocol, with a profile a person signed in with if given, and its page handed to them on a page of this machine's; an X desktop seen in screenshots and used in code; a canvas of A2UI components a person watches and acts on; a person's browser recorded while they do a task once, and written down as a draft skill. |
| Tools: sandbox and egress | `sandbox`, `egress`, `encoding` | Bubblewrap or a Docker image pinned by digest; a filtering proxy and an in-sandbox bridge for the hosts a skill's scripts may reach (`bin/sion-sandbox-bridge.rs`); Base64. |
| MCP | `mcp`, `mcp_http`, `mcp_input`, `oauth` | An MCP client over standard input and output and over HTTP; elicitation and sampling; OAuth 2.1 for servers that ask for it. Tools are offered only while each definition matches the one an operator reviewed. |
| Skills and their improvement | `skills`, `evals`, `evolve`, `judge` | Skills in the Agent Skills format with a canonical bundle; evaluation sets; reflective evolution of a skill's variants; a step-by-step judge. |
| Memory over past work | `search`, `consolidate`, `dream` | BM25 search of past sessions; a model proposes notes and gates decide; going over transcripts while idle. |
| Graphs and comparisons | `graph`, `compare` | Graphs of agents, each node a session; comparing one agent, voting and graphs on the same budget; Wilson and Newcombe intervals at any level. |
| Improving itself | `archive`, `guard`, `harvest`, `remote` | An archive of an agent's variants, the next to build on drawn by Thompson sampling over each line's passes; guards that hold outside an improvement loop (leaks of sealed tasks, canaries, what a patch may touch and add); cases from sessions whose outcome is known; scoring a candidate built from changed code over its standard input and output, every model and tool answer given by the harness. |
| Schedules and work | `schedule`, `work` | Once, interval and cron times in UTC; work an agent takes on by itself under mandates. |

## Main types and functions

- [`Runtime`, `State`, `Limits`, `Ended`, `Seen`](src/session.rs): the session loop, its serialisable state, its limits, how it ended and what a watcher sees.
- [`Model`, `Request`, `Response`, `Message`, `Block`](src/model.rs): what the runtime asks of a model, whatever the provider.
- [`Tool`, `Toolbox`, `Effect`, `Outcome`, `Dedup`](src/tools.rs): a tool declares an effect; the toolbox is what a session draws from.
- [`Autonomy`, `Policy`, `Standing`, `Decision`](src/policy.rs): levels L0 to L6, the rules, and a session's approvals and taint.
- [`Journal`](src/journal.rs): the record of calls that changed something.
- [`Sandbox`](src/sandbox.rs): runs commands where they can write only the workspace.
- [`Scripted`](src/scripted.rs): a model that plays a list of steps, with `when` and `unless` conditions on the conversation.
- [`providers::Anthropic`, `OpenAiCompatible`, `Gemini`, `OpenAiResponses`](src/providers.rs): the HTTPS providers.
- [`cloud::Bedrock`, `cloud::Vertex`, `cloud::SigV4`, `cloud::GoogleCredentials`, `cloud::NAMED`](src/cloud.rs): the cloud platforms, and the providers known by name.
- [`canvas::Canvas`](src/canvas.rs): a page the agent shows a person things on, as A2UI messages, and hears their actions from.
- [`skills`](src/skills.rs), [`evals::EvalSet`](src/evals.rs), [`evolve::evolve`](src/evolve.rs): skills, evaluation sets and improvement against them.
- [`archive::Archive`](src/archive.rs), [`guard`](src/guard.rs), [`harvest::case_of`](src/harvest.rs), [`remote::serve` and `remote::run_candidate`](src/remote.rs): what the agent improving itself is built from.
- [`graph`](src/graph.rs) and [`work`](src/work.rs): graphs of agents and self-started work.

## How it fits

Depends on: `sion-core` and `sion-http` (workspace crates), plus `regex`, `serde`, `serde_json` and `thiserror`; `tiny_http` for tests. Used by: `sion-cli`, and so by `sion-app-core` through it. The crate does not know the ledger: `sion-cli` supplies the ledger tools, the memory tools, profiles, channels and messengers. It does not enforce the charter either; validators check every payment against it whatever the runtime allowed (`agent:local-policy` names the trust placed in the local runtime).

## Tests

About 195 unit tests under `src/` and 94 tests under `tests/` (counted with `grep -c "#\[test\]"`; no test is `#[ignore]`). The tests under `tests/` are `session.rs` (44, the loop, approvals, journal, compaction, pause and resume), `providers.rs` (14, against `tiny_http` stand-ins for the APIs), `reasoning.rs` (11, reasoning kept and given back, and how hard each provider is asked to think, against the same stand-ins), `mcp.rs` (8), `skill_permissions.rs` (5), `code_mode.rs` (4, programs that call tools, and the operator's hooks; skipped where bubblewrap or python3 is not there), `browser.rs` (5, skipped where Chromium, Xvfb and bubblewrap are not installed), `tool_search.rs` (1), `teach.rs` (1) and `attacks.rs` (1, nine attacks on a scripted model that obeys everything it reads, showing what holds and that two get through, as the status file records).

All runs use a scripted model and stand-ins for providers' APIs: the implementation status says the environment these were made in reaches none of them, so a run against a real model is not part of the tests. The sandbox tests run bubblewrap for real, and Docker with `SION_TEST_DOCKER_IMAGE`.

```text
cargo test -p sion-agent
```

## Read more

- [Your agent](../../docs/book/src/agent/index.md) and [A run, within its limits](../../docs/book/src/agent/run.md)
- [Models](../../docs/book/src/agent/models.md)
- [What it does alone, and what it asks](../../docs/book/src/agent/autonomy.md)
- [Checked before it is done](../../docs/book/src/agent/checks.md)
- [Security](../../docs/book/src/agent/security.md)
- [Commands, scripts and MCP servers](../../docs/book/src/agent/tools.md), [a browser and a desktop](../../docs/book/src/agent/browser.md), [a canvas](../../docs/book/src/agent/canvas.md)
- [Skills from others](../../docs/book/src/agent/skills.md), [Skill permissions](../../docs/book/src/agent/skill-permissions.md), [Skills that get better](../../docs/book/src/agent/skill-improvement.md)
- [Graphs of agents](../../docs/book/src/agent/graphs.md), [Profiles and schedules](../../docs/book/src/agent/schedules.md), [Work it takes on](../../docs/book/src/agent/work.md)
- [Memory](../../docs/book/src/agent/memory.md)
