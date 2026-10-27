# Benchmarks

`sion bench` measures the costs that decide how far this design scales:
post-quantum signatures, state roots, data-availability encoding,
clearing certificates, the Assurance Algebra, execution, replay proving,
and a live devnet. Run it on your own machine; the numbers below are from
one machine and say little about yours.

```text
Usage: sion bench [OPTIONS]

Options:
      --quick        Smaller sizes
      --no-devnet    Skip the end-to-end devnet measurement
      --only <ONLY>  Run only these groups: signatures, state, data, clearing, assurance, execution, devnet (comma-separated)
      --json <JSON>  Write the measurements as JSON to this file
```

`--only state` includes the hashing measurement, and `--only execution`
the proving ones. The whole suite took 70 seconds below; the
devnet group starts its own in-process devnet.

## One run

On a virtual machine with 2 cores (Intel Xeon at 2.10 GHz) and 7 GB of
memory, shared with other work (load average about 6 during the run), in a
release build of an earlier version, before validators, staking and epochs
joined the state transition:

```text
group        measurement                                                   value unit     note
signatures   ML-DSA-44 sign                                               688.36 µs
signatures   ML-DSA-44 verify                                             202.60 µs       2420 B signature
signatures   SLH-DSA-SHA2-128f sign                                     16366.62 µs
signatures   SLH-DSA-SHA2-128f verify                                    1062.20 µs       17088 B signature
signatures   SLH-DSA-SHA2-128s sign                                    335272.82 µs
signatures   SLH-DSA-SHA2-128s verify                                     292.59 µs       7856 B signature
hashing      SHA3-256, 32-byte input                                        0.45 µs
state        state root of 1000 objects                                     1.08 ms       root 7a3eb156
state        state root of 100000 objects                                 125.57 ms       root 6946f81d
data         2D Reed–Solomon extend, 16 KiB                                 0.81 ms       128×128 extended square
data         row/column commitments, 16 KiB                                21.52 ms
data         2D Reed–Solomon extend, 256 KiB                               12.58 ms       512×512 extended square
data         row/column commitments, 256 KiB                              417.56 ms
data         2D Reed–Solomon extend, 2048 KiB                             235.63 ms       2048×2048 extended square
data         row/column commitments, 2048 KiB                            7975.27 ms
clearing     solve batch of 10 orders                                       0.33 ms       20 units traded
clearing     verify certificate, 10 orders                                  0.47 ms       exact rational arithmetic
clearing     solve batch of 100 orders                                      3.08 ms       256 units traded
clearing     verify certificate, 100 orders                                12.69 ms       exact rational arithmetic
clearing     solve batch of 2000 orders                                   111.13 ms       5236 units traded
clearing     verify certificate, 2000 orders                              187.59 ms       exact rational arithmetic
assurance    cost of corruption, 64-witness claim                          29.65 µs
assurance    decision rule with reservation check                          42.78 µs
execution    client signing (ML-DSA-44 + SLH-DSA-128f)                  24952.23 µs/tx    512 transfers
execution    Pulse of 512 transfers, with signature checks                612.20 tx/s     836.3 ms per Pulse, one core
execution    Pulse of 512 transfers, state machine only                 15959.34 tx/s     32.1 ms per Pulse, one core
proving      traced execution of 512 transfers                            918.89 ms       9 chunks with witnesses
proving      stateless replay of every chunk (stack A)                   2375.81 ms       1699 KiB of witnesses
proving      stateless replay of every chunk (stack B)                   2470.59 ms       independent implementation of the specification
devnet       Pulse interval (4 validators, dual PQ signatures)           3927.44 ms       4 consensus rounds per Pulse
devnet       128 transfers from 32 accounts: throughput                    15.42 tx/s     lane certification + consensus + execution, one machine
devnet       submit → executed, median                                   7061.38 ms
devnet       submit → executed, 95th percentile                          8235.78 ms
```

## What the numbers say

**Signatures dominate.** A dual signature costs a client 25 ms to make
(SLH-DSA-SHA2-128f is 16 ms of it) and a verifier about 1.3 ms to check, and
it is 19.5 KB on the wire. Checking signatures is what brings one core from
about 16,000 transfers a second (state machine only) down to about 600.
This is why validators sign one Merkle root per batch of approvals instead
of each approval, and why the architecture moves signature checking into
proofs. SLH-DSA-SHA2-128s makes smaller, faster-to-verify signatures but
takes a third of a second to sign: fine for keys that sign rarely.

**Data availability encoding is the next wall.** Extending 2 MiB of data
takes a quarter of a second, but committing to its rows and columns takes
8 seconds on one core. The largest Pulse the devnet allows is therefore
slow to publish, an open issue listed in the implementation status.

**Clearing certificates are cheap to check.** Verifying the certificate of
a 2,000-order batch took 188 ms, with exact rational arithmetic. Checking
is linear in the batch, where solving is not.

**The Assurance Algebra is not a bottleneck.** Pricing a claim with 64
witnesses and applying the decision rule each take tens of microseconds.

**Replay proofs are not succinct.** Re-verifying a Pulse of 512 transfers
took about 2.4 seconds per stack, with 1.7 MB of witnesses, more than
executing it with signature checks. A succinct proof stack would make
verification cheap; this one simulates its interface, not its cost
(`impl:replay-proofs`).

**The devnet is far from the targets.** Four validators in one process, on
two shared cores, closed a Pulse every 3.9 seconds and moved 15 transfers a
second, with a median of 7 seconds from submission to execution. The
architecture's targets (100,000 operations a second on a testnet, sub-second
certificate finality) assume many machines, networking, and signature
aggregation in proofs. Nothing here measures them.

## Measuring your own machine

```bash
cargo build --release
./target/release/sion bench --json bench.json
```

Use `--quick` for a short run and `--no-devnet` to skip the end-to-end
part. Close other heavy work first: the devnet group is sensitive to load.
