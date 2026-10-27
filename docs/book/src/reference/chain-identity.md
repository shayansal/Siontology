# The chain identity

A certificate is only as good as the committee it is checked against, so a
light client must never take the committee from the node it is checking.
The **chain identity** is the one value a client pins, out of band, like an
SSH host key. Everything else it checks follows from it.

## What it covers

The identity is a domain-separated SHA3-256 hash of the Borsh encoding of a
`ChainSpec` (`sion_light::ChainSpec::id`):

| Field | Meaning |
|---|---|
| `genesis` | Digest of the genesis specification: the chain configuration, the funded accounts, the genesis validators, the validator-set parameters and the forced-inclusion parameters |
| `validators` | The genesis validators, each with its weight (stake in whole units) |
| `keys` | Their public keys (ML-DSA-44 and SLH-DSA); each identifier is derived from its keys |
| `shards` | The number of lane shards |
| `seats` | Seats per lane committee |
| `insecure_signatures` | Whether validators sign with the insecure simulation MAC (`--insecure-fast-signatures`) |

So the identity fixes epoch 0's committee. Every later committee is in the
state, and a client follows it from epoch 0 one boundary at a time; see
[Epochs and committees](../concepts/validators/epochs.md).

Pulse headers carry the identity, lane batches and consensus headers name
it, and transaction signatures cover the chain configuration. Nothing
signed for one chain counts on another.

## Where to get it

`sion devnet` prints it at startup:

```text
Siontology devnet: 4 validators, 2 shards, API on http://127.0.0.1:8730
chain identity (pin it: sion verify --chain …): 6345655cfd2a714ed1dab69c0660f2f48f429ce2981e25629d0467d7b7ee97b7
accounts funded at genesis: alice, bob, carol
```

Any change to the genesis changes it: another chain name, other funded
accounts, another number of validators, shards or seats, other epoch,
unbonding or forced-inclusion parameters. A devnet started with
`--standby 1 --epoch-length 24 --unbonding 8` had the identity
`e2fdac57…`. Two devnets started with the same flags have the same
identity, and the same devnet keeps it across restarts.

A node also reports it: `chain` in `GET /v1/status`, and both the
`ChainSpec` (as `chain`) and its identity (as `chain_id`) in
`GET /v1/validators`. Taking it from the node you want to check proves
nothing; get it from a source you trust.

## How to pin it

Pass it to every command that verifies, or set it once:

```bash
sion verify --chain 6345655cfd2a714ed1dab69c0660f2f48f429ce2981e25629d0467d7b7ee97b7
export SION_CHAIN=6345655cfd2a714ed1dab69c0660f2f48f429ce2981e25629d0467d7b7ee97b7
```

`sion verify`, `sion receipt`, `sion x402 serve`, `sion x402 fetch` and the
MCP tools `sion_verify_pulse`, `sion_fetch_receipt` and `sion_fetch_paid`
accept it. Without it, they say the node vouched for itself:

```text
chain: 6345655c (NOT PINNED: learned from the node itself; pass --chain 6345655cfd2a714ed1dab69c0660f2f48f429ce2981e25629d0467d7b7ee97b7 to pin it)
```

With it:

```text
chain: 6345655c matches the pinned identity; 4 validators, keys derive their identifiers ✓
```

## What a client checks

1. The `ChainSpec` the node serves hashes to the pinned identity.
2. It is well formed: at least one validator, each listed once with
   positive stake; at least one shard and one seat; and, unless the chain
   uses simulation signatures, every validator identifier is derived from
   exactly the keys listed for it. A node that serves its own keys under
   honest identifiers fails here.
3. From the specification, epoch 0's committee; from boundary proofs, every
   later one.

A node serving another chain fails step 1. `sion demo light` shows a node
that substitutes validator keys being caught.
