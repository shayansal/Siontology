# Payment streams

A **payment stream** lets one account pay another many times without a
transaction per payment. The payer escrows funds once and pays by revealing
the elements of a secret hash chain, one per payment. The payee checks each
payment with one hash, and settles thousands or millions of them in one
transaction. With checkpoints, a settlement costs the ledger the same however
many payments it covers.

Streams are built for traffic that is small, frequent and between parties
that deal with each other again and again: an AI agent paying an API per
call, a device paying for data by the megabyte, a service paying a model
per token.

## How it works

1. **Open.** The payer picks a random secret `w_n` and hashes it `n` times:
   `w_{i−1} = H(w_i)`, down to `w_0`, the anchor. It sends `OpenStream`
   with the anchor, the value of one step (`unit`), the chain's length,
   a deposit, and the last Pulse in which the payee may settle. The deposit
   moves into the stream, a shared object.
2. **Pay.** To pay `k` steps, the payer reveals the element `k` positions
   above the last one it revealed. It does this off the ledger; over HTTP
   the element rides in the request it pays for.
3. **Check.** The payee hashes the element `k` times. If it gets the last
   element it accepted, the payment is genuine: only the payer could have
   produced it. No signature and no ledger read are needed.
4. **Settle.** From time to time the payee sends `SettleStream` with the
   latest element. The ledger checks it and pays `unit × steps` out of
   escrow. The element becomes the stream's new head.
5. **End.** The payer can top up the escrow and extend the stream. The
   payee can close the stream at any time, and the payer can close it once
   the last Pulse has passed. Either way, what is left goes back to the
   payer.

## Settling by walking, or by a jump

A settlement proves that its element is genuine in one of two ways:

- **A walk.** The ledger hashes the element down to the stream's head, one
  hash per step, at most 65,536 steps per settlement.
- **A jump.** If the payer opened the stream with **checkpoints**, it
  committed to a Merkle tree over every `spacing`-th element of its chain.
  The payee then names the checkpoint just below its position and gives the
  checkpoint's Merkle path. The ledger walks to the checkpoint (fewer than
  `spacing` hashes), hashes the leaf and checks the path (at most 20
  hashes). A jump costs at most `spacing + 21` hashes, whether it settles
  ten payments or a billion.

The payer hands the payee the path for each checkpoint as its payments pass
it. The leaves are hashes of chain elements, so the paths give nothing
away. A settlement pays a fee of 2 micro-USD for each hash it makes the
validators check, on top of the minimum fee.

On one core, settlements by jump executed at about 9 × 10⁸ settled payments
a second with every signature checked; walking, at about 2 × 10⁶. See
[Billions of payments a second](../research/billion-scale.md) for the
measurements and the capacity law.

## What each side can rely on

| Who | Can rely on | Must do |
|---|---|---|
| Payer | It pays only for elements it revealed: settling above them means inverting the hash function | Keep the secret; never reveal an element for two different payments |
| Payee | Every payment it accepted is backed by escrow until the last Pulse, and only it can settle | Refuse payments the escrow does not cover; settle before the last Pulse |
| Anyone else | Any payment can be checked against the ledger later: hash the settled head down to the payment's position, or check it by its checkpoint | Nothing |

The payee carries one risk: if it stays offline past the stream's last
Pulse, the payer may close the stream and take back what was not settled.
That is why a payee's server refuses payments from a stream that ends too
soon for it to settle, and settles everything once a stream gets close.

## The transactions

- **`OpenStream { payee, asset, unit, length, anchor, deposit, until,
  checkpoints }`** (owned linear). Draws `deposit` from the sender's coins
  into a new stream. `length` is at most 2³²; `until` at most 1,000,000
  Pulses ahead; with checkpoints, `spacing` is between 1 and `length` with
  at most 2²⁰ checkpoints.
- **`SettleStream { stream, position, value, jump }`** (contended, in the
  stream's own domain). Only the payee, by the last Pulse. Pays `unit ×
  (position − settled position)` if the escrow covers it.
- **`TopUpStream { stream, deposit, until }`** (contended). Only the payer,
  by the last Pulse. Adds to the escrow and may push the last Pulse later,
  never earlier.
- **`CloseStream { stream }`** (contended). The payee at any time, or the
  payer after the last Pulse. Returns the escrow to the payer. A closed
  stream keeps its last head, so its payments stay checkable.

The rules are §5.31 to §5.34 of the [state transition
specification](../specification.md), versions 1.6 and 1.7.

## Streams, channels and transfers

| | A transfer per payment | A payment channel | A payment stream |
|---|---|---|---|
| Ledger transactions per payment | 1 | 0 (2 per channel) | 0 (a few per stream) |
| What checks a payment | Every validator, with a signature | The counterparty, with a signature | The payee, with a hash |
| Directions | Any | Both | Payer to payee |
| Disputes | None | A challenge period, often watchtowers | None: the ledger pays the highest position proven |
| Post-quantum | With post-quantum signatures | With post-quantum signatures, per update | By construction: hash functions only |
| Checkable later by a third party | Yes | Only the final state | Yes, every payment |

## Try it

- `sion demo streams --local` runs an agent paying an API 2,000 times over
  HTTP from one stream, then settles, audits a receipt and closes.
- [Streams from the command line](../guides/streams.md) walks through
  opening, paying, checking, settling and closing a stream by hand.
- [x402](../guides/x402.md#the-stream-scheme-one-hash-per-request) shows
  the `stream` scheme for paid HTTP APIs.
