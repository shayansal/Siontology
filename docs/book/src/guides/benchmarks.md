# Benchmarks

`sion bench` measures the costs that decide how far this design scales:
post-quantum signatures, state roots, data-availability encoding,
clearing certificates, the Assurance Algebra, execution, replay proving,
payment streams, and a live devnet. Run it on your own machine; the numbers below are from
one machine and say little about yours.

```text
Usage: sion bench [OPTIONS]

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --quick        Smaller sizes
      --no-devnet    Skip the end-to-end devnet measurement
      --only <ONLY>  Run only these groups: signatures, authorization, state, data, clearing, assurance, execution, streams, devnet (comma-separated)
      --json <JSON>  Write the measurements as JSON to this file
  -h, --help         Print help
```

In the run below, from an earlier version, the hashing measurement is
labelled `hashing` and the replay-proving ones `proving`; they now appear
under `state` and `execution`, the groups `--only` selects them by. The
whole suite took 70 seconds; the devnet group starts its own in-process
devnet.

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

## Payment streams

The `streams` group measures what a [payment stream](../concepts/payment-streams.md)
payment costs its payee, its payer and the ledger. On the same kind of
2-core machine, idle, release build, 28 October 2026
(`sion bench --only streams`):

```text
group        measurement                                                   value unit     note
streams      payee checks a payment (one SHA3-256 hash)                     0.43 µs       2.30 million payments/s per core
streams      payer builds a chain of 1000000 steps (once)                 380.45 ms       one hash per step
streams      payer reveals the next element                                 0.50 µs       in order; checkpoints every √n steps
streams      Pulse of 16 settlements × 65536 steps, walked            2343833.23 payments/s 447.4 ms per Pulse with signature checks, one core; one hash per step
streams      stateless replay of those settlements (stack A)          2219115.59 payments/s 472.5 ms
streams      fee per payment, walked settlement                             2.02 µUSD     0.132172 USD for 65536 steps
streams      Pulse of 16 settlements × 1048064 steps, by checkpoint   996467770.74 payments/s 16.8 ms per Pulse with signature checks, one core; 524 hashes each
streams      stateless replay of those settlements (stack A)        812938546.36 payments/s 20.6 ms
streams      fee per 1,000 payments, settlement by checkpoint               2.05 µUSD     0.002148 USD for 1048064 steps
streams      published bytes per 1,000 payments, by checkpoint              0.52 B        547 B body; 20071 B with signatures, which are verified, not published
```

**A settlement by checkpoint costs the same however many payments it
covers.** Each of the 16 settlements above covers 1,048,064 payments for
524 hashes and one dual signature check, so one core executed about 10⁹
settled payments a second in this run (5.9 × 10⁸ to 1.0 × 10⁹ over four
runs). Walking costs one hash per payment, and a transfer per payment
costs a signature check: about 2.3 × 10⁶ and 880 a second. The payments
themselves are checked by their payees, one hash each. What this does and
does not mean for throughput at scale is in
[Billions of payments a second](../research/billion-scale.md).

## Authorization

The `authorization` group measures what [batched
authorization](../concepts/batched-authorization.md) saves: checking and
carrying a transaction's signatures when it is signed by itself and when
it is a member of a batch, and executing a Pulse of agent payments both
ways. On a 2-core virtual machine (Intel Xeon at 2.80 GHz, without the
SHA extensions, so SLH-DSA-SHA2 signed and verified four to five times
slower than on the machine of the first run), idle, release build of
specification v1.13, 29 October 2026 (`sion bench --only
signatures,authorization`, the second of three runs):

```text
group        measurement                                                   value unit     note
signatures   ML-DSA-44 sign                                               263.15 µs       
signatures   ML-DSA-44 verify                                             182.91 µs       2420 B signature
signatures   SLH-DSA-SHA2-128f sign                                     77007.00 µs       
signatures   SLH-DSA-SHA2-128f verify                                    4152.45 µs       17088 B signature
signatures   SLH-DSA-SHA2-128s sign                                   1590552.76 µs       
signatures   SLH-DSA-SHA2-128s verify                                    1679.48 µs       7856 B signature
authorization signed by itself: check                                     4501.87 µs       ML-DSA-44 + SLH-DSA-SHA2-128f, per transaction
authorization signed by itself: bytes                                    19525.00 B        signatures, per transaction
authorization batch of 16: check per member                                537.84 µs       one signature check per batch, one inclusion path per member
authorization batch of 16: bytes per member                               1434.25 B        214 B of place and path, plus a 16th of the signatures
authorization batch of 64: check per member                                149.61 µs       one signature check per batch, one inclusion path per member
authorization batch of 64: bytes per member                                583.06 B        278 B of place and path, plus a 64th of the signatures
authorization batch of 256: check per member                                30.40 µs       one signature check per batch, one inclusion path per member
authorization batch of 256: bytes per member                               418.27 B        342 B of place and path, plus a 256th of the signatures
authorization Pulse of 256 agent payments, keyed subagents, each signed by itself        5314.39 µs/tx    1360.5 ms per Pulse with signature checks, one core
authorization Pulse of 256 agent payments, keyless subagents, one batch         112.14 µs/tx    28.7 ms per Pulse with signature checks, one core
```

**A batch member costs a share of one check.** In a batch of 256, a
member's check took 30 µs (30 to 32 µs over three runs), against 4.5 ms
(4.5 to 6.7 ms) for a transaction signed by itself: about 150 times less.
That is a 256th of one dual signature check plus the member's own
inclusion path. A member carries 418 bytes, 342 of them its place in the
batch (the batch, its index, an eight-hash path and the charter option)
and the rest a 256th of the signatures, against 19,525 bytes for a
transaction signed by itself: 47 times less. Smaller batches save less: a
member of a batch of 64 carried 583 bytes and cost 127 to 170 µs.

**A Pulse of agent payments.** On one core, with every signature checked,
a Pulse of 256 payments from keyless subagents in one batch executed in 29
to 43 ms over three runs, and a Pulse of 256 payments each signed by its
own keyed subagent in 1.25 to 1.36 s: 29 to 47 times longer, run by run.
Both include everything else executing a payment costs: its caveats, its
allotment and its coins.

## Measuring your own machine

```bash
cargo build --release
./target/release/sion bench --json bench.json
```

Use `--quick` for a short run and `--no-devnet` to skip the end-to-end
part. Close other heavy work first: the devnet group is sensitive to load.

CI runs `sion bench --quick --json bench.json` every night
(`.github/workflows/nightly.yml`) and keeps the file as a build artifact for
90 days, so a change in a measured number shows up between nights. Shared
CI machines are noisy: compare trends, not single runs.
