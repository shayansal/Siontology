# sion-exec

The Siontology state machine, also called the reference executor. State is [two-sorted](../../docs/book/src/concepts/two-sorted-state.md): linear resources (coins, allotments, positions) are objects with an owner and a version, each version consumed at most once, and knowledge (claims) goes into a grow-only log that never conflicts. The crate defines every object kind and transaction kind, derives each transaction's coordination class from what it touches (never from what the sender declares), and executes a [Pulse](../../docs/book/src/concepts/pulses.md) in three phases: owned transactions, then contended ones in consensus order, then the commutative deferred effects and whatever the schedule says is due. Execution doubles as witness generation: every chunk of a Pulse can be re-executed statelessly from its witness, which is what stack A of [the Fold](../../docs/book/src/concepts/the-fold.md) checks.

Part of [Siontology](../../README.md) · [Architecture](../../ARCHITECTURE.md)
· Status: **Built** (the state-machine rows of the status file, from two-sorted state and Agent Charters to claims, markets, memory logs, tasks, pooled stake and storage deals, are built at devnet scale; see [implementation status](../../docs/implementation-status.md)).

## What is in it

About 13,750 lines of Rust under `src/` at the time of writing (`find crates/sion-exec/src -name '*.rs' -print0 | xargs -0 cat | wc -l`).

| Module | What it holds |
|---|---|
| `object`, `account`, `store` | Objects, owners, versions and the coordination class of each; account state; object stores and the recording overlay that execution runs over |
| `tx` | Transactions, `TxKind` (every operation), batches and fee floors, and `Transaction::class` |
| `auth` | Authorization (spec §4.3): the authorizing policy, batches, keyless subagents |
| `execute` | One transaction: prepare, apply, or fail with the fee charged |
| `pulse` | One Pulse: canonical order, the three phases, chunk statements, traces and witnesses |
| `stateless` | Re-executing a chunk from its witness alone |
| `state`, `genesis` | The full state, chain configuration, world commitment, and the opening state |
| `claim`, `index` | Claims in the knowledge log; the claim index of each owner's latest claim under a key |
| `assurance` | The assurance contention domain: the registry plus the funds it holds (rules are in `sion-assurance`) |
| `charter`, `cohort`, `delegation` | Agent Charters and agent identities (lineage, token buckets, overlays); cohorts of subagents committed as a Merkle sum tree; pooled stake |
| `escrow` | Bounded shared totals split into single-writer allotments |
| `market` | Markets that clear in certified batches (rules are in `sion-clearing`) |
| `stream`, `task`, `storage` | Payment streams, Task Charters, storage deals |
| `memory` | Memory log heads and their consistency rule (the logs themselves are in `sion-memory`) |
| `validators`, `beacons` | Stake, epochs, committees, equivocation evidence and forced-inclusion obligations; beacon records |
| `wallet` | Building and signing transactions: coin selection, fees, inputs |
| `sim` | A single-process chain for tests and demos. Simulation only: no lanes, consensus or data availability |
| `error` | `ExecError` |

## Main types and functions

- [`State`](src/state.rs): the full state a node holds (objects, claims, knowledge log, state tree); [`ChainConfig`](src/state.rs) and [`WorldCommitment`](src/state.rs) go with it.
- [`Object`](src/object.rs), `ObjectData`, `Owner`: one piece of mutable state, its owner and its version.
- [`Transaction`](src/tx.rs), `SignedTransaction`, `TxKind`: what is signed, what is submitted, and the operations.
- [`execute_transaction`](src/execute.rs): runs one transaction over an object store; the result is `ExecStatus::Success`, `Failed` (fee charged, inputs released) or `Rejected` (nothing changes).
- [`execute_pulse`](src/pulse.rs) and `execute_pulse_traced`: run a Pulse from a `PulseInput`; the traced form also cuts it into chunks.
- [`ChunkStatement`, `ChunkTrace`, `Witness`](src/pulse.rs): what a chunk claims, and what is needed to check it.
- [`replay_chunk`](src/stateless.rs): re-executes a chunk from its witness and compares every root and accumulator with the statement; a read outside the witness invalidates the chunk.
- [`genesis`](src/genesis.rs) with `GenesisSpec`: builds the opening `State`.
- [`Wallet`](src/wallet.rs): holds an account's dual keypair and turns an operation into a valid, signed transaction against a `StateView`.
- [`sim::Chain`](src/sim.rs): executes Pulses directly, replays every chunk and checks conservation after each Pulse.

## How it fits

Depends on: `sion-core`, `sion-crypto`, `sion-assurance`, `sion-clearing`, `sion-lp` and `sion-consensus` (committee sampling and the lane and DAG message types), with `borsh`, `serde`, `serde_json`, `thiserror` and `hex` outside the workspace. Used by: `sion-spec` (types only), `sion-prover`, `sion-light`, `sion-node`, `sion-cli` and `sion-app-core`.

It executes what has been ordered; it does not order anything. Lane certificates, consensus, networking, persistence and proving belong to `sion-consensus`, `sion-node` and `sion-prover`. Every consensus-critical value has one Borsh encoding, and the rules this crate implements are written in [the specification](../../docs/spec/state-transition.md). `sion-spec` is a second implementation of that specification, and the two must agree.

## Tests

- Unit tests under `src/`: 40 `#[test]` functions (one `proptest` block).
- `tests/scenarios.rs` (20 tests): end-to-end scenarios for the transaction kinds. Every Pulse runs through `sim::Chain`, so each scenario also exercises stateless replay.
- `tests/properties.rs` (2 tests, one `proptest`): random workloads. Supply is conserved, the vault balances, every chunk replays from its witness and two nodes agree.
- `tests/buckets.rs` (2 tests, one `proptest`): random trees of spawns, cohorts, materialisations and payments never pay more than the token bucket allows.
- `tests/validators.rs` (5), `tests/staking_review.rs` (2) and `tests/pool_slashing.rs` (1): staking, epochs, rewards, slashing, forced-inclusion obligations, regressions from the independent review, and slashing of pooled stake.
- `tests/storage.rs` (6): storage rounds, beacon records and challenges.
- Counts come from `grep -c "#\[test\]"`: 78 in all. No test is `#[ignore]`d.

Run them with `cargo test -p sion-exec`. The differential tests that compare this crate with `sion-spec` are in `sion-spec/tests`.

## Read more

- [Two-sorted state](../../docs/book/src/concepts/two-sorted-state.md), [Pulses](../../docs/book/src/concepts/pulses.md) and [Claims](../../docs/book/src/concepts/claims.md)
- [A transaction, from wallet to receipt](../../docs/book/src/architecture/transaction-flow.md)
- [Agent Charters](../../docs/book/src/concepts/agent-charters.md), [agent identities](../../docs/book/src/concepts/agent-identities.md) and [batched authorization](../../docs/book/src/concepts/batched-authorization.md)
- [Escrow Lanes](../../docs/book/src/concepts/escrow-lanes.md), [payment streams](../../docs/book/src/concepts/payment-streams.md), [Task Charters](../../docs/book/src/concepts/task-charters.md), [storage](../../docs/book/src/concepts/storage.md) and [memory](../../docs/book/src/concepts/memory.md)
- [Validators and epochs](../../docs/book/src/concepts/validators/epochs.md) and [pooled stake](../../docs/book/src/concepts/validators/pooled-stake.md)
- [Transactions reference](../../docs/book/src/reference/transactions.md)
- [The state transition specification](../../docs/book/src/specification.md)
