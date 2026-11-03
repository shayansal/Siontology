# sion-memory

Verifiable memory for AI agents and robots, kept off the ledger. A memory is a single-writer, append-only log of atoms. An atom is a statement (a claim, a revision, a conflict, an erasure or a reference to content) with the time it held, the time it was captured, an optional box in a named frame, an integer confidence, provenance, typed links, a policy and a payload that may be public, sealed or a commitment to a blob. The ledger (`sion-exec`) keeps only each log's head and moves it forward along an RFC 9162 consistency proof, so a log cannot fork, and the Pulse of the commit is the transaction time of the atoms it adds. Two ideas matter for using the crate: the 4D index is canonical (a deterministic function of the log prefix, so anyone can rebuild it and a writer who commits another root can be caught), and belief is computed, never stored (a fold over the revisions that trusted revisers wrote).

Part of [Siontology](../../README.md) · [Architecture](../../ARCHITECTURE.md)
· Status: **Built** (the status file lists this library as built, with named simplifications: the ledger stores the index root unchecked and does not act on omission proofs, `impl:memory-index`; see [implementation status](../../docs/implementation-status.md)).

## What is in it

About 9,160 lines of Rust under `src/` at the time of writing (`find crates/sion-memory/src -name '*.rs' -print0 | xargs -0 cat | wc -l`).

| Module | What it holds |
|---|---|
| `atom` | `MemoryAtom`, its builder, kinds and classes, statements and values, payloads, links, policy, validation rules and canonical encoding |
| `geom` | Integer space (millimetres) and time (nanoseconds): `Box3`, `Interval`, `TimeRange` |
| `log` | `WriterLog`, `Head`, the ledger's commit history (`known_at`), and `LogStore`, a simple append-only on-disk store |
| `index` | The canonical 4D index, specified in the module's own documentation: frames, a time-bucketed forest of Merkle R-trees, a long-lived R-tree, a subject map and a link map |
| `index::range`, `index::keyed`, `index::omission` | Range proofs, subject and link proofs, and fraud proofs that an index leaves an atom out |
| `belief` | Belief as a fold over revision and conflict atoms, with a proof a client can check from heads |
| `seal` | Epoch keys, per-atom keys, sealed payloads (ChaCha20-Poly1305), key wrapping to members with ML-KEM-768, and key destruction |
| `cell` | Shared world cells: a first building block for curated memory of a named frame, with digests and omission proofs |
| `rmp` | The Reality Memory Protocol operations over one writer's log (`Memory`) |
| `error` | `MemoryError` |

## Main types and functions

- [`MemoryAtom`](src/atom.rs) and its builder: the unit of memory, addressed by `H("sion/mem/atom", borsh(atom))`. Everything hashed, compared or proved is an integer.
- [`WriterLog`](src/log.rs): one writer's atoms with their Merkle tree; `head(size)` gives the `Head` (size, log root, index root).
- [`LogStore`](src/log.rs): the log and its key ring on disk; a torn final record is cut off on opening, and any other record that does not check refuses to open.
- [`MemoryIndex`](src/index/mod.rs) and [`index_root`](src/index/mod.rs): the index of a log prefix and its root.
- [`prove_range`, `verify_range`](src/index/range.rs): everything one writer had recorded in a box during a time range, as of a head, complete, including that nothing was recorded.
- [`prove_subject`, `prove_links`](src/index/keyed.rs) with `verify_subject` and `verify_links`: every claim about a subject, or every revision or link of an atom.
- [`prove_omission`, `verify_omission`](src/index/omission.rs): convict an index that leaves an atom out.
- [`belief_at`, `prove_belief`, `verify_belief`](src/belief.rs): the fold, and a bundle of link-map proofs that lets a client check it without the logs.
- [`KeyRing`](src/seal.rs): the epoch keys a writer or member holds, and which are destroyed; destroying one makes the payloads sealed under it unreadable.
- [`Memory`](src/rmp.rs): REMEMBER, RECALL, VERIFY, REVISE, FORGET, SHARE and AUDIT over one writer's log, in memory or on disk, with JSON-ready request and response types for the MCP tools.
- [`CuratedCell`, `CellDigest`](src/cell.rs): a curator's digest of the atoms tagged to a cell, and proofs that it omits one.

## How it fits

Depends on: `sion-core` and `sion-crypto` (ML-KEM-768), with `borsh`, `serde`, `thiserror`, `chacha20poly1305`, `rand_core` and `zeroize` outside the workspace. Used by: `sion-cli` (`sion memory …`, the MCP tools `sion_memory_*`, and the `robots` demo).

The crate does not depend on `sion-exec`. The ledger side, the `OpenMemory` and `CommitMemory` transactions and the head they keep, is in `sion-exec` (`memory`) and in `sion-spec`; this crate defines the log, the index and the proofs a head commits to. Completeness is relative to the log: a proof shows what was recorded, never what happened. Erasure holds only if every member who holds a wrapped epoch key destroys it too. Per the status file, the index is rebuilt for every head and logs are held in memory. Not built: compact fraud proofs for other malformed indexes, selective disclosure of an atom's fields, aggregators that commit many logs in one claim, curated world memory beyond cell digests, and storage with proofs.

Measured over 10⁶ atoms on one core (from the status file; `examples/proof_sizes.rs` reproduces it): a 50 × 50 × 10 m, one-hour query returns 9 atoms with a 36 KiB proof verified in 0.2 ms, and the index builds in 0.78 s.

## Tests

- Unit tests under `src/`: 78 `#[test]` functions, nine of them in `proptest` blocks with fixed seeds.
- `tests/spec_reference.rs` (3 tests): a second implementation of the index root, written from the specification in `sion_memory::index`'s documentation alone, using raw bytes and `sion-core`'s primitives and none of this crate's index code. It must agree with `index_root` on edge cases, on a large log, and on random logs (a property test).
- `tests/warehouse.rs` (2 tests): robots share a warehouse end to end. Two robots record what they see, a curator records and decides a conflict, consumers with different policies fold different beliefs, a curator's cell digest is convicted of leaving an atom out, and a sealed camera frame is forgotten. Everything is checked from heads alone.
- Counts come from `grep -c "#\[test\]"`: 83 in all. No test is `#[ignore]`d.

Run them with `cargo test -p sion-memory`.

## Read more

- [Memory for agents and robots](../../docs/book/src/concepts/memory.md) and the [memory guide](../../docs/book/src/guides/memory.md)
- The design note [Billions of agents](../../docs/book/src/research/agent-scale.md), section 4
- The index specification is the module documentation of [`src/index/mod.rs`](src/index/mod.rs)
- [Specification](../../docs/book/src/specification.md), §5.40 and §5.41 for the ledger side
