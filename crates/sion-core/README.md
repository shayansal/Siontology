# sion-core

The base crate of the Siontology workspace: domain-separated hashing, the
canonical encoding every hashed value uses, typed identifiers, fixed-point
money, and the two Merkle structures behind the two-sorted state. It depends
on no other workspace crate, and every other crate (and every light client)
builds on it. Two ideas matter to a user. Every hash is
`SHA3-256(len(domain) || domain || data)`, so a digest made for one purpose
cannot be replayed for another. And every consensus-critical value has exactly
one Borsh encoding, so every node hashes it the same way.

Part of [Siontology](../../README.md) · [Architecture](../../ARCHITECTURE.md)
· Status: **Built** (the foundations are implemented as the architecture
describes, with tests; see
[implementation status](../../docs/implementation-status.md)).

## What is in it

| Module | What it holds |
|---|---|
| `hash` | Domain-separated SHA3-256, the `Hash32` digest and the incremental `Hasher` |
| `encode` | Canonical Borsh `encode` and `decode`, and the `Canonical` trait |
| `ids` | Newtype identifiers (accounts, claims, assets, shards, validators) and counters (`Pulse`, `Epoch`, `Round`, `Version`) |
| `amount` | `Amount`, fixed-point money in micro-units, and `Cost`, which has an explicit infinity |
| `merkle` | RFC 9162 Merkle tree hash with inclusion and consistency proofs, the `AppendLog` and its `Frontier` |
| `smt` | The sparse Merkle tree for the state root, with inclusion and non-inclusion proofs |
| `rng` | Seeded deterministic randomness for simulations and tests |
| `error` | `CoreError`, the crate's structured error |

## Main types and functions

- [`hash_domain`, `hash_parts`](src/hash.rs): hash bytes under a domain tag.
- [`Hash32`](src/hash.rs): the 32-byte digest, with hex conversion and bit access.
- [`encode`, `decode`](src/encode.rs): the one canonical byte form of a value.
- [`AccountId`, `ClaimId`, `AtomId`, `ObjectId` and the other identifiers](src/ids.rs): digest-based newtypes, so the compiler rejects a claim ID where an account ID is expected.
- [`Amount`](src/amount.rs): micro-unit money (six decimal places) with checked arithmetic.
- [`Cost`](src/amount.rs): a finite `Amount` or `Infinite`; the Assurance Algebra uses it for the cost of breaking a trusted assumption.
- [`AppendLog`](src/merkle.rs): the grow-only knowledge log; it gives roots at any earlier size and inclusion and consistency proofs.
- [`verify_inclusion`, `verify_consistency`](src/merkle.rs): check a proof against a root. A consistency proof is how "the knowledge root only grew" is shown.
- [`SparseMerkleTree`](src/smt.rs): the state tree. A subtree with one entry is stored as that entry's leaf, so paths stay about `log2(n)` long.
- [`SmtProof`, `PartialSmt`](src/smt.rs): a proof of a key's value or absence, and a partial tree rebuilt from such proofs for stateless checking.
- [`seeded`, `labeled`](src/rng.rs): reproducible randomness for tests and simulations.

## How it fits

Depends on no workspace crate. Outside dependencies are `sha3` for SHA3-256,
`borsh` for the encoding, and `serde`, `hex`, `thiserror`, `rand_core` and
`rand_chacha`. Used by every other crate in the workspace.

The crate holds primitives only. It knows nothing of transactions, claims or
accounts' policies; entries in the sparse tree are never deleted, and what a
consumed object looks like is decided by `sion-exec`.

## Tests

The tests are unit tests inside `src/`: 26 `#[test]` functions at the time of
writing (counted with `grep`), covering hashing and domain separation,
encodings, amounts, identifiers, and the Merkle log and sparse tree. Two of
them are property tests with `proptest` (random consistency proofs in
`merkle.rs`, and one in `smt.rs`). There is no `tests/` directory and no
`#[ignore]`d test.

```text
cargo test -p sion-core
```

The crate is 1,952 lines of Rust under `src/` at the time of writing.

## Read more

- [Two-sorted state](../../docs/book/src/concepts/two-sorted-state.md): why knowledge is a grow-only log and resources are tombstoned objects.
- [Claims](../../docs/book/src/concepts/claims.md): what is appended to the knowledge log.
- [The Fold](../../docs/book/src/concepts/the-fold.md): where the consistency proof is used.
- [Crate map](../../docs/book/src/architecture/crates.md)
