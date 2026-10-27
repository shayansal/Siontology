# Pulses

A **Pulse** is the network's heartbeat: the unit in which transactions
execute, a header is certified, data is published and a World Proof is
built. Pulses are numbered from 1; the genesis state is Pulse 0.

## When a Pulse closes

Consensus produces a sequence of committed anchors with increasing round
numbers (see [Contention Consensus](contention-consensus.md)). Pulse *p* is
everything committed by anchors in rounds `((p − base − 1)·K, (p − base)·K]`,
where `K` is the number of rounds per Pulse (`--rounds-per-pulse`, default
4) and `base` is the last Pulse executed before the current consensus
session began. A Pulse closes as soon as an anchor beyond its last round
commits.

A session starts at every epoch boundary and at every restart. A session
closes no Pulse after its epoch's last Pulse; what it commits later is
proposed again in the next session (see
[Epochs and committees](validators/epochs.md)).

Every honest validator commits the same anchors in the same order, so every
validator closes the same Pulses with the same contents, with no extra
agreement step. How long a Pulse takes in wall-clock time depends on how
fast consensus rounds advance.

## The beacon

Each Pulse has a **beacon** value: public randomness nobody could predict
before the Pulse closed. Each validator commits to the tip of a private
hash chain, at genesis or when it registers, and while it is in the active
set reveals one preimage per Pulse; the Pulse's beacon hashes the previous
beacon with every valid reveal, in validator order. It is unpredictable as
long as one revealer is honest.

Markets seal their batches with the beacon, and clearing breaks ties with
priorities derived from it, so nobody can buy or predict a position. Each
epoch's lane committees are sampled with the beacon of the previous
epoch's last Pulse, and a shard's replacement committee with the beacon of
the Pulse that replaces it. Its known weakness is recorded as the atom
`impl:hash-chain-beacon`: the last validator to reveal can withhold its
reveal and so choose between two outputs, at the cost of being seen.

## Three phases

Executing a Pulse runs three phases:

1. **Owned.** Certified owned-path transactions (owned-linear, bounded and
   knowledge classes). Each owned input is locked to one transaction by its
   lane certificate, and no owned-path operation writes a pre-existing
   shared object, so these transactions commute. The canonical order is
   transaction digest order, in passes: a transaction whose inputs are
   created by another transaction of the same Pulse waits for a later pass.
2. **Contended.** Consensus-ordered transactions, in commit order.
3. **Deferred.** The commutative effects collected from the first two
   phases, sorted canonically: atom registrations, bonds, claims (by claim
   identifier), market orders, returns to bounded totals, fees to the
   treasury, mint records and schedule entries. Then the forced intents
   committed in the Pulse are posted as obligations of their senders'
   shards ([Censorship resistance](censorship-resistance.md)). Then
   whatever the protocol schedule says is due: sealing market batches,
   refunding batches nobody cleared in time, registry ticks that expire
   claims and release reservations, forced-inclusion deadlines (which can
   replace a shard's committee), and the end of an epoch (rewards, joins
   and exits, the next active set; see
   [Epochs and committees](validators/epochs.md)).

Because claims may reference only claims from earlier Pulses, and deferred
items are applied in a canonical order, the knowledge log never depends on
the order of transactions within a Pulse.

## Three outcomes

Each transaction ends in one of three outcomes, and the receipt commits to
which:

| Outcome | When | What is kept |
|---|---|---|
| **Success** | Every check and the operation succeeded | All writes; the fee is charged |
| **Failed** | Preparation passed but the operation failed (for example a charter caveat refused the payment) | Only the fee payment, the new account object if this was the sender's first transaction, and the version bumps of the inputs; events are emptied |
| **Rejected** | Preparation failed (expired, bad signature, wrong input version, fee too low) | Nothing; no fee is charged |

Failure messages are not committed; only the outcome code is. Two correct
implementations may word an error differently.

## Chunks

For proving, a Pulse's execution is cut into **chunks**: consecutive runs of
transactions (16 per chunk by default, `--chunk`) and, last, the deferred
phase. Each chunk has a **statement**: the world commitments before and
after, accumulators of deferred items and receipts, and the digest of the
transactions it executed. Each chunk also has a **witness**: every object
it touches, with a sparse-Merkle proof against the starting state root, and
the knowledge log's frontier. A chunk can be re-executed from its witness
alone. See [The Fold and World Proofs](the-fold.md).

## The Pulse header

After executing a Pulse, every validator builds the same header:

| Field | Meaning |
|---|---|
| `chain` | The [chain identity](../reference/chain-identity.md) |
| `epoch` | The validator-set epoch, whose committee signs the header |
| `pulse` | The Pulse number |
| `beacon` | The Pulse's beacon value |
| `before`, `after` | World commitments before and after |
| `order_digest` | Digest of the executed order of transactions |
| `receipts_root` | Merkle root over every transaction's effects |
| `data_root`, `data_k` | The data-availability root of the published data and the square's width |
| `forced` | Forced-inclusion obligations after this Pulse: pending, overdue, and a root over the overdue ones |

Each validator signs the header and sends its signature to the others. A
header signed by validators holding more than two thirds of the active
set's stake is **certified**. A validator never signs a header whose data has no
availability commitment.

## In this implementation

- **Built**: Pulses on round boundaries, the hash-chain beacon, the three
  phases, outcome codes, chunk statements and witnesses, header
  certificates.
- **Simplified**: every validator executes every Pulse in full; the
  architecture's maturity target of a World Proof per second, with proof
  finality a few seconds behind, is not measured here.
