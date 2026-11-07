# Documentation

Everything written about Siontology, in one place. Start with the
[README](../README.md) for what the project is, then
[`ARCHITECTURE.md`](../ARCHITECTURE.md) to change the code, or
[the book](book/src/introduction.md) to use it.

## The authorities

- [Implementation status](implementation-status.md): what is built,
  simplified, simulated or missing, and every trust assumption the
  implementation adds. Every other document keeps to it.
- [State transition specification](spec/state-transition.md), v1.15: the
  rules both proof stacks implement.
- [Implementation plan](implementation-plan.md): how the architecture maps
  onto crates.
- [Changelog](../CHANGELOG.md): what changed, newest first.

## The crates

Each crate has a README: what it holds, its main types, how it fits, its
status and its tests.

[`sion-core`](../crates/sion-core/README.md) · [`sion-crypto`](../crates/sion-crypto/README.md) · [`sion-assurance`](../crates/sion-assurance/README.md) · [`sion-lp`](../crates/sion-lp/README.md) · [`sion-clearing`](../crates/sion-clearing/README.md) · [`sion-da`](../crates/sion-da/README.md) · [`sion-consensus`](../crates/sion-consensus/README.md) · [`sion-exec`](../crates/sion-exec/README.md) · [`sion-spec`](../crates/sion-spec/README.md) · [`sion-prover`](../crates/sion-prover/README.md) · [`sion-light`](../crates/sion-light/README.md) · [`sion-memory`](../crates/sion-memory/README.md) · [`sion-net`](../crates/sion-net/README.md) · [`sion-http`](../crates/sion-http/README.md) · [`sion-agent`](../crates/sion-agent/README.md) · [`sion-enclave`](../crates/sion-enclave/README.md) · [`sion-node`](../crates/sion-node/README.md) · [`sion-cli`](../crates/sion-cli/README.md) · [`sion-app-core`](../crates/sion-app-core/README.md)

And the [`mcp-notes` example](../examples/mcp-notes/README.md), the
[app](../apps/sion-app/README.md), the [release scripts](../scripts/release/README.md),
and the [checks against other implementations of ACP and AP2](../scripts/interop/README.md).

## The book

The book's [contents](book/src/SUMMARY.md), with every chapter. Build it
with `mdbook build docs/book`.

- [Introduction](book/src/introduction.md)

### Getting started

Install it, run the agent of your own, run the demos, a devnet, a first transfer, a light client, an AI agent over MCP, and the app.

- [Run it on your computer](book/src/getting-started/your-computer.md)
- [Install from source](book/src/getting-started/install.md)
- [Run the demos](book/src/getting-started/demos.md)
- [Run a devnet](book/src/getting-started/devnet.md)
- [Your first transfer](book/src/getting-started/first-transfer.md)
- [Verify as a light client](book/src/getting-started/light-client.md)
- [Connect an AI agent over MCP](book/src/getting-started/mcp.md)
- [The app: a light node on every device](book/src/getting-started/app.md)

### Your agent

What the agent does and how: runs within limits, models, autonomy, memory, checks, security, tools, the browser, skills, channels, messengers, voice, schedules, an agent brought over from OpenClaw or Hermes, servers, work, teams, renting it out, selling and buying through ACP and AP2, and graphs.

- [Your agent](book/src/agent/index.md)
  - [A run, within its limits](book/src/agent/run.md)
  - [Models](book/src/agent/models.md)
  - [What it does alone, and what it asks](book/src/agent/autonomy.md)
  - [Memory](book/src/agent/memory.md)
  - [Checked before it is done](book/src/agent/checks.md)
  - [Security](book/src/agent/security.md)
- [Commands, scripts and MCP servers](book/src/agent/tools.md)
  - [A browser and a desktop](book/src/agent/browser.md)
