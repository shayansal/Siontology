# sion-da

The data-availability layer of the Fold architecture. A validity proof shows
that a transition was correct; it cannot show that the transition's data was
published. This crate makes withholding detectable by everyone. A Pulse's data
is laid out as a `k × k` square of Goldilocks field symbols, extended with a
two-dimensional Reed–Solomon code to `2k × 2k`, and committed with a SHA3
Merkle root for every row and column. Two ideas matter to a user. To make the
data unrecoverable a producer must withhold at least `(k + 1)²` shares, over a
quarter of the square, so a light client that gets all of `s` random samples
back is fooled with probability below `(3/4)^s`. And hash commitments bind the
shares but not the encoding: the World Proof is what shows the square was
encoded correctly, and the fraud proofs here are the fallback.

Part of [Siontology](../../README.md) · [Architecture](../../ARCHITECTURE.md)
· Status: **Built** (the code, sampling, reconstruction and fraud proofs are
implemented with tests; retention and archival, which are not in this crate,
are **Simplified**; see
[implementation status](../../docs/implementation-status.md)).

## What is in it

| Module | What it holds |
|---|---|
| `field` | `Fp`, the Goldilocks field `p = 2^64 − 2^32 + 1`, with number-theoretic transforms (`ntt`, `intt`) and batch inversion |
| `layout` | Mapping a blob onto a `k × k` square: seven bytes per symbol, `choose_k`, `MIN_K` (2) and `MAX_K` (2048) |
| `rs` | Systematic Reed–Solomon coding of one axis: `extend_axis`, `decode_axis`, codeword checks |
| `square` | `ExtendedSquare`, its `DaHeader` (row roots, column roots and the data root), `Share` and share verification |
| `sampling` | `LightSampler`, sample positions and the detection and miss probabilities |
| `reconstruct` | Rebuilding the square, and the blob, from a subset of shares |
| `fraud` | Encoding checks and compact `BadEncodingProof`s |
| `error` | `DaError` and `Axis` |

## Main types and functions

- [`ExtendedSquare`](src/square.rs): `encode` lays out and extends a blob; `header` commits to it; `share` returns one cell with its audit path.
- [`DaHeader`](src/square.rs): the commitment a light client holds: `k`, the `2k` row roots, the `2k` column roots and a data root over all of them.
- [`Share`, `verify_share`](src/square.rs): one cell and its Merkle path, and the check against a header.
- [`choose_k`](src/layout.rs): the smallest supported `k` whose square holds a blob of a given length.
- [`LightSampler`](src/sampling.rs): `new(samples, seed)` then `run(header, fetch)`; returns a `SampleReport` whose outcome is `Available`, `Unavailable` (some samples not returned) or `Invalid` (a returned share failed its proof).
- [`detection_probability`, `undetected_probability`, `undetected_bound`, `crowd_undetected_probability`](src/sampling.rs): the figures behind the `(3/4)^s` bound, for one client and for many.
- [`min_withholding`](src/sampling.rs): the least a producer must withhold to make a square of a given `k` unrecoverable.
- [`reconstruct`, `reconstruct_blob`, `complete`](src/reconstruct.rs): iterative row and column decoding from any sufficient subset of shares, checked against the header.
- [`verify_extended_square`](src/fraud.rs): check that every axis of a full square is a codeword.
- [`BadEncodingProof`, `verify_bad_encoding_proof`](src/fraud.rs): a compact proof that a committed square is badly encoded, either `k + 1` shares of one axis on no polynomial of degree `< k`, or a cell on which a row root and a column root disagree.

The crate documentation has a doc example that encodes a blob and samples it
with 16 samples; see the top of [`src/lib.rs`](src/lib.rs).

## How it fits

Depends on: `sion-core` (hashing and the Merkle tree), plus `borsh`, `serde`,
`thiserror` and `rand_core`. Used by: `sion-prover`, `sion-light`, `sion-node`
and `sion-cli`.

The crate encodes, commits, samples, reconstructs and checks. It does not
store or serve data and it does not decide retention: validators keep a
write-ahead log and serve the last 256 Pulses from memory, with no archival
tier, and that is the part the status file calls simplified. It also does not
prove correct encoding succinctly: in the Fold the World Proof checks that the
published data encodes to the certified data root, and the World Proof is
itself checked by replay today (atom `impl:replay-proofs`). The commitments are
hash-based, with no trusted setup and no pairings. Sampling assumes a client's
requests cannot be linked to it.

## Tests

- Unit tests inside `src/`: 29 `#[test]` functions at the time of writing
  (counted with `grep`) for the field (including 3 `proptest` properties
  on reduction and inverses), NTT against a naive transform, layout round
  trips, Reed–Solomon extension and decoding from any `k` positions, header
  consistency, sampling figures, reconstruction (including that minimal
  withholding is unrecoverable and that wrong values are caught), and fraud
  proofs against honest and tampered squares.
- `tests/da.rs`: 7 tests. Round trips through partial shares for many blob
  sizes, a modified share failing its proof, serialization of headers and
  shares, a producer unable to hide bad encoding from full nodes, two
  `proptest` properties (rows with `k` symbols always recover; random erasure
  never yields wrong data), and `encode_one_megabyte`.

There are 36 `#[test]` functions in all. `encode_one_megabyte` is
`#[ignore]`d because it measures time and prints it; run it with
`cargo test -p sion-da --release -- --ignored --nocapture`.

```text
cargo test -p sion-da
```

The crate is 2,203 lines of Rust under `src/` at the time of writing.

## Read more

- [Data availability](../../docs/book/src/concepts/data-availability.md)
- [The Fold](../../docs/book/src/concepts/the-fold.md): the World Proof and its data-root check.
- [Pulses](../../docs/book/src/concepts/pulses.md)
- [Post-quantum identity](../../docs/book/src/concepts/post-quantum.md): why hash-based commitments.
- [Crate map](../../docs/book/src/architecture/crates.md)
