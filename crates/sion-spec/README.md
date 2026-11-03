# sion-spec

Proof stack B: a second implementation of the Siontology state transition, written from [`docs/spec/state-transition.md`](../../docs/spec/state-transition.md) and not from the reference executor in `sion-exec`. The Fold checks every chunk of every Pulse with two independent programs (N-version programming), and a [World Proof](../../docs/book/src/concepts/the-fold.md) verifies only if both accept every chunk. Stack A is `sion-exec`, replayed statelessly. This crate is stack B. Its entry point is `verify_chunk`: given a chunk's statement and witness, it executes the chunk reading only witnessed objects, and compares the result with the statement. Its modules follow the sections of the specification, and it is kept independent by rule: it may use the data layouts and a short list of type helpers from `sion-exec`, and it may not call the reference executor's logic.

Part of [Siontology](../../README.md) · [Architecture](../../ARCHITECTURE.md)
· Status: **Built** (N-version proving is built as two independent replay stacks, but the proofs they produce are replay proofs, a stand-in for succinct ones, `impl:replay-proofs`; see [implementation status](../../docs/implementation-status.md)).

## What is in it

About 10,840 lines of Rust under `src/` at the time of writing (`find crates/sion-spec/src -name '*.rs' -print0 | xargs -0 cat | wc -l`).

| Module | Spec | What it holds |
|---|---|---|
| `context` | §1 | The Pulse context (`SpecContext`) |
| `view` | §2.1 | The witnessed pre-state plus the writes kept so far |
| `journal`, `tx` | §3 | `load`, `try_load`, `store`, `consume`; `create`, `mint`, `draw`, `charge_fee_from_gas`, `finalize_inputs` |
| `prepare`, `outcome` | §4.1, §4.2 | Preparation (failures reject), then success, failure with fee, or rejection |
| `authorize` | §4.3 | Fee floor, authorizing policy and check, written again here |
| `ops` | §5 | One function per transaction kind: `coins`, `knowledge`, `assurance`, `charter`, `bounded`, `market`, `stream`, `validators`, `memory`, `task`, `delegation`, `storage`, `index` |
| `deferred` | §6.3 | The deferred phase: items, forced intents, due events, and the end of an epoch (`epoch`, `forced`) |
| `commit` | §7 | Effects leaves, accumulators and steps |
| `chunk` | §8 | `execute_chunk` and `verify_chunk` |
| `error` | §3.4, §8 | `SpecError` (a chunk is invalid) and `OpError` (one operation stops) |

## Main types and functions

- [`verify_chunk`](src/chunk.rs): checks one `ChunkTrace` exactly as §8 describes, executing it with this crate's own implementation of §3 to §6; returns a `ChunkReport` or the first check that failed.
- [`execute_chunk`](src/chunk.rs): runs the checks on the witness and executes the chunk without comparing the results with the statement; useful to diagnose a rejected chunk.
- [`ChunkExecution`](src/chunk.rs) and [`ChunkReport`](src/chunk.rs): what execution produced, and what a verified chunk proved.
- [`SpecContext`](src/context.rs): the Pulse number, beacon and chain configuration a chunk executes under.
- [`SpecError`](src/error.rs): why a chunk is rejected, by check number of §8.
- [`OpError`](src/error.rs): an error of one operation (§3.4). During preparation it rejects the transaction, during apply it fails it; only the outcome class is committed, not the message.
- [`commit`](src/commit.rs): the commitments a statement carries, written from §7.

## How the two stacks are compared

The rule is in §0 of the specification. Stack B may use the data layouts (the Rust type definitions in `sion-exec` are normative) and a short list of type helpers. It may not call `sion_exec::execute`, the pulse logic, `stateless`, `wallet`, `sim`, `genesis`, the reference's authorization or the other helpers §0 names.

- **A lint.** `scripts/check-independence.sh` searches this crate's library sources (comments and unit-test modules excluded) for the forbidden paths and calls. CI runs it on every push. It is a lint, not a proof: a function copied under another name would pass it.
- **Differential tests.** `tests/differential.rs` runs workloads on the reference executor (`sion_exec::sim::Chain`), which records the honest chunk traces of every Pulse. Stack B must accept every one, and must reject traces whose statements, witnesses, transactions or deferred items were tampered with. When B rejects an honest chunk, the failure message diffs B's execution against the effects and writes A recorded, transaction by transaction. A disagreement means one of the two is wrong.
- **In the World Proof.** `sion-prover` wraps this crate as `SpecStack` next to `ReplayStack` (stack A), and a World Proof verifies only if both accept every leaf.

What the stacks share is common-mode (`impl:shared-primitives`): hashing and Merkle structures (`sion-core`), signatures and validator identities (`sion-crypto`), the Assurance Algebra, clearing certificates and committee sampling. A bug in any of these would be accepted by both. The tests found two defects in the reference executor and several gaps in the specification, fixed in specification v1.1 (see [Keeping proof stack B independent](../../docs/book/src/architecture/independence.md)).

## How it fits

Depends on: `sion-core`, `sion-crypto`, `sion-assurance`, `sion-clearing`, `sion-consensus` and `sion-exec` (types and permitted helpers only), with `borsh` and `thiserror` outside the workspace. Used by: `sion-prover` and `sion-cli`.

It verifies chunks; it does not produce traces, build witnesses, fold proofs or run a chain. A change to what a transaction or a Pulse does follows the order in `CONTRIBUTING.md`: the specification first, then `sion-exec`, then this crate from the specification text alone.

## Tests

- Unit tests under `src/`: 65 `#[test]` functions (three `proptest` blocks). Some compare helpers with the reference's on purpose, for example the fee floor and the batch signing context.
- `tests/differential.rs`: 88 tests and four `proptest` blocks. They cover every transaction kind in one-per-chunk and many-per-chunk layouts, random workloads, boundary probes (amounts, windows and deadlines near `u64::MAX`, maximal batches, deep formulas) and tampered traces. Failures from the property tests are kept in `tests/differential.proptest-regressions`.
- Counts come from `grep -c "#\[test\]"`: 153 in all. No test is `#[ignore]`d.

Run them with `cargo test -p sion-spec`, and the lint with `./scripts/check-independence.sh`.

## Read more

- [The state transition specification](../../docs/spec/state-transition.md) and [its overview](../../docs/book/src/specification.md)
- [Keeping proof stack B independent](../../docs/book/src/architecture/independence.md)
- [The Fold and World Proofs](../../docs/book/src/concepts/the-fold.md)
- [The prover](../../docs/book/src/architecture/prover.md)
- [Trust model](../../docs/book/src/security/trust-model.md)
