# Siontology

Siontology is a reference implementation of a **neo-blockchain**: a network whose
product is verified knowledge, not ordered transactions. Every claim, whether
about a payment, a computation, an AI output or a physical event, carries a
proof or priced evidence, and anyone can check it from a phone.

It implements the architecture described in *Neo-Blockchains: The Fold
Architecture*: two-sorted state, consensus only where operations conflict,
a validity proof folded over every Pulse, and a knowledge layer in which
every claim states what it would cost to falsify.

It comes with an AI agent you run on your own computer: one command,
`sion setup`, makes a chain of your own and an agent whose spending the
ledger holds to its charter, and whose memory log's head it keeps. The agent answers in the terminal
and on messaging services, works in a folder you give it, browses, and
asks before it spends; the ledger refuses whatever its charter forbids,
whatever the agent was told.

This is a devnet-scale reference implementation in Rust. Some parts of the
architecture are simulated, simplified or left out, most importantly
succinct proofs (simulated by replay proofs) and networking between
machines (simplified: QUIC between processes, tested on one machine).
[`docs/implementation-status.md`](docs/implementation-status.md) lists every
simplification.

Website: [siontology.org](https://siontology.org) ·
Documentation: [the Siontology book](docs/book/src/introduction.md) ·
Paper: [Neo-Blockchains: The Fold Architecture](docs/book/src/papers/architecture.md)

## What it does

- **Two-sorted state.** Linear resources (coins, allotments, positions) are
  versioned objects that are consumed at most once. Knowledge (claims) is
  appended to a grow-only log and never conflicts. A transaction's
  coordination class is derived from what it touches.
- **Ordering only where needed.** An owned-object spend is final once its
  shard committee certifies it, with no consensus round (Linear Lanes).
  Shared totals are split into single-writer allotments (Escrow Lanes).
  Only contended objects go through BFT consensus (a certified DAG).
- **Pulses and the Fold.** About once a second the network executes a
  Pulse, signs its header, publishes its data with 2D Reed–Solomon
  erasure coding and proves it. The World Proof binds the state transition,
  every receipt, the published data and every lane certificate.
- **The Assurance Algebra.** Every claim carries a formula of the
  assumptions it rests on. Any consumer computes, under its own policy,
  what an attacker would have to spend or forfeit to falsify the claim,
  relies on it only when that exceeds the value at stake, and reserves the
  backing so no claim is relied on beyond it. Bonds are slashed to
  compensate the parties who relied on a false claim.
- **Verifiable Clearing.** A market batch settles only with an optimality
  certificate (a dual solution) that the ledger checks in linear time, with
  sealed orders and beacon-derived tie-breaking, so no solver or sequencer
  can choose winners.
- **Agent Charters.** AI agents spend inside limits the protocol enforces:
  payees, a per-payment limit, a token bucket carved down the tree of
  sub-agents, expiry, evidence requirements, receipts and delegation that
  can only narrow. A prompt-injected agent cannot exceed them; suspending,
  narrowing or revoking a charter applies to every sub-agent at once.
- **Post-quantum from the start.** Every account and validator signs with
  two unrelated families at once (ML-DSA-44 and SLH-DSA-SHA2-128f), so a
  forger must break both; payloads are encrypted with ML-KEM-768.
- **Censorship resistance.** A user whose transactions are ignored posts
  them straight to consensus as forced intents. One honest proposer is
  enough to order one; the state then holds it as an obligation of the
  sender's shard. If the shard lets the deadline pass while the intent
  could still execute, its committee is replaced, and every certified
  header counts the intents a shard owes, so censorship by more than a
  third of a committee is visible to every light client.
- **Staking and epochs.** Validators join by staking, earn a share of fees
  by stake, are slashed on signed evidence of equivocation, and leave after
  unbonding. Who certifies is part of the state transition: every
  committee is derived from the validator set in the state, epoch by
  epoch, and the World Proof covers every change.
- **Light clients.** A client that pins the genesis chain identity follows
  the committee from epoch to epoch with boundary proofs, then checks the
  committee certificate, the certified order of transactions, receipts,
  state and claim proofs, samples data availability at random and verifies
  World Proofs, without running a node.
- **Payment streams.** A payer escrows funds once and pays by revealing
  the elements of a hash chain, one per payment: the payee checks each
  with one hash, and settles thousands or millions in one transaction.
  With checkpoints, a settlement costs the ledger the same however many
  payments it covers. On one core, settlements covering about 9 × 10⁸
  payments a second executed with every signature checked (measured; see
  [the research note](docs/book/src/research/billion-scale.md) for what
  this does and does not mean).
- **Agent identities, memory and tasks.** An agent launches subagents,
  each with its own identity, account and budget, and suspending one stops
  its whole tree. A subagent can be keyless: it has no keys of its own,
  and its nearest keyed ancestor signs for it, one batch for up to 256
  transactions, so a swarm costs one post-quantum signature per batch,
  not one per action. Agents and robots keep memory in single-writer logs
  whose heads are on the ledger, so a reader can check that an answer is
  complete and a rewrite is refused. Task Charters escrow work whose
  acceptance is fixed when it is posted. `sion agent run` puts an agent to
  work with a model, under autonomy levels and approvals, while the
  ledger enforces its charter. It installs a skill from others only when
  the bonds and audits behind the skill cover what the agent could lose,
  and a skill's better version can be released on the attestations of
  verifiers, fixed before it existed, who ran its evaluation set. Agents
  spawn subagents with budgets carved from their own, hire each other
  through Task Charters, run on schedules, and answer in the terminal,
  over HTTP, OpenAI's chat API and A2A, and on 28 messaging services
  (tested against stand-ins for their APIs), on their own machine or
  through an agent gateway. They take work on by themselves from the
  GitHub repositories and chats they watch: what people on a list ask
  for, and, once their operator lets them, what a rule covers, each piece
  of work under a mandate kept on the ledger, following their pull
  requests' reviews and failed checks. They search the web, change and
  check code, use a browser with tabs, downloads and uploads, and use MCP
  servers (resources, prompts and questions to you included) and sion's
  own for mail, calendars and documents. In the terminal they can be
  spoken to live, stopped by speaking over them, and woken by a phrase.
  `sion deploy` puts them on a server of yours, where `sion agent up`
  keeps them running while your computer is off.
- **Storage deals.** A client escrows payment to have data kept; the
  provider posts collateral and proves every round, by answering random
  challenges drawn from the beacon, that it can still produce the data,
  and forfeits collateral for the rounds it misses.
- **Agent-native interfaces.** A CLI, a JSON HTTP API and a Model Context
  Protocol server, so AI agents can pay, issue claims, price a claim's
  trustworthiness before relying on it, and verify the chain. An x402
  adapter lets any HTTP API charge per request (`402 Payment Required`)
  and settle on Siontology: with `exact`, the server serves only once the
  payment's receipt verifies, and an agent paying under a charter cannot
  exceed its limits; with `stream`, each request is paid from a payment
  stream and checked with one hash.

## Quickstart

An agent of your own, on your computer ([Run it on your
computer](docs/book/src/getting-started/your-computer.md)):

```bash
sh scripts/install.sh   # or curl -fsSL https://raw.githubusercontent.com/ColdAI-org/siontology/main/scripts/install.sh | sh
sion setup              # a chain of your own, your wallet and your agent's, a model, a profile
sion                    # talk with it; sion -c continues the last conversation
sion update             # a newer release, once its signature and SHA-256 are checked
```

Building requires Rust 1.80 or later and a C compiler. `sh scripts/install.sh`
puts `sion` in `~/.local/bin` (see [Install from
source](docs/book/src/getting-started/install.md) for the image, Homebrew
and Nix), and `sion doctor` says what else the machine has.

```bash
cargo build --release
alias sion=./target/release/sion

# Run every scripted demo against an in-process devnet.
sion demo all --local
```

Or run a devnet and use it from another terminal:

```bash
sion devnet                          # 4 validators, 2 shards, API on 127.0.0.1:8730
                                     # (--standby 1 --epoch-length 24 for staking and epochs)
sion status
sion keys new dana                   # a wallet in ~/.sion/wallets (or $SION_HOME/wallets)
sion faucet dana 100
sion send dana alice 12.5
sion balance dana
sion verify --chain <identity printed by sion devnet>
sion demo invoice                    # the demos also run against a live node
```

Connect an AI agent over the Model Context Protocol (stdio):

```bash
sion mcp                             # talks to the node at $SION_NODE
```

Sell an API per request with x402, and buy from it:

```bash
sion x402 serve carol --price 0.25   # http://127.0.0.1:8402/v1/resource
sion x402 fetch http://127.0.0.1:8402/v1/resource --wallet dana --max 1
```

Or pay per request from a payment stream, one hash per request:

```bash
sion x402 serve carol --price 0.001 --streams
sion stream open dana carol --unit 0.001 --deposit 5   # prints the stream's ID
sion x402 fetch http://127.0.0.1:8402/v1/resource --stream <ID> --count 100
```

Measure everything on your machine:

```bash
sion bench                           # signatures, hashing, DA, clearing, assurance,
                                     # execution, proving and a live devnet
```

## The demos

`sion demo <name>` drives real transactions through the whole protocol
(lane certificates, consensus, Pulse execution, header certificates, data
availability and proving) and narrates what the ledger established.

| Demo | What it shows |
|---|---|
| `invoice` | Trade finance on insured knowledge: a buyer's acceptance of invoice #1042 is a bonded claim; a financier prices it with the Assurance Algebra, relies on it within its backing, and is compensated from the slashed bond when the claim proves false. |
| `agents` | An AI agent with a budget the ledger enforces: a payment to an unlisted payee and an over-limit payment are refused; revoking the charter stops the agent and its sub-agent. |
| `swarm` | Agent identities: an orchestrator launches 12 researchers in one transaction and one of them launches 3 of its own, each with its own identity, account and budget; 15 pay at once without consensus; one suspension stops the whole tree and one resumption restarts it. |
| `batch` | Batched authorization: an orchestrator launches 64 keyless subagents in one transaction and signs one batch for a payment from each; each member pays a lower fee because the signature check is shared, and nobody else can sign for them. |
| `tasks` | Task Charters: an eval orchestrator posts work from its budget; a result is paid only when it rests on the environment's claim and enough attested backing, and only once; a bounty is settled and another withdrawn; an exploit in the grader is proven and the lab is compensated from the vendor's bond. |
| `robots` | Memory for robots: two robots with their own identities and memory logs share a warehouse; a reader checks complete answers against the heads on the ledger; a rewrite is refused; a curator's refutation gives two readers two provable beliefs; an index that leaves an atom out is convicted; a camera frame is forgotten; a suspended robot's memory stops growing. |
| `storage` | Storage deals: a robot commits its memory log with its archive's root as the manifest and pays a laptop to keep the archive, proven every round; a phone that kept only half of the footage it accepted cannot answer the challenges and forfeits its collateral. |
| `market` | A sealed-bid GPU-hour market that clears at one price with an optimality certificate. |
| `receipts` | Verifiable AI inference receipts: each consumer prices the same receipt under its own assumptions (trusting a TEE, distrusting it, or assuming it broken). |
| `x402` | An AI agent pays an HTTP API per request with x402: the API answers `402`, the agent pays under its charter and retries, and the API serves once the receipt verifies; a replayed payment and an over-limit payment are refused. |
| `streams` | An agent pays an API 2,000 times from one payment stream: the server checks one hash per call and sends no transaction per call; two to four settlements, proven by checkpoint, cover all of them; an auditor checks a call's receipt against the ledger. |
| `censorship` | Forced inclusion: a user censored by the gateway gets through by posting straight to consensus; when more than a third of a committee censors too, the shard misses its deadline, its committee is replaced, certified headers count the overdue intent and a light client sees it. |
| `light` | A light client pins the chain identity, verifies the latest Pulse end to end and catches a node that serves substituted validator keys. |
| `staking` | A standby validator stakes and joins the active set at the next epoch boundary, a light client follows the committee change from genesis, rewards are claimed, and the newcomer is slashed for signing two headers for one round and loses its seat. `--local` starts a devnet with a standby validator and 24-Pulse epochs; against your own devnet, start it with `--standby 1 --epoch-length 24`. |

## Repository layout

| Crate | What it holds |
|---|---|
| [`sion-core`](crates/sion-core) | Domain-separated SHA3-256, canonical Borsh encoding, typed identifiers, fixed-point money, the RFC 9162 append-only log, the sparse Merkle state tree |
| [`sion-crypto`](crates/sion-crypto) | ML-DSA-44, SLH-DSA-SHA2-128s/128f, ML-KEM-768, dual-origin signatures, account policies, sealed envelopes, Shamir sharing |
| [`sion-assurance`](crates/sion-assurance) | The Assurance Algebra: atoms, formulas, cost of corruption, decision rule, reservations, challenges, slashing, insurance |
| [`sion-lp`](crates/sion-lp) | Exact rational simplex and linear-time optimality-certificate verification |
| [`sion-clearing`](crates/sion-clearing) | Verifiable Clearing: sealed double auctions and assignment markets with certificates |
| [`sion-da`](crates/sion-da) | 2D Reed–Solomon over the Goldilocks field, commitments, sampling, reconstruction, fraud proofs |
| [`sion-consensus`](crates/sion-consensus) | Linear Lanes, certified-DAG BFT, committees and sampling, the beacon, forced inclusion, a deterministic network simulator |
| [`sion-exec`](crates/sion-exec) | The state machine: objects, accounts, claims, bonds, Agent Charters, agent identities, escrow, markets, payment streams, memory log heads, tasks, the validator set, storage deals; Pulse execution with chunk traces and witnesses |
| [`sion-spec`](crates/sion-spec) | An independent implementation of the state transition written from [the specification](docs/spec/state-transition.md) (proof stack B) |
| [`sion-prover`](crates/sion-prover) | Proof stacks, World Proofs and their verification |
| [`sion-light`](crates/sion-light) | The light client: chain identity, header certificates, proofs, availability sampling |
| [`sion-memory`](crates/sion-memory) | Memory for agents and robots, off the ledger: memory atoms, single-writer logs, a 4D index with range, subject, link and omission proofs, belief as a fold, sealing and forgetting |
| [`sion-net`](crates/sion-net) | Networking between processes: QUIC with TLS 1.3, X25519MLKEM768 and pinned ML-DSA-44 raw public keys, bounded queues and quotas |
| [`sion-http`](crates/sion-http) | Outbound HTTPS for clients, X25519MLKEM768 offered first, refusing non-public addresses |
| [`sion-agent`](crates/sion-agent) | The agent runtime: models, tools with effects, autonomy levels and approvals, taint, a journal of side effects, budgets, sessions that pause and resume |
| [`sion-node`](crates/sion-node) | The devnet and `sion node`: validators, gateway, prover, write-ahead log and snapshots, HTTP API |
| [`sion-cli`](crates/sion-cli) | The `sion` command: devnet, nodes, wallets, payment streams, x402, agents, tasks, memory, storage deals, demos, light verification, MCP server, benchmarks |
| [`sion-app-core`](crates/sion-app-core) | The app's core: the device's key, a light client that verifies the device's own receipts, payments, storage provided from the device ([apps/sion-app](apps/sion-app)) |

## Documentation

- [The Siontology book](docs/book/src/SUMMARY.md): getting started,
  concepts, guides, a reference for the CLI, the HTTP API and the MCP tools,
  the architecture of the code, and the security model. Build it with
  `mdbook build docs/book`; it is published at
  [siontology.org/docs](https://siontology.org/docs/).
- [Neo-Blockchains: The Fold Architecture](docs/book/src/papers/architecture.md):
  the architecture paper.
- [Implementation status](docs/implementation-status.md): what is built,
  simplified, simulated or missing, and the trust assumptions the
  implementation adds.
- [State transition specification](docs/spec/state-transition.md) (v1.15):
  the normative description both proof stacks implement.
- [Benchmarks](docs/book/src/guides/benchmarks.md): measured costs, and what
  they imply.
- [Research](docs/book/src/research/index.md): billions of payments a
  second, the succinct-proof plan and the networking plan, each with what
  is built, measured or only planned.
- [Implementation plan](docs/implementation-plan.md): how the architecture
  maps onto crates.
- [Brand guidelines](brand/guidelines.md) and the [website](site/) source.
- [`AGENTS.md`](AGENTS.md): rules for anyone, human or AI, changing the code.

## Development

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

The test suite includes property tests for the Assurance Algebra, the
linear-programming certificates and the state machine, a deterministic
consensus simulator with adversarial network phases, scenario tests for
every transaction kind, differential tests between the two proof stacks,
end-to-end devnet tests with dual post-quantum signatures, and the demos.
The demo runs take minutes, so they are `#[ignore]`d and run nightly in CI;
`cargo test --workspace -- --include-ignored` runs everything.

## Contributing and community

- [Contributing](CONTRIBUTING.md) · [Code of conduct](CODE_OF_CONDUCT.md) ·
  [Governance](GOVERNANCE.md) · [Changelog](CHANGELOG.md)
- Security issues: see [SECURITY.md](SECURITY.md); please do not open public
  issues for vulnerabilities.
- The name and marks: [TRADEMARKS.md](TRADEMARKS.md).

## Licence

Licensed under either of the [Apache License, Version 2.0](LICENSE-APACHE)
or the [MIT licence](LICENSE-MIT), at your option. Unless you state
otherwise, any contribution you submit for inclusion is licensed as above,
without any additional terms or conditions.