- [Skills from others](book/src/agent/skills.md)
  - [What a skill may do](book/src/agent/skill-permissions.md)
  - [Skills that get better](book/src/agent/skill-improvement.md)
  - [Improving itself](book/src/agent/self-improvement.md)
- [Channels](book/src/agent/channels.md)
  - [Messengers](book/src/agent/messengers.md)
  - [Pairing and voice](book/src/agent/voice.md)
- [Profiles and schedules](book/src/agent/schedules.md)
  - [Coming from OpenClaw or Hermes](book/src/agent/import.md)
- [Run it on a server](book/src/agent/server.md)
- [Work it takes on by itself](book/src/agent/work.md)
- [Subagents and hired help](book/src/agent/teams.md)
- [Renting your agent out](book/src/agent/renting.md)
- [Selling and buying: ACP and AP2](book/src/agent/commerce.md)
- [Graphs of agents](book/src/agent/graphs.md)

### Build on the ledger

Wallets, transfers, charters, agent identities, claims, bonds, tasks, memory, storage, markets, x402, streams and light-client verification, from the command line.

- [Wallets and keys](book/src/guides/wallets.md)
- [Transfers](book/src/guides/transfers.md)
- [Charters for AI agents](book/src/guides/charters.md)
- [Agent identities from the command line](book/src/guides/agents.md)
- [Issuing and assessing claims](book/src/guides/claims.md)
- [Bonds and challenges](book/src/guides/bonds-and-challenges.md)
- [Task Charters from the command line](book/src/guides/tasks.md)
- [Memory logs from the command line](book/src/guides/memory.md)
- [Storage from the command line](book/src/guides/storage.md)
- [Markets and clearing](book/src/guides/markets.md)
- [x402: selling and buying an API](book/src/guides/x402.md)
- [Streams from the command line](book/src/guides/streams.md)
- [Light-client verification](book/src/guides/light-client.md)

### Run a network

A devnet, a network of processes, staking and epochs, delegation, forced inclusion, and the benchmarks.

- [Running a devnet](book/src/guides/devnet.md)
- [Running a network of processes](book/src/guides/network.md)
- [Staking and epochs](book/src/guides/staking.md)
- [Delegating stake](book/src/guides/delegation.md)
- [Forced inclusion and the censorship demo](book/src/guides/forced-inclusion.md)
- [Benchmarks](book/src/guides/benchmarks.md)

### Concepts

Every mechanism of the design, each chapter ending with what the implementation does and does not do.

- [Neo-blockchains](book/src/concepts/neo-blockchains.md)
- [Two-sorted state](book/src/concepts/two-sorted-state.md)
- [Claims and the knowledge log](book/src/concepts/claims.md)
- [Pulses](book/src/concepts/pulses.md)
- [Linear Lanes](book/src/concepts/linear-lanes.md)
- [Escrow Lanes](book/src/concepts/escrow-lanes.md)
- [Contention Consensus](book/src/concepts/contention-consensus.md)
- [Two levels of finality](book/src/concepts/finality.md)
- [The Fold and World Proofs](book/src/concepts/the-fold.md)
- [Data availability](book/src/concepts/data-availability.md)
- [The Assurance Algebra](book/src/concepts/assurance/index.md)
  - [Atoms and formulas](book/src/concepts/assurance/atoms-and-formulas.md)
  - [Cost of corruption](book/src/concepts/assurance/cost-of-corruption.md)
  - [The decision rule](book/src/concepts/assurance/decision-rule.md)
  - [Reservations](book/src/concepts/assurance/reservations.md)
  - [Bonds, challenges and slashing](book/src/concepts/assurance/bonds-challenges-slashing.md)
  - [Insurance](book/src/concepts/assurance/insurance.md)
