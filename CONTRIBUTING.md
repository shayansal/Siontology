# Contributing to Siontology

Thank you for helping. Siontology is a reference implementation: every
change has to keep it honest about what it does, and keep its two proof
stacks independent. This file says how to build, test and propose changes.

Security problems are not reported here: see [SECURITY.md](SECURITY.md).
Everyone taking part follows the [Code of Conduct](CODE_OF_CONDUCT.md).

## Build

You need Rust 1.80 or later (the workspace's `rust-version`).

```bash
cargo build --release
./target/release/sion --version
./target/release/sion demo all --local
```

Post-quantum signing is slow without optimization, so the workspace
profiles optimize dependencies even in debug builds.

## Check before every commit

The same commands CI runs:

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
./scripts/check-independence.sh
```

For documentation changes, also build the book (CI's site build uses
mdBook 0.4.40):

```bash
mdbook build docs/book
```

The test suite is not small: it includes property tests, a consensus
simulator and end-to-end devnets with dual post-quantum signatures. On a
small machine, run one crate's tests with `cargo test -p <crate>` while you
work, and the whole suite before you push.

`cargo test --workspace` is the fast suite, which CI runs on every push and
pull request (about six minutes on two cores, compilation included). The
demo runs take minutes more, so they are marked `#[ignore]` and CI runs
them every night with the quick benchmarks (`.github/workflows/nightly.yml`).
If you change the node, the CLI or a demo, run them before you push:

```bash
cargo test --workspace -- --include-ignored
```

## Rules

[`AGENTS.md`](AGENTS.md) holds the architecture rules for anyone, human or
AI, changing the code. In short:

1. Every consensus-critical value has one canonical encoding (Borsh) and is
   hashed with a domain tag.
2. Consensus-critical maps are ordered (`BTreeMap`, `BTreeSet`).
3. No floating point in consensus-critical logic.
4. Nothing is deleted from state.
5. Every trust assumption is named. A shortcut that the system relies on
   without proof becomes an `impl:` atom in
   [`docs/implementation-status.md`](docs/implementation-status.md).
6. Simulation-only shortcuts are labelled in the types and the docs.
7. No unsafe Rust.

Documentation, including the book in `docs/book`, never claims more than
`docs/implementation-status.md`. If you build something, update that file
in the same change; if you simplify something, name the simplification
there.

## Changing the state transition: specification first

Two implementations execute every Pulse: the reference executor
(`crates/sion-exec`, proof stack A) and an independent implementation
written only from the specification (`crates/sion-spec`, proof stack B). A
World Proof verifies only if both accept every chunk. To change what a
transaction or a Pulse does:

1. Change [`docs/spec/state-transition.md`](docs/spec/state-transition.md),
   raise its version, and add an entry to its **Changes** section that says
   what changed and why.
2. Change the reference executor.
3. Change stack B **from the specification text alone**. Do not copy code
   from `sion-exec`; §0 of the specification lists what stack B may and may
   not call, and `./scripts/check-independence.sh` checks it.
4. Extend the differential tests (`crates/sion-spec/tests/differential.rs`)
   until both stacks agree on the new behaviour and its edge cases.

A pull request that changes one stack without the specification and the
other stack will not be merged.

## Proposing a change

- Open an issue first for anything larger than a fix, so the approach can
  be discussed before you write it.
- Keep pull requests focused: one change, with its tests and its
  documentation.
- Describe what changed and why, and how you tested it. The pull request
  template has a checklist.
- CI must pass. A maintainer listed in `.github/CODEOWNERS` reviews every
  change.

## Sign your commits (DCO)

Contributions are accepted under the
[Developer Certificate of Origin 1.1](https://developercertificate.org/):
by signing off, you certify that you wrote the change or have the right to
submit it under the project's licence. Add a `Signed-off-by` line to every
commit with:

```bash
git commit -s
```

Commits without a sign-off cannot be merged. Use your real name.

## Licence

Siontology is licensed under either of the Apache License, Version 2.0 or
the MIT license, at your option (`LICENSE-APACHE`, `LICENSE-MIT`). Unless
you state otherwise, any contribution you submit is licensed the same way,
without additional terms. The licences do not grant rights to the
Siontology name or logo; see [TRADEMARKS.md](TRADEMARKS.md).
