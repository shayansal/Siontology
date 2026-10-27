# The World Proof format

A World Proof shows that one Pulse's transition is valid and binds what a
light client relies on: the receipts, the published data and the lane
certificates. This page describes the structure (`sion_prover::WorldProof`)
and the checks that make it valid. The concepts are in
[The Fold and World Proofs](../concepts/the-fold.md); the normative rules
are §8 and §9 of the [specification](../specification.md).

`GET /v1/pulses/{n}/world` returns the proof as hex of its Borsh encoding,
with a summary ([HTTP API](http-api.md#get-v1pulsesnworld)).

## Structure

```rust
pub struct WorldProof {
    pub pulse: Pulse,                    // the Pulse proven
    pub before: WorldCommitment,         // the world before it
    pub after: WorldCommitment,          // the world after it
    pub receipts_root: Hash32,           // Merkle root of the transaction effects
    pub receipt_leaves: Vec<Hash32>,     // one leaf per transaction, in execution order
    pub statements: Vec<ChunkStatement>, // one per chunk, the deferred phase last
    pub statements_root: Hash32,         // Merkle root over the statements' digests
    pub proofs: Vec<Vec<LeafProof>>,     // for each statement, one leaf proof per stack
    pub data: PulseData,                 // the Pulse's published data
    pub data_root: Hash32,               // its availability root (2D Reed–Solomon)
    pub data_k: u32,                     // width k of the data square
    pub chunk_sizes: Vec<u32>,           // transactions per transaction chunk
    pub certificates: Vec<LaneCertificate>, // one per executed transaction
    pub validators: ValidatorsWitness,   // the validator set when the Pulse began, with its state proof
}
```

| Part | Fields |
|---|---|
| `WorldCommitment` | `pulse`, `state_root`, `knowledge_root`, `knowledge_size`, `config` (the chain configuration's digest) |
| `ChunkStatement` | `pulse`, `beacon`, `index`, `deferred_phase`, `before` and `after` (world commitments), `deferred_in` and `deferred_out` (the deferred-items accumulator), `effects_in` and `effects_out` (the receipts accumulator), `steps` (the digest of the transactions executed, in order) |
| `LeafProof` | `stack` (`"A"` or `"B"`), `statement` (the digest of the statement it proves), `body` (stack-specific bytes) |
| `PulseData` | `pulse`, `beacon`, `transactions` (bodies in execution order, without signatures), `forced` (the forced intents the Pulse posted) |
| `ValidatorsWitness` | `object` (the validator-set object), `proof` (a sparse-Merkle proof against `before.state_root`) |

With both replay stacks, a leaf proof's body is the chunk's witness and
transactions: every object the chunk touches with a sparse-Merkle proof,
and the knowledge log's frontier. That is why a proof's size grows with the
Pulse.

## When it is valid

A World Proof for Pulse `p` is valid if and only if (§9):

1. It has at least one statement; statement `i` has `index = i` and
   `pulse = p`; exactly the last is the deferred phase.
2. The first statement starts at `before` and the last ends at `after`;
   `before.pulse = p − 1` and `after.pulse = p`.
3. Consecutive statements chain: each one's `before`, `deferred_in`,
   `effects_in` and `beacon` equal the previous one's `after`,
   `deferred_out`, `effects_out` and `beacon`; the first one's accumulators
   start at zero. So the deferred phase consumes exactly what the
   transaction chunks deferred.
4. Folding the receipt leaves gives the last statement's `effects_out`, and
   their Merkle root is `receipts_root`.
5. The published data is for Pulse `p` with the statements' beacon; the
   transaction chunks' `steps` cover the published transactions, in
   consecutive runs of `chunk_sizes`; the deferred phase's `steps` cover the
   published forced intents; and the data encodes to `data_root` and
   `data_k`.
6. Every leaf proof verifies under every required stack.

`statements_root` must also be the Merkle root over the statements'
digests.

Whoever relies on the proof checks it against the certified Pulse header:
the same `pulse`, `before`, `after`, `receipts_root`, `data_root`, `data_k`
and beacon. And the lane certificates: the validator set in
`validators` must verify against `before.state_root`, and each executed
transaction's certificate must lock exactly its inputs with a quorum of its
shard's lane committee, derived from that set
(`sion_prover::verify_certificates`).

## What it is not

- **Not succinct.** Both stacks replay; verifying a leaf costs as much as
  executing its chunk (`impl:replay-proofs`).
- **Not recursive.** "Folding" is a Merkle tree over statements, not
  proof composition. A client verifies a Pulse at a time, not the whole
  history in one check.
- **Not a header check.** The World Proof does not verify the Pulse
  header's committee certificate or its forced-inclusion summary; the light
  client checks the certificate separately, against the committee it
  follows from the pinned identity.

## Checking one

`sion verify` downloads the proof of the Pulse it checks, re-verifies every
leaf with both stacks, and checks the bindings and the certificates:

```text
world proof: 2 leaves re-verified by stack ["A", "B"]; the published data is what executed and encodes to the certified data root (113 KB; replay proofs simulate succinct ones) ✓
lane certificates: all 1 executed transactions locked their inputs with a quorum of their shard's committee, taken from the proven validator set ✓
```

`sion_prover::verify_world_proof_sampled(proof, stacks, samples, seed)`
re-verifies a random subset of leaves (always including the deferred
phase), which catches one invalid leaf with probability
`samples / leaves`.
