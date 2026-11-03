# sion-consensus

The ordering layer of Siontology. It orders only what conflicts. An object with one owner is finalised by a lane certificate, with no consensus at all ([Linear Lanes](../../docs/book/src/concepts/linear-lanes.md)). Only contended objects, and the lane certificates of a Pulse, go through a certified-DAG BFT protocol in the Bullshark family ([Contention Consensus](../../docs/book/src/concepts/contention-consensus.md)). The crate also holds committees and their sampling from a randomness beacon, the beacon itself, a tracker for forced-inclusion deadlines, and a deterministic network simulator that tests safety and liveness. Two ideas matter for using it: the consensus core is a pure state machine (`handle(event)` returns actions for a driver to carry out), and signatures sit behind two traits so the same code runs with a fast simulation MAC in tests and with dual post-quantum signatures in a node.

Part of [Siontology](../../README.md) · [Architecture](../../ARCHITECTURE.md)
· Status: **Simplified** (Linear Lanes are built, but Contention Consensus is a certified DAG, not the uncertified one the architecture targets, and forced inclusion is obligations and committee rotation without freezing the shard; see [implementation status](../../docs/implementation-status.md)).

## What is in it

| Module | What it holds |
|---|---|
| `lanes` | Linear Lanes: per-validator locks on `(object, version)`, approvals signed once per batch root, lane certificates, owner and validator equivocation evidence |
| `core` | The certified-DAG consensus state machine: rounds, anchors, commit rule, timers, committed sub-DAGs |
| `dag` | DAG vertices: headers, votes, certificates, messages and equivocation evidence |
| `committee` | Stake-weighted committees, quorum and validity thresholds, committee sampling and resampling from a beacon, capture probability |
| `beacon` | Hash-chain commit-reveal randomness (RANDAO-like) |
| `forced` | Forced-inclusion deadlines: posted intents, their resolution, overdue intents |
| `auth` | The `Authenticator` and `SignatureVerifier` traits, domain-separated signing digests, and the insecure simulation MAC |
| `sim` | A deterministic simulator with Byzantine behaviours, seeded delays and an adversarial phase before the global stabilisation time |
| `error` | `ConsensusError` |

## Main types and functions

- [`LaneValidator`](src/lanes.rs): one validator's lane state for one committee; approves a request at most once per object version and seals approvals into signed batches.
- [`CertificateBuilder`](src/lanes.rs): collects sealed approvals into a `LaneCertificate` once they reach a quorum.
- [`verify_lane_certificate`](src/lanes.rs): checks a certificate against a committee.
- [`find_conflicting_approvals`](src/lanes.rs): turns two conflicting approvals into `ValidatorEquivocation` evidence.
- [`Core`](src/core.rs): one validator's consensus state machine; driven by `Event`, answers with `Action`, reports `CommittedSubDag`.
- [`CoreConfig`](src/core.rs): timeouts, garbage-collection depth, size limits, re-proposal delay and the `LeaderSchedule`.
- [`Committee`](src/committee.rs): members and weights for one chain, epoch and shard.
- [`sample_committee`](src/committee.rs) and [`resample_committee`](src/committee.rs): stake-weighted draws from a beacon value; the second draws first from outside a failed committee.
- [`HashChain`](src/beacon.rs) and [`Beacon`](src/beacon.rs): a validator's reveal chain and the per-Pulse beacon output.
- [`ForcedInclusionTracker`](src/forced.rs): pending and resolved forced intents with a deadline in Pulses.
- [`Simulation`](src/sim.rs): runs real `Core` instances over a simulated network and reports safety, liveness and evidence.

## How it fits

Depends on: `sion-core`. Outside the workspace it uses `borsh`, `serde`, `thiserror`, and `rand_core` with `rand_chacha` for the simulator's seeded randomness. Used by: `sion-exec`, `sion-spec`, `sion-prover`, `sion-light`, `sion-node` and `sion-cli`.

The crate decides who may certify and in what order contended transactions commit. It does not execute transactions, hold state, or touch the network: `sion-exec` applies what is ordered, and `sion-node` drives `Core` and `LaneValidator` over a transport. The `ForcedInclusionTracker` is used by the validator in `sion-node`; the obligations recorded in the state are in `sion-exec`. The simulator's MAC is insecure by design and labelled so; nodes plug in `sion-crypto`'s dual signatures. The beacon can be biased by the last validator to reveal, which can withhold (`impl:hash-chain-beacon`).

## Tests

- Unit tests in the modules: 18 `#[test]` functions under `src/`.
- `tests/consensus_sim.rs`: 11 tests. Four validators commit every item; seven tolerate two crashes; an adversarial network before the stabilisation time; equivocating proposers and colluding double voters cannot break safety; silent voters and invalid signatures; stake-weighted committees; conflicts beyond the fault bound are detected and attributed; garbage collection bounds the DAG; the simulation is deterministic. One of the 11 is a property test (`proptest`, 16 cases) over random faults within tolerance.
- `tests/lanes.rs`: 9 tests of certificates, batch-root voting, atomic locks, owner equivocation evidence, conflicting certificates below and at a third Byzantine stake, weighted committees, restored locks across epochs, and chain binding.
- `tests/observer.rs`: 1 test that an observer commits what members commit without speaking.
- Counts come from `grep -c "#\[test\]"`, 39 in all. No test is `#[ignore]`d.
- The examples `simulate` and `stress` print commit statistics and run 300 random fault configurations; they are not part of `cargo test`.

Run the tests with `cargo test -p sion-consensus`.

## Read more

- [Linear Lanes](../../docs/book/src/concepts/linear-lanes.md) and [Escrow Lanes](../../docs/book/src/concepts/escrow-lanes.md)
- [Contention Consensus](../../docs/book/src/concepts/contention-consensus.md), including the simulator and the differences from the architecture
- [Committees](../../docs/book/src/concepts/committees.md) and [epochs](../../docs/book/src/concepts/validators/epochs.md)
- [Censorship resistance](../../docs/book/src/concepts/censorship-resistance.md) and the [forced inclusion guide](../../docs/book/src/guides/forced-inclusion.md)
- [Two levels of finality](../../docs/book/src/concepts/finality.md)
- [The validator thread](../../docs/book/src/architecture/validator.md), which drives this crate
