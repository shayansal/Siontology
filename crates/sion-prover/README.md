# sion-prover

The Fold: validity proofs for every Pulse. Execution of a Pulse is cut into chunks, and each chunk carries a statement (these steps take the world from commitment `before` to commitment `after`) and a witness. This crate proves each statement with two proof stacks and folds the leaf proofs into one World Proof per Pulse. Checking a World Proof means checking that the leaves chain from the Pulse's start to its end, and that every stack's proof of every leaf verifies, so a false statement needs a soundness bug in both stacks at once. The stacks here are replay stacks, a stand-in for succinct proofs: a leaf proof is the witness itself, and verifying it means re-executing the chunk statelessly and comparing every root and accumulator. The proofs are sound but not succinct.

Part of [Siontology](../../README.md) · [Architecture](../../ARCHITECTURE.md)
· Status: **Simulated** (World Proofs are checked by replaying the Pulse in two independent stacks, a stand-in for succinct proofs, trust atom `impl:replay-proofs`; see [implementation status](../../docs/implementation-status.md)).

## What is in it

About 1,050 lines of Rust under `src/` at the time of writing (`find crates/sion-prover/src -name '*.rs' -print0 | xargs -0 cat | wc -l`).

| Module | What it holds |
|---|---|
| `stack` | The `ProofStack` trait and `LeafProof` |
| `replay` | Stack A (`ReplayStack`): stateless re-execution with the reference executor, `sion_exec::replay_chunk` |
| `spec` | Stack B (`SpecStack`): stateless re-execution with the independent implementation, `sion_spec::verify_chunk` |
| `world` | Aggregation into a `WorldProof`, its verification (full and sampled), and the check of lane certificates |
| `error` | `ProveError` |

## Main types and functions

- [`ProofStack`](src/stack.rs): a proof system for chunk statements, with `prove`, `verify`, a `name` and a `simulated` flag. A succinct stack would implement the same trait.
- [`LeafProof`](src/stack.rs): one stack's proof of one chunk statement.
- [`ReplayStack`](src/replay.rs) and [`SpecStack`](src/spec.rs): stacks A and B. Both are labelled `simulated: true`, and both use the same proof body (transactions or deferred items, witness, expected effects).
- [`prove_pulse`](src/world.rs): proves every chunk with every given stack and folds the statements into a `WorldProof`.
- [`WorldProof`](src/world.rs): the statements, one leaf proof per stack for each, the receipts, the Pulse's published data, its availability root, the lane certificates, and the validator set the Pulse began with.
- [`PulseData`](src/world.rs): the published data, which is the transaction bodies in execution order and the forced intents posted in the Pulse.
- [`verify_world_proof`](src/world.rs): checks the structure, the binding of the data, and every leaf under every stack given.
- [`verify_world_proof_sampled`](src/world.rs): the same, but re-executes only a seeded sample of leaves (always the deferred phase); an invalid leaf is found with probability about samples over leaves.
- [`verify_certificates`](src/world.rs): checks that each executed transaction carried a lane certificate from its shard's committee, taken from the proven validator set.
- [`WorldReport`](src/world.rs): what a verification established, including `simulated`.

## What a World Proof binds

- **Execution.** The statements chain from `before` to `after`, the deferred phase consumes exactly the effects the transactions deferred, and every leaf verifies under every stack.
- **Data.** The published data is exactly what the statements executed, and it encodes (2D Reed–Solomon, `sion-da`) to the data root a header certifies.
- **Certificates.** Every executed transaction locked its owned inputs with a quorum of its shard's committee. The committees come from the validator set the proof carries, checked against the Pulse's starting state root.

Folding is a Merkle tree over the leaf statements, not recursive proof composition, and a World Proof grows with the Pulse's witnesses. Matching the proof against a certified Pulse header (same commitments, receipts root, data root and beacon) is done by the callers, `sion-node`'s prover thread and `sion verify`. The forced-inclusion summary in a header is not recomputed by the proof. Not built: a proving market, payments to provers, recursion, and more than one prover.

## How it fits

Depends on: `sion-core`, `sion-assurance`, `sion-exec`, `sion-spec`, `sion-da` and `sion-consensus`, with `borsh`, `serde` and `thiserror` outside the workspace. Used by: `sion-node` (the prover thread) and `sion-cli` (`sion verify`).

It proves and verifies; it does not execute Pulses, which `sion-exec` does when it traces them, and it does not follow committees, which `sion-light` does. The two stacks share hashing, Merkle, signature, Assurance Algebra and clearing-certificate code (`impl:shared-primitives`).

## Tests

- 5 unit tests in `src/world.rs`, counted with `grep -c "#\[test\]"`: World Proofs verify and bind receipts and data; tampering is detected; the deferred phase posts exactly the published intents; forged Pulse numbers fail cleanly; both stacks must accept every leaf.
- There is no `tests/` directory. The stacks are exercised from other crates: `sion-spec/tests/differential.rs` runs stack B against recorded chunk traces, and `sion-node/tests/devnet.rs` checks on an end-to-end devnet that the prover folded a Pulse into a World Proof without error and that the proof is marked `simulated`.
- No test is `#[ignore]`d.

Run the unit tests with `cargo test -p sion-prover`.

## Read more

- [The Fold and World Proofs](../../docs/book/src/concepts/the-fold.md)
- [The prover](../../docs/book/src/architecture/prover.md), the devnet's prover thread
- [The World Proof format](../../docs/book/src/reference/world-proof.md)
- [Keeping proof stack B independent](../../docs/book/src/architecture/independence.md)
- [Two levels of finality](../../docs/book/src/concepts/finality.md)
- [Succinct proofs](../../docs/book/src/research/succinct-proofs.md)
