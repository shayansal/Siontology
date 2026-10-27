# Two levels of finality

Siontology gives two guarantees, at two speeds.

| Level | What it guarantees | Who relies on it | Architecture target |
|---|---|---|---|
| **Certificate finality** | Validators holding more than two thirds of the stake approved it and will not approve a conflict, if fewer than a third are Byzantine | Users, merchants, agents | Under one second |
| **Proof finality** | The World Proof covers it; anyone can check its validity without trusting the committee (ruling out a conflicting proof still assumes fewer than a third are Byzantine) | Light clients, bridges, auditors | 10 to 30 s at launch; a few seconds at maturity |

## Certificate finality in this implementation

- **Owned paths.** A transaction is final when its lane certificate forms:
  approvals from more than two thirds of its shard committee. Each member
  has locked the input versions to it, so no conflicting spend can be
  certified by that committee afterwards, even across a restart (locks are
  durable). When the committee changes before the transaction executes,
  the gateway certifies it again with the new one; see
  [Linear Lanes](linear-lanes.md#locks-restarts-and-committee-changes).
- **Contended paths.** A contended transaction is final when consensus
  commits it.
- **Pulse certificates.** After executing a Pulse, the validators sign its
  header. A header signed by more than two thirds of the active set's stake
  is certified; a receipt proven against a certified
  header is final for a light client that follows the committee from the
  genesis it pinned. `sion receipt` checks exactly this.

## Proof finality in this implementation

The prover builds a World Proof for each Pulse, and `sion verify` re-checks
it. Because the World Proof is a replay proof, "proof finality" costs a
verifier as much as executing the Pulse (atom `impl:replay-proofs`). The
status fields `certified_pulse` and `proven_pulse` show how far each level
has reached:

```text
$ sion status
{
  ...
  "certified_pulse": 662,
  ...
  "proven_pulse": 662,
  "pulse": 662,
  ...
}
```

## Measured latency

`sion bench` measures the time from submitting a transfer to its execution
on an in-process devnet with 4 validators and dual post-quantum signatures.
On the machine used for this book (2 cores, Intel Xeon at 2.10 GHz, shared
with other builds, load average about 6), with an earlier version:

```text
devnet       Pulse interval (4 validators, dual PQ signatures)           3927.44 ms       4 consensus rounds per Pulse
devnet       128 transfers from 32 accounts: throughput                    15.42 tx/s     lane certification + consensus + execution, one machine
devnet       submit → executed, median                                   7061.38 ms
devnet       submit → executed, 95th percentile                          8235.78 ms
```

That includes certification, ordering, waiting for the Pulse to close and
execution, all on two shared cores. It is far from the architecture's
targets, and it says little about them: the devnet runs every validator in
one process, signs every batch with SLH-DSA-SHA2-128f (16 ms per signature
on this machine) and has no networking. See [Benchmarks](../guides/benchmarks.md)
to measure your own machine.

## What finality does not cover

- Proofs stop invalid state, not equivocation. A Byzantine third of a
  committee can certify two conflicting spends. Equivocation leaves signed
  evidence, and anyone who reports it has the offender's whole stake
  slashed ([Slashing for equivocation](validators/equivocation-slashing.md)),
  but the offender keeps its seat until the epoch ends.
- The architecture caps value in flight (certified but not yet proven) at
  the committee's slashable stake. That cap is not built.
