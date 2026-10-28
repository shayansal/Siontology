# Crate map

The repository is one Cargo workspace of thirteen crates. Lower crates know
nothing of higher ones, and every crate depends on `sion-core` rather than
on its neighbours where it can (a rule in `AGENTS.md`).

Each row depends only on rows below it (the table lists the exact
dependencies):

```text
sion-cli
sion-node
sion-prover
sion-spec        sion-light
sion-exec
sion-consensus   sion-clearing   sion-assurance   sion-crypto   sion-da
sion-core        sion-lp
```

| Crate | What it holds | Lines | Depends on |
|---|---|---|---|
| `sion-core` | Domain-separated SHA3-256, canonical Borsh encoding, typed identifiers, fixed-point amounts, the RFC 9162 append-only log, the sparse Merkle state tree, seeded randomness for tests | 1,952 | none |
| `sion-crypto` | ML-DSA-44, SLH-DSA-SHA2-128s/128f, ML-KEM-768, dual-origin signatures, account policies, validator identities and signatures, sealed committee envelopes, Shamir sharing | 1,801 | core |
| `sion-assurance` | The Assurance Algebra: atoms, formulas, cost of corruption, decision rule, reservations, challenges, slashing, insurance | 3,780 | core |
| `sion-lp` | Exact rational simplex and linear-time verification of optimality certificates | 1,360 | none |
| `sion-clearing` | Verifiable Clearing: sealed double auctions and assignment markets with certificates, beacon priorities | 1,402 | core, lp |
| `sion-da` | 2D Reed–Solomon over the Goldilocks field, row and column commitments, sampling, reconstruction, fraud proofs | 2,203 | core |
| `sion-consensus` | Linear Lanes (lane validators, batch-signed approvals, certificates), the certified-DAG BFT core, committees and sampling, the beacon, a deterministic network simulator | 4,318 | core |
| `sion-exec` | The state machine: objects, accounts, claims, bonds, charters, escrow, markets, payment streams, the validator set and forced-inclusion obligations; transactions; Pulse execution with chunk traces and witnesses; stateless replay; wallets | 8,138 | core, crypto, assurance, clearing, lp, consensus |
| `sion-spec` | Proof stack B: an independent implementation of the state transition, written from the specification | 6,065 | core, crypto, assurance, clearing, consensus, exec (types only) |
| `sion-prover` | The `ProofStack` trait, stacks A and B, World Proofs, certificate checks | 1,047 | core, assurance, exec, spec, da, consensus |
| `sion-light` | The light client: chain identity, header certificates, epoch-to-epoch committee following, state and claim proofs, availability sampling | 749 | core, crypto, consensus, exec, da |
| `sion-node` | The devnet: validator threads, gateway, prover thread, committees from the state, forced-inclusion bookkeeping, write-ahead log and snapshots, HTTP API | 6,112 | everything except lp and spec |
| `sion-cli` | The `sion` command: devnet, wallets, transactions, payment streams (payer book, payee desk), demos, light verification, MCP server, x402 adapter, benchmarks, sentences | 8,855 | everything except lp |

Lines are lines of Rust under each crate's `src/`, unit tests included, at
the time of writing.

## Where to look

| To understand… | Read |
|---|---|
| How a transaction executes | `sion_exec::execute` (one transaction), `sion_exec::pulse` (a Pulse and its phases) |
| What a validator does | `sion_node::validator` ([The validator thread](validator.md)) |
| How committees are derived | `sion_exec::validators`, `sion_node::committees` |
| How a light client decides | `sion_light::{chain, epochs, header, proofs}`, `sion_cli::verify` |
| The rules both proof stacks implement | `docs/spec/state-transition.md` ([Specification](../specification.md)) |
| What the CLI prints | `sion_cli::describe` |

## Tests

| Where | What |
|---|---|
| Unit tests in every crate | Encodings, hashes, Merkle structures, signatures, sampling, the beacon |
| `sion-assurance`, `sion-lp`, `sion-exec/tests/properties.rs` | Property tests (`proptest`) of the algebra, the certificates and the state machine |
| `sion-exec/tests/scenarios.rs`, `sion-exec/tests/validators.rs` | Scenario tests for every transaction kind, staking, forced inclusion and payment streams |
| `sion-consensus/tests` | The consensus simulator with adversarial network phases; lanes; observers |
| `sion-clearing/tests`, `sion-da/tests` | Auctions and assignments; encoding, sampling, reconstruction |
| `sion-spec/tests/differential.rs` | Stack A against stack B on random workloads, boundary probes and tampering |
| `sion-node/tests/devnet.rs` | End-to-end devnets with dual post-quantum signatures |
| `sion-cli/tests/demos.rs` | The demos |
| `sion-cli/tests/streams.rs` | An x402 server's stream desk on a devnet: refusals, recovery, top-ups and the close-out |

Run them all with `cargo test --workspace`.
