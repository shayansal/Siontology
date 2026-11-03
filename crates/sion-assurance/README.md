# sion-assurance

The Assurance Algebra: the part of Siontology that lets a consumer put a price
on a claim before relying on it. Every claim carries a monotone Boolean
formula over the assumptions it rests on (its atoms) and over other claims.
From that formula and the consumer's own policy, the crate computes the claim's
cost of corruption: the least an attacker must spend or forfeit to make the
claim false. Two ideas matter to a user. The decision rule accepts a claim only
when `min(δ·CoC_low, CoC(adjudicator)) ≥ κ·V`, where `V` is what the consumer
has at stake. And an acceptance reserves `κ·V` of the claim's backing, pushed
down its formula, so total reliance can never exceed what slashing can repay.

Part of [Siontology](../../README.md) · [Architecture](../../ARCHITECTURE.md)
· Status: **Built** (claims, formulas, cost of corruption, the decision rule,
reservations, bonds, challenges, slashing with compensation and insurance
probabilities are implemented and property-tested; see
[implementation status](../../docs/implementation-status.md)).

## What is in it

| Module | What it holds |
|---|---|
| `atom` | `Atom` and `AtomClass`, and constructors for the standard atoms (SHA3, SHA2, module lattices, proof stacks, the specification, a TEE, a committee threshold) |
| `formula` | `Formula`: `∧`, `∨`, atoms and references to other claims; normalization and evaluation |
| `policy` | `Policy` (which atoms a consumer trusts, distrusts or assumes broken, and its safety factor κ) and `Ratio` |
| `registry` | `Registry`: atoms, claims, bonds, reservations and broken atoms |
| `coc` | `CocInterval` and `coc_interval`, with `exact_coc` for formulas that share atoms |
| `reserve` | `allocate` and `max_reservable`: pushing a reservation of κ·V down a formula, never beyond a bond |
| `decide` | The decision rule: `decide`, `accept_and_reserve` and `release` |
| `challenge` | Challenges, rulings, slashing, falsification of dependent claims and compensation |
| `insurance` | Failure probabilities for independent atoms and a premium |
| `error` | `AssuranceError` |

## Main types and functions

- [`Atom`, `AtomClass`](src/atom.rs): one irreducible assumption and its class (cryptographic, code, specification, hardware, institutional, economic, committee, adjudicator).
- [`Formula`](src/formula.rs): a trust formula; `Formula::and` and `Formula::or` build it, `holds` evaluates it against a set of broken atoms.
- [`Policy`](src/policy.rs): a consumer's view of the world. An atom the policy trusts costs infinity to break; any other costs its bond plus its known attack cost. `Policy::standard` and `Policy::paranoid` are the two presets.
- [`Registry`](src/registry.rs): the state this crate works on; `register_atom`, `post_bond`, `issue_claim`, `advance_to`.
- [`coc_interval`, `CocInterval`](src/coc.rs): `min` over `∧`, `+` over atom-disjoint `∨`, and `max` as a sound lower bound when atoms are shared; the interval says whether it is exact.
- [`exact_coc`](src/coc.rs): an exact solver for formulas with few shared atoms.
- [`decide`, `Decision`](src/decide.rs): applies the rule and returns every intermediate value and the reasons.
- [`accept_and_reserve`, `release`](src/decide.rs): take and give back a reservation.
- [`allocate`, `max_reservable`](src/reserve.rs): spread a reservation over bonded atoms.
- [`Challenge`, `SlashOutcome`, `Compensation`](src/challenge.rs): opening a challenge, a ruling, and what follows when an atom is broken: its bond is slashed into a compensation pool, active claims are re-evaluated in issue order, and relying parties are repaid.
- [`failure_probability`, `premium`](src/insurance.rs): probabilities for independent atoms; these use floating point and are reports, not consensus values.

## How it fits

Depends on: `sion-core`, and on `num-bigint` and `num-traits` for exact
arithmetic. Used by: `sion-exec`, `sion-spec`, `sion-prover`, `sion-node`,
`sion-cli` and `sion-app-core`.

The crate is deterministic state and arithmetic. It does not verify
proofs, sign anything or order transactions: the ledger (`sion-exec`) holds the
`Registry` and calls these functions. Each consumer evaluates its own `Policy`
locally; the network only proves the formulas and bonds. Both proof stacks
share this code, so a bug here is common to both (atom `impl:shared-primitives`).

Two limits are stated in the code and the status file. When an atom occurs
several times in a formula, `reserve` splits its capacity evenly across the
occurrences: sound, but it can refuse a reservation an exact linear program
would admit. And the registry is one contended object, so every claim, bond
and reservation operation touches it; the status file calls that fine at
devnet scale. `insurance` assumes independent atoms.

## Tests

Unit tests inside `src/`: 31 `#[test]` functions at the time of writing
(counted with `grep`), in every module except `error`. They include two
`proptest` property tests (in `coc.rs` and `challenge.rs`), and `coc.rs` ships a `brute_force_coc` the tests compare the
solver against. There is no `tests/` directory and no `#[ignore]`d test. The
algebra is also exercised through the state machine by
`sion-exec/tests/properties.rs`, and by the differential tests in
`sion-spec` that run both proof stacks over the same workloads.

```text
cargo test -p sion-assurance
```

The crate is 3,780 lines of Rust under `src/` at the time of writing.

## Read more

- [The Assurance Algebra](../../docs/book/src/concepts/assurance/index.md)
- [Atoms and formulas](../../docs/book/src/concepts/assurance/atoms-and-formulas.md)
- [Cost of corruption](../../docs/book/src/concepts/assurance/cost-of-corruption.md)
- [The decision rule](../../docs/book/src/concepts/assurance/decision-rule.md)
- [Reservations](../../docs/book/src/concepts/assurance/reservations.md)
- [Bonds, challenges and slashing](../../docs/book/src/concepts/assurance/bonds-challenges-slashing.md)
- [Insurance](../../docs/book/src/concepts/assurance/insurance.md)
- [Claims](../../docs/book/src/concepts/claims.md)
- [Trust model](../../docs/book/src/security/trust-model.md)
