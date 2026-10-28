# Billions of payments a second

> **Status.** A research note, written on 28 October 2026. The construction
> it describes is built and tested: payment streams (specification v1.6)
> and checkpoint settlement (v1.7), in both proof stacks, with the x402
> `stream` scheme, the `sion stream` commands and the `streams` demo. Every
> figure below is labelled: **measured** (on the machine named in
> [Measurements](#4-measurements), with the command that reproduces it),
> **design target** (what the architecture is built to reach, not shown),
> or **our arithmetic** (computed here from measured or published figures).

## Summary

- **Count payments, not transactions.** A ledger that records every
  payment as a transaction is bounded by what its validators can check and
  prove per second. The [succinct-proof plan](succinct-proofs.md) estimates
  that a billion general transactions a second would need millions of
  prover machines. That target is not reachable by scaling the ledger.
- **Separate verification from settlement.** In a payment stream, each
  payment is one element of the payer's hash chain. The payee checks it
  when it arrives, with one hash (0.42 µs, **measured**). The ledger
  checks only settlements, and with checkpoints a settlement costs the
  same however many payments it covers: at most `spacing + 21` hashes and
  one signature check.
- **What we measured.** One core of a two-core cloud machine executed
  settlements covering about **9 × 10⁸ payments a second**, with every
  signature checked: 16 settlements of 1,048,064 payments each, by
  checkpoint, in 17 to 28 ms (four runs, 5.9 × 10⁸ to 1.0 × 10⁹ payments a
  second). Settling the same kind of payments by walking, one hash each,
  ran at about 2.3 to 2.7 × 10⁶ payments a second. Paying with one
  transaction per payment ran at about 880 a second on the same core.
- **What a billion payments a second would take** (our arithmetic): about
  950 settlements a second on the ledger, about 940 cores' worth of
  hashing spread over the payers and payees themselves, about 0.5 MB/s of
  published data, and on the order of ten prover machines per proof stack.
  The ledger side is two orders of magnitude below the Gate 3 target of
  100,000 operations a second. None of this has been run across machines
  yet: the devnet measured 15 transactions a second on one machine.
- **What it is not.** Stream payments are one-way, from one payer to one
  payee, out of escrow the payer locked in advance. They are not general
  transactions. The cryptography is from 1996 (PayWord, PayTree). The
  contribution is the ledger design around it, its two independent
  implementations and the measurements.

## 1. The question

The industry's unit of throughput is the transaction per second. It
measures the wrong thing for most of the traffic a ledger would carry at
scale: machine-to-machine payments that are small, frequent and between
parties that meet again and again. An AI agent calling an API a thousand
times an hour does not need a thousand ledger transactions. It needs each
call to be paid, each payment to be checkable, and the money to move with
certainty.

We count **verifiable payments per second**: payments that the payee has
checked when they arrived, that the ledger will settle as long as the
payee settles in time, and whose validity anyone can check later against
the ledger's state, without trusting either party.

The question this note answers: can a ledger offer billions of those a
second without validating billions of anything a second?

## 2. The construction

### 2.1 Streams (specification v1.6)

A payer opens a stream to one payee (§5.31): it escrows a deposit and
commits to a secret hash chain by publishing its last element, the anchor
`w_0`. With `step(x) = H("sion/stream/step", x)` (SHA3-256), the chain is
`w_n, …, w_1, w_0` with `w_{i−1} = step(w_i)`. Revealing `w_i` pays `i`
steps of the stream's unit.

- **Paying.** The payer reveals `w_1, w_2, …` in order, off the ledger.
  Over HTTP, the element rides in the request it pays for (the x402
  `stream` scheme).
- **Checking.** The payee checks a payment of `k` steps by hashing it `k`
  times and comparing with the last element it accepted. No signature, no
  ledger read.
- **Settling.** The payee settles with the latest element (§5.32). The
  ledger walks it down to the stream's head, one hash per step, at most
  65,536 steps, and pays out `unit × steps` from escrow. A fee of 2
  micro-USD per hash pays for the walk.
- **Ending.** The payer may top up the escrow and extend the stream (§5.33).
  The payee may close it at any time; the payer may close it only after its
  last Pulse (§5.34). Either way the payer gets back what is left.

### 2.2 Checkpoints (specification v1.7)

A walk makes the ledger check every payment again, one hash each. That is
cheaper than a signature by three orders of magnitude, but it still grows
with the number of payments. Checkpoints remove that growth.

When it opens the stream, the payer may also commit to a Merkle tree over
every `spacing`-th element of its chain: leaf `j` is the leaf hash of
`"sion/stream/checkpoint" ‖ j ‖ w_{j·spacing}`. The leaves are hashes of
chain elements, so they reveal nothing; the payer hands the payee the path
for each checkpoint as its payments pass it (the x402 client attaches it to
the first payment past each checkpoint).

To settle at position `p`, the payee may now send a **jump** instead of
walking: the checkpoint `j` just below `p`, and its path. The ledger walks
from `w_p` down to `w_{j·spacing}` (fewer than `spacing` hashes), hashes the
leaf and checks the path (at most 20 hashes). That is at most `spacing + 21`
hashes, whether the settlement covers ten payments or a billion; the fee is
charged on the hashes actually checked.

### 2.3 Why it is safe

Informally, assuming `step` is one-way (it is hard to find a preimage of a
chain element, or a different element with the same image after `k`
steps) and the Merkle tree hash is second-preimage resistant:

1. **The payer pays only for what it revealed.** To settle above the
   highest position the payer revealed, a payee must produce an element
   whose walk reaches the head, or reaches a committed checkpoint. Either
   means inverting `step`, finding a second preimage of an iterate of
   `step`, or a second preimage of the Merkle tree hash.
2. **The payee is paid for what it accepted,** if it settles by the
   stream's last Pulse and the escrow covers it. Only the payee can
   settle, and the payer cannot withdraw before the last Pulse. A payee's
   server therefore refuses payments the escrow does not cover, or from a
   stream that ends too soon for it to settle (`StreamDesk::margin`).
3. **Anyone can check any payment later.** Given a revealed element `w_p`
   and the ledger's settled position `c ≥ p`, hashing the head `c − p`
   times must give `w_p`; with the jump for `p`'s checkpoint, at most
   `spacing + 21` hashes do.
4. **A wrong commitment hurts only the payer's counterparty's
   convenience.** The ledger cannot check that the checkpoint root commits
   to the payer's chain. A wrong root makes jumps fail; the payee's server
   checks every jump it receives and falls back to walking.

Everything a payment relies on is a hash function. SHA3-256 keeps about 128
bits of security against preimage search on a quantum computer. The
signatures in this design (dual ML-DSA-44 and SLH-DSA) are on the
transactions that open, top up and settle streams, not on payments.

Both proof stacks implement the rules independently, and the differential
tests run every way a settlement can be wrong through both
(`crates/sion-spec/tests/differential.rs`, scenarios `streams` and
`stream_checkpoints`). A property test mixes walks, jumps, top-ups and
closes into random workloads and checks that supply is conserved and that
two nodes agree (`crates/sion-exec/tests/properties.rs`).

## 3. The capacity law

Write `V` for payments a second, `k` for payments per settlement,
`spacing` for the checkpoint spacing, `σ` for the cost of checking one
settlement's signature and everything else about it, and `h` for one
hash.

| Who | Work per second, walking | Work per second, with checkpoints |
|---|---|---|
| Payees, all together | `V · h` | `V · h` (plus one path per checkpoint) |
| Payers, all together | about `V · h` | about `V · h` |
| The ledger (each replica) | `V · h + (V / k) · σ` | `(V / k) · (σ + (spacing/2 + 21) · h)` |

With checkpoints the ledger's work per payment falls as `1 / k`: settle
less often and the ledger does less. Walking, it can never fall below one
hash per payment.

Measured on one core: `h ≈ 0.42 µs`, and `σ ≈ 1.0 ms` (one dual signature
check, 0.16 ms for ML-DSA-44 and 0.86 ms for SLH-DSA-SHA2-128f, plus
execution). At `k = 2²⁰` and `spacing = 1,024`, the ledger's work
per payment is about `(1.0 ms + 524 · 0.42 µs) / 2²⁰ ≈ 1.2 ns` (524 hashes:
half a spacing, the leaf and an 11-hash path), which predicts about
8.5 × 10⁸ payments a second per core. The benchmark measured 5.9 × 10⁸ to
1.0 × 10⁹.

What the rest of a billion payments a second would take (our arithmetic,
for `V = 10⁹`, `k = 2²⁰`, a unit of 0.0001 USD):

| Quantity | Value | How |
|---|---|---|
| Settlements on the ledger | about 954 a second | `V / k` |
| Ledger execution per replica | about 1.2 cores | `V · 1.2 ns` |
| Payee hashing, spread over all payees | about 420 cores | `V · h` |
| Payer hashing, spread over all payers | about 520 cores | measured 0.52 µs per reveal |
| Published data | about 0.5 MB/s | 547 bytes per settlement |
| Signature data validators receive | about 19 MB/s | 20 KB per settlement |
| Money in flight between payment and settlement | about 10 M USD | `V × unit × time to settle`, at 100 s |
| Proving, per proof stack (future succinct proofs) | about 13 machines, plus the ML-DSA checks | 954 settlements × about 7,000 hash compressions each (one SLH-DSA verification is about 6,300 SHA-256 compressions), at the best published rate of about 500,000 compressions a second per machine; what an ML-DSA verification costs to prove has not been measured |

For contrast, the [succinct-proof plan](succinct-proofs.md#summary)
estimates 4.7 to 19.3 million prover machines for a billion general
transactions a second.

Two limits remain at the edges, and both scale out rather than up. One
stream is sequential: a payee checks one payer's chain on one core, about
2.2 million payments a second. And each stream needs escrow: a payer
locks, in advance, what it may spend before the payee next settles.

## 4. Measurements

Measured on 28 October 2026, on a cloud sandbox with two virtual cores,
release build, nothing else running:

```bash
cargo build --release
./target/release/sion bench --only streams,execution,signatures
```

| Measurement | Value (three or four runs) |
|---|---|
| Payee checks a payment (one SHA3-256 hash) | 0.41 to 0.43 µs: 2.3 to 2.5 million a second per core |
| Payer builds a chain of a million steps, once | 0.38 to 0.41 s |
| Payer reveals the next element | 0.50 to 0.54 µs |
| 16 settlements of 65,536 payments each, walked | 2.3 to 2.7 million payments a second per core (395 to 447 ms) |
| Stateless replay of those settlements (proof stack A) | 1.9 to 2.3 million payments a second |
| 16 settlements of 1,048,064 payments each, by checkpoint (524 hashes each) | 5.9 × 10⁸ to 1.0 × 10⁹ payments a second per core (16.8 to 28.5 ms) |
| Stateless replay of those settlements (proof stack A) | 4.8 to 8.1 × 10⁸ payments a second |
| Fee, walked | 2.02 micro-USD per payment |
| Fee, by checkpoint | 2.05 micro-USD per thousand payments |
| Published bytes, by checkpoint | 0.52 bytes per thousand payments (547 bytes per settlement; the 20 KB of signatures are checked, not published) |
| For comparison: a Pulse of 512 transfers, with signature checks | 881 transactions a second per core |
| For comparison: verifying one dual signature | 1.02 ms (ML-DSA-44 0.16 ms, SLH-DSA-SHA2-128f 0.86 ms) |

The spread comes from the shared machine. Runs while other work shared it
were up to half as fast.

End to end, the `streams` demo (`sion demo streams --local`, release
build) pays an API 2,000 times over HTTP from one stream in 0.6 to 0.9 s,
from a single client thread on the same two cores as the devnet: the
server checks one hash per call plus a checkpoint path every 100 calls,
sends no transaction per call, and settles everything in two to four
transactions whose validators check 44 to 192 hashes in all.

## 5. What is new, and what is not

Not new:

- **Hash-chain payments.** PayWord (Rivest and Shamir, 1996) pays with the
  elements of a hash chain, one hash to check each. PayTree (Jutla and
  Yung, 1996) commits to payment tokens with a Merkle tree.
- **Paying without a ledger transaction per payment.** Payment channels
  (Lightning, 2016) and probabilistic payments (Rivest's lottery tickets,
  1997; MICROPAY, 2015) settle many payments in few transactions.
- **Very high off-chain throughput.** Sui reported 6.09 million operations
  a second in two-party "tunnels" on 4 July 2026, and 40.6 million on 7
  October 2026 in a vendor press release that does not say what was
  counted; both settle when a tunnel closes, with both parties' signatures.
  Radix reported 500,000 swaps a second sustained in a public test of its
  sharded design in January 2026. Project Hamilton processed 1.7 million
  transactions a second with a centralized design in 2022.
- **Metered x402.** Coinbase added an `upto` scheme to x402 in April 2026,
  letting a client authorize up to a limit and pay for what it uses.

What we have not found elsewhere, in the sources below, is the combination:

1. **No signature per payment, and no dispute game.** A channel update is
   signed by both parties and can be contested; a stream payment is a hash
   preimage, and the ledger simply pays the highest position the payee
   proves.
2. **Settlement whose cost does not grow with the payments it covers,** so
   that the ledger's work per payment tends to zero as settlements grow,
   while every payment stays checkable by anyone against the ledger.
3. **Hash functions only for payments,** which makes them post-quantum by
   construction, on a ledger whose transactions are themselves
   post-quantum.
4. **Two independent implementations** of the rules, with every edge case
   run through both, and a fee for each hash a settlement makes the
   validators check.
5. **HTTP-native.** An AI agent pays per call in the request itself, and
   the server checks the payment before it serves.

We state this as a hypothesis to test, not as a discovery: the separation
is simple, and someone may have published it.

## 6. Limits

- **One-way, one payee.** A stream pays one payee. Paying many parties
  means a stream to each, or a future netting design (problem 2 below).
- **Escrow locks capital.** The payer locks what it may spend before the
  next settlement.
- **The payee must settle in time.** A payee that stays offline past the
  stream's last Pulse loses what it did not settle; the payer can then take
  the escrow back. This is the payee's own liveness, not a watchtower's.
- **Prices are whole numbers of steps.** A payment of `k` units costs the
  payee `k` hashes.
- **Payers build their chain up front:** one hash per step, about 0.4 s
  per million steps. Fractal traversal (Coppersmith and Jakobsson, 2002)
  would make this logarithmic; it is not built.
- **Fees price execution, not proving.** Two micro-USD per hash prices the
  validators' work today. Under succinct proofs a hash costs far more to
  prove than to compute, and the fee schedule must follow (the
  [succinct-proof plan](succinct-proofs.md) flags this for every fee).
- **Nothing here ran across machines.** The billion-payment figures assume
  a network that carries about a thousand settlements a second, which the
  [networking plan](networking.md) has not built.

## 7. Open problems

1. **A lower bound.** Is one hash per payment at the payee optimal for
   publicly verifiable, post-quantum payments without a signature each?
   MACs are as cheap but not publicly verifiable.
2. **Netting.** Pay many payees from one escrow, with settlement cost
   independent of the number of payees, e.g. by succinct proofs of netted
   balances.
3. **Two-way streams,** without reintroducing signed state updates and
   dispute periods.
4. **Proving hash walks cheaply.** Keep SHA3-256 for consensus, as the
   succinct-proof plan recommends, and find out what a jump costs to prove
   in each proof stack.
5. **Insured unsettled balances.** Use the Assurance Algebra to let a
   payee rely on a bonded claim about a payer's future top-ups, and accept
   payments beyond the escrow at a priced risk.
6. **Receipts without the payer.** Let anyone check any old payment by
   jump without asking the payer for the path, e.g. by publishing the
   checkpoint leaves in data availability at open.

## 8. Next experiments

1. **Across machines.** A devnet on several machines with 1,000 streams,
   each settling every 2¹⁶ payments: payments per second at the edges,
   settlements per second on the ledger, time from payment to settlement.
2. **A real API server.** The payee check behind a production HTTP server
   with keep-alive connections, measured in requests per second per core.
3. **Proving a settlement** in the first succinct proof stack (phase 1 of
   the [succinct-proof plan](succinct-proofs.md)).
4. **Adversarial runs.** Payers who never top up, payees who hoard
   unsettled payments until the last Pulse, and bursts of failing jumps
   (each pays its hashes' fee).

## Sources

Opened on 28 October 2026.

- Rivest and Shamir, [PayWord and MicroMint: two simple micropayment schemes](https://people.csail.mit.edu/rivest/pubs/RS96a.pdf) (1996).
- Jutla and Yung, [PayTree: "amortized signature" for flexible micropayments](https://www.usenix.org/legacy/publications/library/proceedings/ec96/summaries/node31.html), USENIX Electronic Commerce (1996).
- Rivest, [Electronic lottery tickets as micropayments](https://fermatslibrary.com/s/electronic-lottery-tickets-as-micropayments) (1997).
- Coppersmith and Jakobsson, [Almost optimal hash sequence traversal](https://eprint.iacr.org/2002/023) (2002).
- Pass and shelat, [Micropayments for decentralized currencies](https://shelat.khoury.northeastern.edu/research/2015-07-25-micropayment) (2015).
- Poon and Dryja, [The Bitcoin Lightning Network](https://www.Lopp.net/pdf/lightning-network-paper.pdf) (2016).
- Lovejoy et al., [Hamilton: a high-performance transaction processor for central bank digital currencies](https://www.usenix.org/conference/nsdi23/presentation/lovejoy), NSDI (2023).
- Radix, [Hyperscale update: 500k public test done](https://www.radixdlt.com/blog/hyperscale-update-500k-public-test-done) (January 2026).
- Sui Foundation, [Sui processes over 6 million transactions per second in AI agent livestream experiment](https://www.sui.io/blog/sui-processes-over-6-million-transactions-per-second-in-ai-agent-livestream-experiment) (July 2026).
- Sui, [press release on 40 million transactions per second](https://cryptoslate.com/press-releases/sui-sets-record-for-highest-verified-throughput-settled-to-a-blockchain-40-million-transactions-per-second/) (7 October 2026, sponsored).
- ForkLog, [Developers of x402 implement usage-based AI computation payments](https://forklog.com/en/news/developers-of-x402-implement-usage-based-ai-computation-payments) (April 2026).
- Ethereum Magicians, [The real blocker for post-quantum Ethereum isn't the math, it's aggregation](https://ethereum-magicians.org/t/the-real-blocker-for-post-quantum-ethereum-isn-t-the-math-it-s-aggregation-and-we-don-t-have-it-yet/27839) (February 2026).
