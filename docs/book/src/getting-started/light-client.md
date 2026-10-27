# Verify as a light client

A light client follows the chain without executing it. It pins one value,
the **chain identity**, and checks everything else against it with proofs.

## Pin the chain identity

The chain identity is a digest of everything that decides who certifies
the chain's first epoch: the genesis specification, the genesis validators
with their stake and public keys, and the shard layout. Later committees
are in the state, and the client follows them from there. `sion devnet`
prints the identity at startup:

```text
chain identity (pin it: sion verify --chain …): 6345655cfd2a714ed1dab69c0660f2f48f429ce2981e25629d0467d7b7ee97b7
```

A real client gets it from a source it trusts (a release, a friend, a
second node), like an SSH host key. Without a pin, the client learns the
identity from the node it is checking, and the node vouches for itself.

## Verify a Pulse

```text
$ sion verify --chain 6345655cfd2a714ed1dab69c0660f2f48f429ce2981e25629d0467d7b7ee97b7
chain: 6345655c matches the pinned identity; 4 validators, keys derive their identifiers ✓
Pulse 5 header fc972683 (state root 04ed71f4, knowledge root b3950ae7, 0 claims)
certificate: 300000 of 400000 stake signed (quorum 266667), each signature ML-DSA-44 + SLH-DSA-SHA2-128f ✓
forced inclusion: no overdue intents (0 pending within their deadline) ✓
receipts: 1 of 1 transactions proven against the receipts root, in the certified order ✓
state: proof skipped (node already at Pulse 7)
availability: 16 of 16 random shares of the 32×32 square verified; a withholding would go unnoticed with probability < 4.7e-3 ✓
world proof: 2 leaves re-verified by stack ["A", "B"]; the published data is what executed and encodes to the certified data root (113 KB; replay proofs simulate succinct ones) ✓
lane certificates: all 1 executed transactions locked their inputs with a quorum of their shard's committee, taken from the proven validator set ✓
```

Line by line:

| Line | What was checked |
|---|---|
| `chain` | The node's chain specification hashes to the pinned identity, every validator is listed once with positive stake, and every validator identifier derives from its two public keys |
| `Pulse … header` | The Pulse chosen: by default the most recent certified (and, unless `--no-world`, proven) Pulse that executed transactions, looking back up to 32 Pulses; otherwise the latest certified one |
| `committees` | Only for a Pulse after epoch 0: the client followed the committee from the pinned epoch 0 to the header's epoch, one certified boundary and validator-set proof at a time ([Epochs and committees](../concepts/validators/epochs.md)) |
| `certificate` | Validators holding more than two thirds of the epoch's committee stake signed this header, each with both signatures. Epoch 0's committee comes from the pinned specification, later ones from the boundary proofs, never from the node's word |
| `forced inclusion` | What the certified header says the shards owe |
| `receipts` | The node's list of executed transactions hashes to the order the header committed to; up to three receipts are proven at their positions against the receipts root |
| `state` | An account object proven against the header's state root, when the node's latest state is this Pulse; otherwise skipped |
| `availability` | Random shares of the Pulse's extended data square, at positions the node cannot predict, verified against the certified data root |
| `world proof` | The World Proof matches the header, and every leaf re-verifies under both proof stacks |
| `lane certificates` | Every executed transaction carried a lane certificate from a quorum of its shard's committee, derived from the validator set the World Proof carries |

If any check fails, `sion verify` stops with `verification failed: …` and
exits with status 1.

Options:

- `--pulse <n>`: verify a specific Pulse. The node keeps Pulse headers for
  the last 256 Pulses, data shares for the last 64 and World Proofs for the
  last 32.
- `--samples <s>`: data-availability samples (default 16).
- `--no-world`: skip the World Proof. A World Proof is a replay proof,
  so checking it costs as much as executing the Pulse.

## Without a pin

```text
$ sion verify
chain: 6345655c (NOT PINNED: learned from the node itself; pass --chain 6345655cfd2a714ed1dab69c0660f2f48f429ce2981e25629d0467d7b7ee97b7 to pin it)
...
```

Everything else is checked the same way, but against a committee the node
chose. Set `SION_CHAIN` to pin it for every command.

## A lying node is caught

The `light` demo builds the attack a lying node would need: serving its own
keys under a real validator's identifier. The pin catches it:

```text
a node serving substituted validator keys is caught: header chain: the node serves chain 6fc4795f, not the pinned chain 6345655c ✗
```

The substituted keys change the specification the node serves, so it no
longer hashes to the pinned identity.

## What a light client still trusts

- Fewer than a third of the committee's stake is Byzantine, for the
  certificate.
- The World Proof is a replay proof (`impl:replay-proofs`): verifying it is
  not cheap, and a phone cannot do it for a busy Pulse.
- The forced-inclusion summary in the header is committee-certified; the
  World Proof proves the obligations themselves but does not check the
  summary against them.

See [Light-client verification](../guides/light-client.md) for receipts,
claims and objects, and [Trust model](../security/trust-model.md).
