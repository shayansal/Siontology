# sion-light

The building blocks of the Siontology light client: a client that follows the chain without executing it. It pins one value, the chain identity (a digest of the genesis state, the genesis validators with their stake and keys, and the shard layout), and trusts nothing a node serves beyond what it can check against that. From the identity it fixes epoch 0's committee, then follows the committee from epoch to epoch through the validator set in the state. It checks Pulse headers signed by more than two thirds of a committee's stake, proves objects, absences, claims and receipts against a certified header, and samples a Pulse's data square to detect withholding. The crate holds the checks; the commands that fetch from a node and print results (`sion verify`, `sion receipt`) are in `sion-cli`.

Part of [Siontology](../../README.md) · [Architecture](../../ARCHITECTURE.md)
· Status: **Built** (header certificates, epoch following, state and knowledge proofs and sampling; World Proofs are not checked here but by replaying chunks, which `sion verify` does through `sion-prover`, and that is **Simulated**: `impl:replay-proofs`; see [implementation status](../../docs/implementation-status.md)).

## What is in it

About 820 lines of Rust under `src/` at the time of writing (`find crates/sion-light/src -name '*.rs' -print0 | xargs -0 cat | wc -l`).

| Module | What it holds |
|---|---|
| `chain` | `ChainSpec`: the chain identity a client pins, its check, and the committees it fixes for the first epoch |
| `epochs` | `EpochChain` and `EpochProof`: following the committee from one epoch to the next |
| `header` | `PulseHeader`, `PulseCertificate`, certificate verification, header chains, `ForcedSummary`, and the validators' public keys |
| `proofs` | `ObjectProof`, `ClaimProof` and `ReceiptProof` with their `verify_*` functions |
| `availability` | Data-availability sampling against a certified header |
| `error` | `LightError` |

## Main types and functions

- [`ChainSpec`](src/chain.rs): everything that fixes who may certify the chain. `id()` is the identity a client pins, and `verify(pinned)` checks that the specification hashes to it and that every validator identifier derives from its keys.
- [`EpochChain`](src/epochs.rs): the committees a client has verified, from epoch 0. `extend` adds the next epoch from an `EpochProof`, and `verify_header` checks a certificate in a verified epoch.
- [`EpochProof`](src/epochs.rs): the certificate of an epoch's last Pulse, and the validator set after it, proven against that header.
- [`PulseHeader`](src/header.rs): what a Pulse committed to: chain, epoch, beacon, world commitments before and after, order digest, receipts root, data root and the forced-inclusion summary.
- [`PulseCertificate`](src/header.rs) and [`verify_certificate`](src/header.rs): a header with committee signatures, and the check that they are valid, distinct, from members, and above the quorum. Signatures are dual post-quantum (ML-DSA-44 and SLH-DSA-SHA2-128f, both required).
- [`verify_header_chain`](src/header.rs): headers are contiguous, each starting where the last ended.
- [`ForcedSummary`](src/header.rs): pending and overdue forced intents after a Pulse, with a root over the overdue ones, so censorship shows in certified headers.
- [`verify_object`, `verify_claim`, `verify_receipt`](src/proofs.rs): a sparse-Merkle proof of an object or its absence, an inclusion path in the knowledge log, and an inclusion path in the receipts root, each against a header.
- [`availability::sample`](src/availability.rs): fetches random shares of the Pulse's extended square and checks them against the certified data root. With `s` samples, a minimal withholding goes unnoticed with probability below (3/4)^s.

## How it fits

Depends on: `sion-core`, `sion-crypto`, `sion-consensus` (committees and the `SignatureVerifier` trait), `sion-exec` (the state's types) and `sion-da` (sampling), with `borsh`, `serde` and `thiserror` outside the workspace. Used by: `sion-node` (it builds and serves headers, certificates and proofs in these formats), `sion-cli` (`sion verify`, `sion receipt`) and `sion-app-core`.

It does not fetch anything, hold a store, or execute a chunk. A chain started with `--insecure-fast-signatures` sets `insecure_signatures` in its identity, and `EpochChain::verify_header` then checks the chain and the epoch but no signatures (`impl:insecure-sim-signatures`). The `LightError::World` variant exists, but nothing in this crate returns it. What a light client trusts is set out in the [light-client trust](../../docs/implementation-status.md#light-client-trust) section of the implementation status.

## Tests

- 3 unit tests, counted with `grep -c "#\[test\]"`: a well-formed specification verifies and fixes its committees; substituted keys or another chain are rejected (`src/chain.rs`); forgetting old epochs keeps the latest and those after the mark (`src/epochs.rs`).
- There is no `tests/` directory. The rest is covered from other crates: `sion-node/tests/devnet.rs` runs end-to-end devnets, and `sion-app-core/tests/devnet.rs` runs a light client against a devnet over HTTP (pinning on first use, a wrong pin, following new epochs, a payment with its receipt proven).
- No test is `#[ignore]`d.

Run the unit tests with `cargo test -p sion-light`.

## Read more

- [Light-client verification](../../docs/book/src/guides/light-client.md) and [Verify as a light client](../../docs/book/src/getting-started/light-client.md)
- [Chain identity](../../docs/book/src/reference/chain-identity.md)
- [Two levels of finality](../../docs/book/src/concepts/finality.md)
- [Data availability](../../docs/book/src/concepts/data-availability.md)
- [Committees](../../docs/book/src/concepts/committees.md) and [epochs](../../docs/book/src/concepts/validators/epochs.md)
- [Censorship resistance](../../docs/book/src/concepts/censorship-resistance.md)
- [The Fold and World Proofs](../../docs/book/src/concepts/the-fold.md)