- [Verifiable Clearing](book/src/concepts/verifiable-clearing.md)
- [Agent Charters](book/src/concepts/agent-charters.md)
- [Agent identities and subagents](book/src/concepts/agent-identities.md)
- [Payment streams](book/src/concepts/payment-streams.md)
- [Memory for agents and robots](book/src/concepts/memory.md)
- [Storage deals](book/src/concepts/storage.md)
- [Task Charters](book/src/concepts/task-charters.md)
- [Privacy](book/src/concepts/privacy.md)
- [Post-quantum identity](book/src/concepts/post-quantum.md)
- [Batched authorization](book/src/concepts/batched-authorization.md)
- [Censorship resistance](book/src/concepts/censorship-resistance.md)
- [Validators, staking and epochs](book/src/concepts/committees.md)
  - [Staking](book/src/concepts/validators/staking.md)
  - [Epochs and committees](book/src/concepts/validators/epochs.md)
  - [Rewards](book/src/concepts/validators/rewards.md)
  - [Slashing for equivocation](book/src/concepts/validators/equivocation-slashing.md)
  - [Pooled stake](book/src/concepts/validators/pooled-stake.md)

### Reference

The CLI, the HTTP API, the MCP tools, the transaction kinds, events, configuration, the chain identity, the World Proof format and the error messages.

- [Command line](book/src/reference/cli.md)
- [HTTP API](book/src/reference/http-api.md)
- [MCP tools](book/src/reference/mcp.md)
- [Transaction kinds](book/src/reference/transactions.md)
- [Events](book/src/reference/events.md)
- [Configuration](book/src/reference/configuration.md)
- [The chain identity](book/src/reference/chain-identity.md)
- [The World Proof format](book/src/reference/world-proof.md)
- [Error messages](book/src/reference/errors.md)

### Architecture of the code

The crate map, a transaction from wallet to receipt, the validator thread, the gateway, the prover, storage, and how proof stack B stays independent.

- [Crate map](book/src/architecture/crates.md)
- [A transaction, from wallet to receipt](book/src/architecture/transaction-flow.md)
- [The validator thread](book/src/architecture/validator.md)
- [The gateway](book/src/architecture/gateway.md)
- [The prover](book/src/architecture/prover.md)
- [Storage](book/src/architecture/storage.md)
- [Keeping proof stack B independent](book/src/architecture/independence.md)

### Specification

The normative state transition that both proof stacks implement.

- [The state transition specification](book/src/specification.md)

### Research

What it would take to go further: billions of payments a second, billions of agents, succinct proofs, networking between machines, reliable agents.

- [Research](book/src/research/index.md)
  - [Billions of payments a second](book/src/research/billion-scale.md)
  - [Billions of agents](book/src/research/agent-scale.md)
  - [Succinct validity proofs](book/src/research/succinct-proofs.md)
  - [Networking between machines](book/src/research/networking.md)
  - [Reliable, self-improving agents](book/src/research/agents.md)

### Security

The trust model, the threat model, the reviews and their fixes, and how to report a vulnerability.

- [Trust model](book/src/security/trust-model.md)
- [Threat model](book/src/security/threat-model.md)
- [Reviews and fixes](book/src/security/reviews.md)
- [Responsible disclosure](book/src/security/disclosure.md)

### Project

Operations, questions and answers, the glossary and the roadmap.

- [Operations](book/src/project/operations.md)
- [FAQ](book/src/project/faq.md)
- [Glossary](book/src/project/glossary.md)
- [Roadmap](book/src/project/roadmap.md)

### Papers

The architecture paper.

- [Neo-Blockchains: The Fold Architecture](book/src/papers/architecture.md)

## Elsewhere in the repository

- [`AGENTS.md`](../AGENTS.md): the rules for anyone, person or AI, who
  changes the code.
- [Contributing](../CONTRIBUTING.md), [security](../SECURITY.md),
  [governance](../GOVERNANCE.md), [code of conduct](../CODE_OF_CONDUCT.md),
  [trademarks](../TRADEMARKS.md).
- [Brand guidelines](../brand/guidelines.md), and the website's
  [source](../site/).
