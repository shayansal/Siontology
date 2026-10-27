# AGENTS.md

Working rules for people and agents building Fold, the reference
implementation of the neo-blockchain architecture.

## Architecture rules (non-negotiable)

1. Every consensus-critical value has one canonical encoding (Borsh) and is
   hashed with a domain tag (`fold_core::hash`). Never hash ad hoc bytes.
2. Consensus-critical maps are `BTreeMap`/`BTreeSet`. No `HashMap` iteration
   order may influence a hash, a root, an ordering or a decision.
3. No floating point in consensus-critical logic. Money is `Amount`
   (micro-units, `u128`); exact optimization uses rationals. Floats are
   allowed only for reporting (probabilities, statistics).
4. Nothing is deleted from state. Consumed objects are tombstoned; claims
   are appended. History stays auditable and proofs stay stateless.
5. Every trust assumption is named. Anything the system relies on that is not
   proven is an atom in some trust formula, including this implementation's
   simplifications (see `docs/implementation-status.md`).
6. Simulation-only shortcuts (fast test signatures, replay proofs) are
   labeled at the type level and in docs; they never silently replace real
   cryptography in production code paths.
7. Unsafe Rust is forbidden.

## Coding style

- Newtype IDs from `fold_core::ids`; explicit domain types; structured errors
  with `thiserror`.
- Deterministic tests: seed every RNG (`fold_core::rng`).
- Property tests (`proptest`) for every algebraic or adversarial invariant.
- Public functions have doc comments and tests.
- Keep crates focused; depend on `fold-core` rather than on each other where
  possible.

## Commands

Run before every commit:

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Post-quantum signing is slow without optimization; the workspace profiles
optimize dependencies even in debug builds.
