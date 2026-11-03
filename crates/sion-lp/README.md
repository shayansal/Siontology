# sion-lp

Exact linear programming with certificates that anyone can check. It is the
arithmetic core of [Verifiable Clearing](../../docs/book/src/concepts/verifiable-clearing.md):
a market clears by solving a linear program, and the solver must hand the
ledger a primal-dual pair showing that no better allocation exists. Two ideas
matter to a user. The ledger never solves; it runs `verify_certificate`, which
checks five conditions in time linear in the number of nonzero coefficients
(`Ax ≤ b`, `x ≥ 0`, `Aᵀy ≥ c`, `y ≥ 0`, `cᵀx ≥ bᵀy − ε`), and by weak duality
that is enough. And all values are exact rationals with one canonical Borsh
encoding, so there is no floating point anywhere in the verification path.

Part of [Siontology](../../README.md) · [Architecture](../../ARCHITECTURE.md)
· Status: **Built** (exact arithmetic and the linear-time certificate check
are implemented with tests, as part of Verifiable Clearing; see
[implementation status](../../docs/implementation-status.md)).

## What is in it

| Module | What it holds |
|---|---|
| `rational` | `Q`, a reduced rational over big integers with a single Borsh encoding, and `ParseQError` |
| `program` | `LinearProgram`: `maximize cᵀx subject to Ax ≤ b, x ≥ 0`, stored as sparse rows; `LpError` |
| `certificate` | `Certificate` (a primal `x` and a dual `y`), `verify_certificate`, `CertificateReport`, `CertError`, `is_integral` |
| `simplex` | `solve`: an exact two-phase simplex with Bland's rule; `LpOutcome` and `LpSolution` |

## Main types and functions

- [`Q`](src/rational.rs): an exact rational, always in lowest terms with a positive denominator, so a certificate hashes the same on every node.
- [`LinearProgram`](src/program.rs): build a program with `new`, `set_objective` and `add_le`; `validate` checks its shape.
- [`Certificate`](src/certificate.rs): the claimed optimum `x` and its dual `y`. For market programs the duals are the clearing prices.
- [`verify_certificate`](src/certificate.rs): check a certificate against a program and a tolerance `ε`; it never solves anything. With `ε = 0` an accepted `x` is exactly optimal.
- [`CertificateReport`](src/certificate.rs): the primal objective, the dual objective and the gap that verification established.
- [`CertError`](src/certificate.rs): why a certificate was rejected, one variant per failed condition, including a value larger than `MAX_VALUE_BITS` (512), so that verification cost does not depend on how large a submitter makes its numbers.
- [`is_integral`](src/certificate.rs): whether a solution has only whole-number values.
- [`solve`](src/simplex.rs): the reference solver; returns `LpOutcome::Optimal`, `Infeasible` or `Unbounded`.
- [`LpSolution::certificate`](src/simplex.rs): turn a solver result into a `Certificate`.

The crate documentation has a doc example (`maximize 3x + 2y subject to
x + y ≤ 4, x ≤ 3`) that solves a program and verifies its certificate; see the
top of [`src/lib.rs`](src/lib.rs).

## How it fits

Depends on no workspace crate. Outside dependencies are `num-bigint`,
`num-rational`, `num-traits` and `num-integer` for exact arithmetic, plus
`borsh`, `serde` and `thiserror`. Used by: `sion-clearing` and `sion-exec`.

The simplex solver is a reference, not part of the trust base: whatever it or
any other solver returns is accepted only if the certificate verifies. The
crate knows nothing of markets, orders or prices; `sion-clearing` builds the
programs. Optimality is for the program as given, not for the bids behind it.

## Tests

- Unit tests inside `src/`: 13 `#[test]` functions at the time of writing
  (counted with `grep`) for rational arithmetic and encoding, program
  validation, the simplex (a textbook program, negative bounds that need
  phase one, infeasible and unbounded programs, Beale's cycling example) and
  the certificate checks (an accepted pair, each kind of rejection).
- `tests/solver_properties.rs`: 3 `proptest` property tests on random
  programs. Optimal outcomes verify with zero gap; the simplex agrees with a
  brute-force enumeration of vertices on bounded programs; a tampered
  certificate is rejected.

There are 16 `#[test]` functions in all, and none is `#[ignore]`d.

```text
cargo test -p sion-lp
```

The crate is 1,360 lines of Rust under `src/` at the time of writing.

## Read more

- [Verifiable Clearing](../../docs/book/src/concepts/verifiable-clearing.md)
- [Trust model](../../docs/book/src/security/trust-model.md): both proof stacks share the certificate-checking code (atom `impl:shared-primitives`).
- [Crate map](../../docs/book/src/architecture/crates.md)
