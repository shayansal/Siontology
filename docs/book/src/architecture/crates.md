# Crate map

The repository is one Cargo workspace of seventeen crates. Lower crates know
nothing of higher ones, and every crate depends on `sion-core` rather than
on its neighbours where it can (a rule in `AGENTS.md`). The app's GUI shell,
`apps/sion-app/src-tauri`, is a separate Cargo workspace on top of
`sion-app-core`, so that GUI dependencies stay out of the protocol's.

Each row depends only on rows below it (the table lists the exact
dependencies):

```text
sion-app-core
sion-cli
sion-node
sion-prover
sion-spec        sion-light      sion-net
sion-exec        sion-memory
sion-consensus   sion-clearing   sion-assurance   sion-crypto   sion-da
sion-core        sion-lp         sion-http
```

| Crate | What it holds | Lines | Depends on |
|---|---|---|---|
| `sion-core` | Domain-separated SHA3-256, canonical Borsh encoding, typed identifiers, fixed-point amounts, the RFC 9162 append-only log, the sparse Merkle state tree, seeded randomness for tests | 1,952 | none |
| `sion-crypto` | ML-DSA-44, SLH-DSA-SHA2-128s/128f, ML-KEM-768, dual-origin signatures, account policies, validator identities and signatures, sealed committee envelopes, Shamir sharing | 1,801 | core |
| `sion-assurance` | The Assurance Algebra: atoms, formulas, cost of corruption, decision rule, reservations, challenges, slashing, insurance | 3,780 | core |
| `sion-lp` | Exact rational simplex and linear-time verification of optimality certificates | 1,360 | none |
| `sion-clearing` | Verifiable Clearing: sealed double auctions and assignment markets with certificates, beacon priorities | 1,402 | core, lp |
| `sion-da` | 2D Reed–Solomon over the Goldilocks field, row and column commitments, sampling, reconstruction, fraud proofs | 2,203 | core |
| `sion-consensus` | Linear Lanes (lane validators, batch-signed approvals, certificates), the certified-DAG BFT core, committees and sampling, the beacon, a deterministic network simulator | 4,332 | core |
| `sion-exec` | The state machine: objects, accounts, claims, bonds, charters, agent identities, escrow, markets, payment streams, memory log heads, tasks, the validator set, pooled stake, storage deals, beacon records and forced-inclusion obligations; transactions; Pulse execution with chunk traces and witnesses; stateless replay; wallets | 11,444 | core, crypto, assurance, clearing, lp, consensus |
| `sion-spec` | Proof stack B: an independent implementation of the state transition, written from the specification | 9,774 | core, crypto, assurance, clearing, consensus, exec (types only) |
| `sion-prover` | The `ProofStack` trait, stacks A and B, World Proofs, certificate checks | 1,047 | core, assurance, exec, spec, da, consensus |
| `sion-light` | The light client: chain identity, header certificates, epoch-to-epoch committee following, state and claim proofs, availability sampling | 816 | core, crypto, consensus, exec, da |
| `sion-memory` | Memory for agents and robots, off the ledger: memory atoms with space and two kinds of time, single-writer logs, the canonical 4D index with range, subject, link and omission proofs, belief as a fold, sealing and forgetting | 9,156 | core, crypto |
| `sion-net` | Networking between processes: QUIC with TLS 1.3, X25519MLKEM768 and pinned ML-DSA-44 raw public keys, one stream per message class, bounded queues and quotas | 2,958 | core, crypto |
| `sion-http` | Outbound HTTPS for clients: TLS 1.3 or 1.2 with the hybrid X25519MLKEM768 key exchange offered first (TLS 1.3 only), web PKI roots, a resolver that refuses loopback, private and other non-public addresses, proxy support, bounded reads, timeouts, no redirects | 670 | none |
| `sion-node` | The devnet and `sion node`: validator threads, gateway, prover thread, committees from the state, forced-inclusion bookkeeping, write-ahead log and snapshots, HTTP API, the transport seam | 10,448 | core, crypto, assurance, clearing, consensus, exec, da, light, net, prover |
| `sion-cli` | The `sion` command: devnet, nodes and gateways, wallets, transactions, payment streams (payer book, payee desk), agents, tasks, memory, pooled stake, storage deals and the storage provider, demos, light verification, MCP server, x402 adapter, benchmarks, sentences | 15,177 | everything except lp and net |
| `sion-app-core` | The app's core: the device's key, a light client that verifies the latest Pulse and the device's own receipts, payments, pooled stake and storage provided from the device ([The app](../getting-started/app.md)) | 850 | core, crypto, exec, light, cli |

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
| `sion-exec/tests/storage.rs`, `sion-exec/tests/pool_slashing.rs` | Beacon records, storage rounds and challenges, proportional forfeits; slashing pooled stake and the reporter's share |
| `sion-consensus/tests` | The consensus simulator with adversarial network phases; lanes; observers |
| `sion-clearing/tests`, `sion-da/tests` | Auctions and assignments; encoding, sampling, reconstruction |
| `sion-spec/tests/differential.rs` | Stack A against stack B on random workloads, boundary probes and tampering |
| `sion-node/tests/devnet.rs` | End-to-end devnets with dual post-quantum signatures |
| `sion-app-core/tests/devnet.rs` | The app's core against a devnet over HTTP: pinning on first use, a payment with its receipt proven, pooled stake, a wrong pin, following new epochs without starting over, providing storage and proving its rounds |
| `sion-cli/tests/demos.rs` | The demos (`#[ignore]`d: they take minutes) and an MCP session |
| `sion-cli/tests/streams.rs` | An x402 server's stream desk on a devnet: refusals, recovery, top-ups and the close-out |

`cargo test --workspace` runs the fast suite, as CI does on every push.
`cargo test --workspace -- --include-ignored` adds the demo runs, as CI
does every night.
