# Data availability

A validity proof shows that a transition was correct. It cannot show that
the transition's data was published. If a Pulse's data is withheld, nobody
can rebuild the state or file a challenge. The data-availability layer makes
withholding detectable by everyone.

## How it works

1. **Lay out.** Each Pulse's data is laid out as a `k × k` square of field
   elements (the Goldilocks field).
2. **Extend.** The square is extended with a two-dimensional Reed–Solomon
   code to `2k × 2k`. Any half of a row or column rebuilds it, so making the
   data unrecoverable means withholding at least `(k + 1)²` shares: more
   than a quarter of the extended square.
3. **Commit.** Every row and column is committed with a SHA3 Merkle root,
   and a data root commits to all of them. The commitments are hash-based:
   no trusted setup and no pairings, nothing a quantum computer breaks.
4. **Sample.** A light client samples *s* random shares and checks each
   against the data root. Each sample hits a withheld share with probability
   above 1/4, so a client that gets every sample back is fooled with
   probability at most

   ```text
   Pr[withholding goes undetected by one client] ≤ (3/4)^s
   ```

   About 1% at *s* = 16 and about 0.02% at *s* = 30. Thousands of
   independent samplers make undetected withholding vanishingly unlikely,
   provided their requests cannot be linked.
5. **Rebuild.** Anyone holding enough shares reconstructs the square by
   iterative row and column decoding.
6. **Prove the encoding.** Hash commitments do not show that the square is
   a correct Reed–Solomon encoding. In the architecture the World Proof
   proves it (in the style of ZODA); compact fraud proofs are the fallback.

## What is published

A Pulse's published data is its **transaction bodies, in execution order,
without signatures**. Validators verify signatures and the World Proof
covers those checks, so the data layer never stores a signature. That
matters: a dual post-quantum signature is about 19.5 KB, while a transfer's
body is a few hundred bytes. On the devnet used for this book, an idle Pulse
published 48 bytes, laid out in a 4 × 4 square.

The certified Pulse header carries the data root and the square's width
`k`. Validators never sign a header without an availability commitment.

## Sampling from the command line

`sion verify` samples 16 shares by default; `--samples` changes it. It draws
positions from fresh operating-system randomness mixed with the data root,
so the node cannot predict them. A real run on the devnet:

```text
$ sion verify --chain 6345655cfd2a714ed1dab69c0660f2f48f429ce2981e25629d0467d7b7ee97b7 --no-world --samples 30
chain: 6345655c matches the pinned identity; 4 validators, keys derive their identifiers ✓
Pulse 5 header fc972683 (state root 04ed71f4, knowledge root b3950ae7, 0 claims)
certificate: 300000 of 400000 stake signed (quorum 266667), each signature ML-DSA-44 + SLH-DSA-SHA2-128f ✓
forced inclusion: no overdue intents (0 pending within their deadline) ✓
receipts: 1 of 1 transactions proven against the receipts root, in the certified order ✓
state: proof skipped (node already at Pulse 8)
availability: 30 of 30 random shares of the 32×32 square verified; a withholding would go unnoticed with probability < 4.0e-5 ✓
```

The square in the output is the extended one. The printed probability is
computed for its actual size, so it is smaller than the `(3/4)^s` bound.

The HTTP API serves the axis roots (`GET /v1/pulses/{n}/data`) and single
shares with their Merkle proofs (`GET /v1/pulses/{n}/data/{row}/{column}`);
see the [HTTP API](../reference/http-api.md).

## Retention

In the architecture:

- **Hot window.** Pulse data stays available for at least the longest claim
  window plus its challenge period, so anyone can rebuild state and file
  challenges.
- **Archive.** After the window, data moves to paid archivers who post
  storage proofs. Validity never depends on archives, because the World
  Proof already covers the past.
- **Owner-held data.** A private claim may keep its payload with its owner
  and publish only a commitment; availability is then the owner's risk.

In this implementation, validators keep a write-ahead log of every executed
Pulse and periodic snapshots. In memory, a validator keeps Pulse summaries
and data-availability headers for the last 256 Pulses, and the published
data from which it re-derives shares for the last 64 Pulses. Older shares
are answered with `data pruned`. There is no archival tier and no incentive
to keep shares.

## In this implementation

- **Built**: the 2D Reed–Solomon code over the Goldilocks field, row and
  column commitments, sampling, reconstruction, bad-encoding fraud proofs
  (as a library), and a World Proof check that the published data encodes
  to the certified data root.
- **Simplified**: retention (see above).
- **Not built**: light clients that store and serve the shares they
  sampled, unlinkable sample requests, paid archivers with storage proofs,
  committee-attested data for lanes that cannot afford sampled
  availability.
