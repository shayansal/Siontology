# Light-client verification

A light client checks what a node tells it, with proofs, against one value
it pinned: the [chain identity](../reference/chain-identity.md).
[Verify as a light client](../getting-started/light-client.md) explains
`sion verify` line by line. This guide covers the other checks, the
options, and what happens when a check cannot be made.

## The commands

| Command | Checks |
|---|---|
| `sion verify [--chain] [--pulse] [--samples] [--no-world]` | A whole Pulse: the chain, the committees, the certificate, forced inclusion, receipts, a state proof, data availability, the World Proof and lane certificates |
| `sion receipt <digest> [--chain]` | One transaction: its Pulse's certificate, its place in the certified order, and its receipt proven against the receipts root |
| MCP `sion_verify_pulse`, `sion_fetch_receipt` | The same, for agents |
| `sion_light` (Rust) | The building blocks: `ChainSpec`, `EpochChain`, `verify_certificate`, `verify_object`, `verify_claim`, `verify_receipt`, `availability::sample` |

Pin the identity once per shell with `export SION_CHAIN=<identity>`, or
pass `--chain` each time.

## Receipts

```text
$ sion receipt 0b4b87c8ed9db1d35d2548e9dfa2d7c529c680a2cd264d9cfcd683f85b897d39
Pulse 5: Done: paid 12.5 USD to alice; fee 0.0012 USD.
  certified by 300000 of the committee's stake (quorum 266667), dual post-quantum signatures ✓
  receipt proven against the certified header's receipts root ✓
  chain 6345655c (NOT pinned: pass --chain to pin it)
```

With `--chain`, the last line reads `chain 6345655c (pinned)`. A transaction
that was never certified cannot have a receipt:

```text
$ sion receipt 8f26f159c0723de6c19a5e1f2fa4f8ea5a2e2ec2eace19d8ffe823db99df69e5
error: verification failed: never certified: object ObjectRef { id: ObjectId(27657e3b), version: Version(0) } is locked to transaction TxDigest(93d6ac50)
```

## Objects and claims

Every object lives in the sparse Merkle state tree, and every claim in the
knowledge log. The node serves both with proofs:

- `GET /v1/objects/{id}` returns the object (or `null`) with a proof
  against the latest state root. A proof for a missing object proves its
  absence.
- `GET /v1/claims/{id}` returns the claim with its inclusion proof in the
  knowledge log.

`sion verify` checks an account object against the Pulse's state root when
the node's latest state is the Pulse being verified:

```text
state: an account object proven against the state root ✓
```

When the node has already moved on, it says so instead:

```text
state: proof skipped (node already at Pulse 7)
```

In Rust, `sion_light::verify_object(header, proof)` and
`sion_light::verify_claim(header, proof)` check such proofs against a
certified header.

## Data availability

`sion verify` samples random shares of the Pulse's extended data square,
at positions it draws from fresh randomness, and checks each against the
certified data root. More samples, more confidence:

```text
$ sion verify --chain 6345655cfd2a714ed1dab69c0660f2f48f429ce2981e25629d0467d7b7ee97b7 --no-world --samples 30
…
availability: 30 of 30 random shares of the 32×32 square verified; a withholding would go unnoticed with probability < 4.0e-5 ✓
```

The bound is `(3/4)^s` for `s` samples: to prevent reconstruction, a
withholder must hide at least a quarter of the extended square. See
[Data availability](../concepts/data-availability.md).

## Across epochs

On a devnet whose validator set changes, `sion verify` follows the
committee from epoch 0 through every boundary, using `GET /v1/epochs/{e}`,
before it checks a certificate:

```text
committees: followed from the pinned epoch 0 to epoch 1 through 1 certified boundaries and validator-set proofs ✓
certificate: 350000 of 450000 stake signed (quorum 300001), each signature ML-DSA-44 + SLH-DSA-SHA2-128f ✓
```

See [Staking and epochs](staking.md#verify-across-epochs).

## Choosing a Pulse

By default `sion verify` takes the most recent certified Pulse that
executed transactions (and, unless `--no-world`, has a World Proof),
looking back up to 32 Pulses, and otherwise the latest certified one.
`--pulse <n>` picks one. A node keeps headers and receipts for the last 256
Pulses, data shares for the last 64 and World Proofs for the last 32:

```text
$ sion pulse 99999
error: unknown or pruned Pulse
```

## When a check cannot be made

Without a prover (`sion devnet --no-prove`) there are no World Proofs, and
`sion verify` without `--no-world` fails with a misleading message:

```text
$ sion verify
error: verification failed: no certified Pulse yet
$ sion verify --no-world
chain: 6345655c (NOT PINNED: learned from the node itself; pass --chain 6345655cfd2a714ed1dab69c0660f2f48f429ce2981e25629d0467d7b7ee97b7 to pin it)
Pulse 2 header 9b0f5f15 (state root f4149220, knowledge root b3950ae7, 0 claims)
certificate: 300000 of 400000 stake signed (quorum 266667), each signature ML-DSA-44 + SLH-DSA-SHA2-128f ✓
…
```

A node that serves another chain than the pinned one fails the first check:

```text
$ sion verify --chain 0000000000000000000000000000000000000000000000000000000000000000
error: verification failed: header chain: the node serves chain 6345655c, not the pinned chain 00000000
```

On a devnet started with `--insecure-fast-signatures`, certificates are
simulation MACs that only the devnet can check. The chain identity records
the mode, and verification reports the certificate check as skipped rather
than passed. That devnet closes Pulses so fast (about 300 a second on the
machine used for this book) that the CLI's 20-Pulse expiry and the 256-Pulse
history are useless:

```text
$ sion send dana alice 1
dana signs: Pay 1 USD to alice. Fee 0.0012 USD; valid until Pulse 1332.
error: not certified: the transaction has expired
$ sion verify --no-world
error: unknown or pruned Pulse
```

## What it costs

On the machine used for this book, `sion verify` took 3.5 to 6.4 seconds
for a one-transaction Pulse with its World Proof, and 3.7 seconds without
it; for so small a Pulse the proof is not the main cost. A World Proof is a
replay proof, though: for a busy Pulse, re-verifying it costs as much as
executing it (`impl:replay-proofs`). See [Benchmarks](benchmarks.md).
