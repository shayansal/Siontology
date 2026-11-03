# sion-clearing

Verifiable Clearing for contended markets: sealed batches that clear with a
proof of optimality. Any solver may submit an allocation, but it must come with
a certificate (a dual solution) that no better allocation exists. The ledger
checks the certificate with [`sion-lp`](../sion-lp/README.md) instead of
trusting a matching engine, keeps the best certified submission, and settles
everyone at one certified price. Two ideas matter to a user. Ties are broken by
a priority derived from the post-seal beacon and each order's ID, so a solver
cannot choose which of two equal bidders wins. And a certificate proves
optimality for the orders as submitted; bidders can still misreport, and
uniform-price batch auctions are not fully strategy-proof.

Part of [Siontology](../../README.md) · [Architecture](../../ARCHITECTURE.md)
· Status: **Built** (sealed double auctions and assignment markets are
implemented with certificates and tests; assignment markets are not wired
into the ledger, see below; see
[implementation status](../../docs/implementation-status.md)).

## What is in it

| Module | What it holds |
|---|---|
| `auction` | Single-good double auctions priced in a numeraire: `Order`, `Batch`, `clear`, `verify_clearing`, `settlement` |
| `assignment` | Assignment markets that match jobs to slots: `AssignmentBatch`, `solve_assignment`, `verify_assignment`, `settle_assignment` |
| `priority` | Beacon-derived priorities `π ∈ (0, 1)` and the perturbation weight `delta` |
| `select` | `select_best`: keep the best submission whose certificate verifies |
| `error` | `ClearingError`, why a clearing or assignment was rejected |

## Main types and functions

- [`Batch`, `Order`, `Side`](src/auction.rs): one sealed batch of a contention domain and its decrypted orders. Prices are micro-units of the numeraire (a ceiling for buyers, a floor for sellers).
- [`clear`](src/auction.rs): a solver for a batch. It matches greedily and writes the duals in closed form, and returns a `ClearingResult` with the fills, price band, canonical price, certificate and objective.
- [`verify_clearing`](src/auction.rs): the check the ledger runs. It rebuilds the program, requires the certificate to pass with zero tolerance (so the allocation is exactly optimal for the perturbed program, which also fixes the tie-break), checks that volumes balance, that the band and price follow the canonical rule, and that every filled order trades within its limit.
- [`settlement`](src/auction.rs): turns a verified clearing into `Transfer`s at the uniform price.
- [`PriceBand`](src/auction.rs): the interval of uniform prices that support an allocation; the canonical price is its midpoint rounded down.
- [`priority`, `delta`](src/priority.rs): the tie-break. Each order's objective coefficient gets a perturbation `δ·π` small enough never to overturn a real improvement. The certificate is for the perturbed program; welfare and prices are reported at the original limits.
- [`AssignmentBatch`, `Job`, `Slot`, `Bid`](src/assignment.rs): a batch of jobs with values for acceptable slots, and slots with reserves.
- [`solve_assignment`, `verify_assignment`, `settle_assignment`](src/assignment.rs): the same three steps for assignment. The program is totally unimodular, so the certified optimum is a 0/1 matching, and the slot duals are a certified competitive-equilibrium price vector.
- [`select_best`, `select_best_clearing`, `select_best_assignment`](src/select.rs): discard every submission whose certificate fails and keep the best certified objective; the earliest wins a tie.

The crate documentation has a doc example (a buyer of 8 GPU-hours and two
sellers clearing at 2.00 USD per hour); see the top of
[`src/lib.rs`](src/lib.rs).

## How it fits

Depends on: `sion-core`, `sion-lp`, and `num-bigint` and `num-traits`. Used by:
`sion-exec`, `sion-spec`, `sion-node` and `sion-cli`.

The crate takes decrypted orders and produces and checks allocations. It does
not collect orders, seal or decrypt them (that is `sion-crypto` and the ledger
in `sion-exec`), and it does not move money. Only the double auction is used
by the ledger today: the status file lists "assignment markets on the ledger"
as not built, so `assignment` is a library with tests. Both proof stacks share
this code (atom `impl:shared-primitives`).

## Tests

- Unit tests inside `src/`: 9 `#[test]` functions at the time of writing
  (counted with `grep`) for empty and one-sided batches, duplicate IDs,
  crossing orders, the priority range and the perturbation bound, assignment
  preferences and unknown slots, and `select_best`.
- `tests/auction.rs`: 7 tests, including the GPU-hours example, tie-breaking
  by beacon, rejection of a wrongly tie-broken allocation, of a suboptimal one
  and of tampered submissions, and a `proptest` test that random books clear
  with a verifying certificate, that the greedy solver agrees with the general
  simplex from `sion-lp`, and that settlement balances within every limit.
- `tests/assignment.rs`: 2 tests, a tampering test and a `proptest` test
  comparing the certified matching to a brute-force optimum.

There are 18 `#[test]` functions in all, and none is `#[ignore]`d.

```text
cargo test -p sion-clearing
```

The crate is 1,402 lines of Rust under `src/` at the time of writing.

## Read more

- [Verifiable Clearing](../../docs/book/src/concepts/verifiable-clearing.md)
- [Privacy](../../docs/book/src/concepts/privacy.md): sealed orders and committee key shares.
- [Pulses](../../docs/book/src/concepts/pulses.md) and [Contention Consensus](../../docs/book/src/concepts/contention-consensus.md): where contended batches are ordered.
- [Crate map](../../docs/book/src/architecture/crates.md)
