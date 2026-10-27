# Fold

Fold is a reference implementation of a **neo-blockchain**: a network whose
product is verified knowledge, not ordered transactions. Every claim, whether
about a payment, a computation, an AI output or a physical event, carries a
proof or priced evidence, and anyone can check it from a phone.

It implements the architecture described in *Neo-Blockchains: The Fold
Architecture*: two-sorted state, consensus only where operations conflict,
a validity proof folded over every Pulse, and a knowledge layer in which
every claim states what it would cost to falsify.

This is a devnet-scale reference implementation in Rust. Some parts of the
architecture are simulated or left out, most importantly succinct proofs.
[`docs/implementation-status.md`](docs/implementation-status.md) lists every
simplification.

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
  payees, per-payment and per-window budgets, expiry, evidence
  requirements, receipts and delegation that can only narrow. A
  prompt-injected agent cannot exceed them; revoking a charter stops every
  sub-agent at once.
- **Post-quantum from the start.** Every account and validator signs with
  two unrelated families at once (ML-DSA-44 and SLH-DSA-SHA2-128f), so a
  forger must break both; payloads are encrypted with ML-KEM-768.
- **Censorship resistance.** A user whose transactions are ignored posts
  them straight to consensus as forced intents. One honest proposer is
  enough to order one, the shard's validators certify it among themselves,
  and every certified header counts the intents a shard still owes, so
  censorship by more than a third of a committee is visible to every light
  client.
- **Light clients.** A client that pins the chain identity checks the
  committee certificate, the certified order of transactions, receipts,
  state and claim proofs, samples data availability at random and verifies
  World Proofs, without running a node.
- **Agent-native interfaces.** A CLI, a JSON HTTP API and a Model Context
  Protocol server, so AI agents can pay, issue claims, price a claim's
  trustworthiness before relying on it, and verify the chain.

## Quickstart

Requires Rust 1.80 or later.

```bash
cargo build --release
alias fold=./target/release/fold

# Run every scripted demo against an in-process devnet.
fold demo all --local
```

Or run a devnet and use it from another terminal:

```bash
fold devnet                          # 4 validators, 2 shards, API on 127.0.0.1:8730
fold status
fold keys new dana                   # a wallet in ~/.fold/wallets (or $FOLD_HOME/wallets)
fold faucet dana 100
fold send dana alice 12.5
fold balance dana
fold verify --chain <identity printed by fold devnet>
fold demo invoice                    # the demos also run against a live node
```

Connect an AI agent over the Model Context Protocol (stdio):

```bash
fold mcp                             # talks to the node at $FOLD_NODE
```

Measure everything on your machine:

```bash
fold bench                           # signatures, hashing, DA, clearing, assurance,
                                     # execution, proving and a live devnet
```

## The demos

`fold demo <name>` drives real transactions through the whole protocol
(lane certificates, consensus, Pulse execution, header certificates, data
availability and proving) and narrates what the ledger established.

| Demo | What it shows |
|---|---|
| `invoice` | Trade finance on insured knowledge: a buyer's acceptance of invoice #1042 is a bonded claim; a financier prices it with the Assurance Algebra, relies on it within its backing, and is compensated from the slashed bond when the claim proves false. |
| `agents` | An AI agent with a budget the ledger enforces: a payment to an unlisted payee and an over-limit payment are refused; revoking the charter stops the agent and its sub-agent. |
| `market` | A sealed-bid GPU-hour market that clears at one price with an optimality certificate. |
| `receipts` | Verifiable AI inference receipts: each consumer prices the same receipt under its own assumptions (trusting a TEE, distrusting it, or assuming it broken). |
| `censorship` | Forced inclusion: a user censored by the gateway gets through by posting straight to consensus; when more than a third of a committee censors too, certified headers count the overdue intent and a light client sees it. |
| `light` | A light client pins the chain identity, verifies the latest Pulse end to end and catches a node that serves substituted validator keys. |

## Repository layout

| Crate | What it holds |
|---|---|
| [`fold-core`](crates/fold-core) | Domain-separated SHA3-256, canonical Borsh encoding, typed identifiers, fixed-point money, the RFC 9162 append-only log, the sparse Merkle state tree |
| [`fold-crypto`](crates/fold-crypto) | ML-DSA-44, SLH-DSA-SHA2-128s/128f, ML-KEM-768, dual-origin signatures, account policies, sealed envelopes, Shamir sharing |
| [`fold-assurance`](crates/fold-assurance) | The Assurance Algebra: atoms, formulas, cost of corruption, decision rule, reservations, challenges, slashing, insurance |
| [`fold-lp`](crates/fold-lp) | Exact rational simplex and linear-time optimality-certificate verification |
| [`fold-clearing`](crates/fold-clearing) | Verifiable Clearing: sealed double auctions and assignment markets with certificates |
| [`fold-da`](crates/fold-da) | 2D Reed–Solomon over the Goldilocks field, commitments, sampling, reconstruction, fraud proofs |
| [`fold-consensus`](crates/fold-consensus) | Linear Lanes, certified-DAG BFT, committees and sampling, the beacon, forced inclusion, a deterministic network simulator |
| [`fold-exec`](crates/fold-exec) | The state machine: objects, accounts, claims, bonds, Agent Charters, escrow, markets; Pulse execution with chunk traces and witnesses |
| [`fold-spec`](crates/fold-spec) | An independent implementation of the state transition written from [the specification](docs/spec/state-transition.md) (proof stack B) |
| [`fold-prover`](crates/fold-prover) | Proof stacks, World Proofs and their verification |
| [`fold-light`](crates/fold-light) | The light client: chain identity, header certificates, proofs, availability sampling |
| [`fold-node`](crates/fold-node) | The devnet: validators, gateway, prover, write-ahead log and snapshots, HTTP API |
| [`fold-cli`](crates/fold-cli) | The `fold` command: devnet, wallets, demos, light verification, MCP server, benchmarks |

## Documentation

- [Implementation status](docs/implementation-status.md): what is built,
  simplified, simulated or missing, and the trust assumptions the
  implementation adds.
- [State transition specification](docs/spec/state-transition.md): the
  normative description both proof stacks implement.
- [Benchmarks](docs/benchmarks.md): measured costs, and what they imply.
- [Implementation plan](docs/implementation-plan.md): how the architecture
  maps onto crates.
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
every transaction kind, end-to-end devnet tests with dual post-quantum
signatures, and the demos.
