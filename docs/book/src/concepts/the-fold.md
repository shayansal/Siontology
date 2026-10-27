# The Fold and World Proofs

Every Pulse, the network folds all the proofs produced since the last Pulse
into one **World Proof** that certifies the entire history from genesis.
Checking it should cost the same in year ten as on day one, so a phone is as
certain of the state's validity as a datacenter. That is the design. This
chapter describes it, and then what the implementation does today, which is
less.

## What a World Proof states

In the architecture, the World Proof for Pulse *t* proves that a history
exists from genesis to the state root *Rₜ* in which:

1. every linear transition followed its object's rules and carried a valid
   committee certificate;
2. every contended batch cleared under a valid optimality certificate;
3. the knowledge root only grew, and every claim's trust formula and bond
   references are correct;
4. every forced-inclusion obligation older than *k* Pulses was met;
5. availability certificates exist for all data the history depends on;
6. every committee change followed the staking rules, so the chain of
   signers is valid from genesis.

## How it is built, in the architecture

- **Leaves in parallel.** Each lane batch, contention-domain batch and claim
  batch gets its own proof from whichever prover wins the job.
- **A tree of aggregation.** Provers fold or recursively aggregate proofs
  pairwise; a million leaves need about 20 levels.
- **Pipelining.** Pulse *t* + 1 starts aggregating while Pulse *t*'s root
  finishes.
- **Two proof trees, end to end.** Two independent proving stacks, with
  different code and different arithmetizations, each prove every leaf and
  every level. A World Proof is the pair of roots, and both must verify. A
  soundness bug in one stack cannot forge the world. A bug in the shared
  specification still could, so the specification is itself an atom.

The trust formula of a dual proof is:

```text
specification ∧ sha3-256 ∧ ((stack A ∧ circuit A) ∨ (stack B ∧ circuit B))
```

Proving is hash-based throughout (STARKs at launch, lattice folding later),
so the stack stays post-quantum.

## What the implementation does

### Chunk statements

Execution of a Pulse is cut into chunks (see [Pulses](pulses.md)). Each
chunk has a statement: "these transactions take the world from commitment
*before* to commitment *after*, extending these accumulators". Each chunk
also has a witness: every object it touches, with a sparse-Merkle proof
against the starting state root, plus the knowledge log's frontier. Reading
an object outside the witness rejects the whole chunk.

### Two proof stacks, both replay

| Stack | Code | How it checks a chunk |
|---|---|---|
| A | `sion_prover::ReplayStack` | Re-executes the chunk statelessly with the reference executor (`sion-exec`) |
| B | `sion_prover::SpecStack` | Re-executes it with an independent implementation written only from the [specification](../specification.md) (`sion-spec`) |

A leaf proof is the chunk's witness and transactions, and verifying it
means re-executing the chunk. Both stacks are labelled `simulated: true` in
the types and in every report. This is sound (a replay proof of a false
statement does not verify) but not succinct: a proof is as large as its
witness, and checking it costs as much as execution. A STARK stack would
plug into the same `ProofStack` trait without changing the statements, the
chaining or the folding.

This is **N-version proving**: a World Proof verifies only if both stacks
accept every chunk. Differential tests run thousands of chunks, random
workloads, boundary probes and tampered traces through both stacks. They
found two defects in the reference executor and several gaps in the
specification, all fixed in specification v1.1. When stack B implemented
the validator set (v1.3), the stacks disagreed once: about whether an
epoch's end writes the treasury's USD entry when the treasury had none and
nothing is paid. The specification now says it always does. See
[Keeping proof stack B independent](../architecture/independence.md).

### Folding

For every Pulse, the prover builds a World Proof that binds:

- **Execution.** The statements chain from the Pulse's first commitment to
  its last; the deferred phase consumes exactly what the transaction chunks
  deferred; every leaf verifies under every stack.
- **Receipts.** The receipts accumulator over every transaction's effects
  matches, and their Merkle root is the header's receipts root.
- **Data.** The Pulse's published data (transaction bodies in execution
  order, and the forced intents the Pulse posted) is exactly what
  executed, and its two-dimensional Reed–Solomon encoding has the certified
  data root. Sampling against that root therefore samples a correct
  encoding.
- **Certificates.** Every executed transaction carried a lane certificate
  from its shard's committee, locking its owned inputs. The proof carries
  the validator set the Pulse began with, with a state proof against the
  Pulse's starting state root, and the certificates are checked against
  the lane committees derived from it.

"Folding" here is a Merkle tree over the leaf statements, not recursive
proof composition. The proof's size grows with the Pulse's witnesses.

### Against the six statements

| Architecture statement | Here |
|---|---|
| 1. Linear transitions followed the rules, with committee certificates | Yes (replay of every chunk, lane certificates checked) |
| 2. Contended batches cleared under optimality certificates | Yes (clearing certificates are checked by execution, which both stacks replay) |
| 3. The knowledge root only grew; formulas and bond references correct | Yes (by execution) |
| 4. Forced-inclusion obligations were met | Partly. Obligations, deadlines and committee rotations are part of the state transition, so both stacks execute them and the proof covers them. A shard that misses a deadline gets a new committee but is not frozen. The summary in the Pulse header is certified by the committee, not checked against the state by the proof; see [Censorship resistance](censorship-resistance.md) |
| 5. Availability of the data | Partly: the published data encodes to the certified data root |
| 6. Committee changes followed the staking rules | Yes. The validator set is in the state and changes only by the staking rules, which both stacks execute. The proof carries the validator set the Pulse began with, proven against its starting state root, and lane certificates are checked against committees derived from it; see [Epochs and committees](validators/epochs.md) |

### Who verifies

- The devnet's prover thread proves and verifies every Pulse it receives.
- `sion verify` downloads a World Proof and re-verifies every leaf with
  both stacks, then checks it matches the certified header.
- `verify_world_proof_sampled` re-executes a random subset of leaves, which
  detects one invalid leaf with probability `samples / leaves`.

## Cadence

The architecture targets a World Proof every 10 to 30 seconds at launch and
every second at maturity. The devnet tries to prove every Pulse. When the
prover falls more than 16 Pulses behind, the newest Pulses go unproven
rather than queueing without bound, and `proven_pulse` in `sion status`
lags.

## In this implementation

- **Simulated**: succinct proofs (atom `impl:replay-proofs`), the proving
  market, recursion.
- **Built**: chunk statements and witnesses, two independent stacks, the
  bindings to receipts, data and certificates.
- **Shared code**: both stacks use the same hashing, Merkle, signature,
  Assurance Algebra and clearing-certificate code. A bug there is
  common-mode (atom `impl:shared-primitives`).
