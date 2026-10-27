# Two-sorted state

Every operation either **adds knowledge** or **consumes a resource**, and
only consumption ever needs coordination. Siontology makes that split part
of its state model and pays for ordering only where the mathematics requires
it. The architecture paper calls this the **Coordination Rule**.

## Why it holds

- **CALM.** A distributed program has a consistent, coordination-free
  implementation exactly when it is monotone: new inputs can add outputs but
  never retract them
  ([Hellerstein and Alvaro](https://arxiv.org/abs/1901.01930)). Publishing a
  verified claim is monotone: it never invalidates another claim. Spending a
  coin is not: it removes the possibility of a second spend.
- **Linear logic.** A linear resource is used exactly once; a fact can be
  reused any number of times
  ([Girard, 1987](https://doi.org/10.1016/0304-3975(87)90045-4)).
- **Consensus numbers.** Transfers between single-owner accounts need only
  reliable broadcast, not consensus
  ([Guerraoui et al., 2019](https://arxiv.org/abs/1906.05574)). An object
  with *k* writers needs consensus among *k*, so shared accounts, revocable
  Charters and recovery policies count as contended.

Judgments about claims are not monotone: whether a claim is still trusted
changes when a challenge lands. They are evaluated against Pulse snapshots,
which every validator computes identically.

## Four coordination classes

| Class | Example | What must be prevented | Mechanism |
|---|---|---|---|
| Knowledge | Issue a claim, register an atom, post a bond | Nothing: claims never conflict | Data availability; the knowledge log |
| Owned linear | Pay from your balance | The owner signing two spends of one version | A lane certificate ([Linear Lanes](linear-lanes.md)) |
| Shared, bounded | A budget, seats, an agent's allowance | Overdrawing a shared total | Single-writer allotments ([Escrow Lanes](escrow-lanes.md)) |
| Shared, contended | A market batch, the assurance registry, a charter's revocation | Conflicting updates to one object | Consensus in the object's contention domain ([Contention Consensus](contention-consensus.md)) |

## Objects and the knowledge log

The state has two sorts:

- **Objects.** Every piece of mutable state is an object with an
  identifier, a version, an owner and data. The owner is either one account
  (`Owner::Account`) or a contention domain (`Owner::Shared`). Coins,
  accounts, allotments, bounded totals, charters, markets, the assurance
  registry, the treasury and the protocol schedule are all objects. Every
  write increments the version.
- **Knowledge.** Claims are not objects. They are appended to a grow-only
  Merkle log (RFC 9162 style), whose root and size are part of every world
  commitment.

Nothing is ever deleted. A consumed object is replaced by a **tombstone**
that keeps its identifier, owner and version and records which transaction
consumed it and when. History stays auditable, and proofs stay stateless.

The **state root** is a sparse Merkle tree mapping each object identifier
to the object's digest. The **world commitment** binds the Pulse number, the
state root, the knowledge root, the knowledge log's size and the digest of
the chain configuration.

## The class is derived, never declared

A sender cannot choose a cheaper class than its transaction needs. The
class is computed from the transaction kind (`Transaction::class` in
`sion-exec`):

| Class | Transaction kinds |
|---|---|
| Knowledge | `IssueClaim`, `RegisterAtom`, `PostBond` |
| Owned linear | `Transfer`, `Faucet`, `RotatePolicy`, `CreateCharter`, `CreateBounded`, `CreateMarket`, `SubmitOrder` |
| Bounded | `AgentPay`, `Delegate`, `SpendAllotment`, `ReturnAllotment` |
| Contended | `Accept`, `Release`, `Challenge`, `Rule`, `WithdrawBond` (the assurance domain); `RevokeCharter`, `ReclaimCharter` (the charter's domain); `Rebalance` (the bounded total's domain); `RevealBatch`, `ClearBatch` (the market's domain) |

Every transaction names at least one owned input, its **gas coin**, which
pays the fee and gives replay protection: an object version can be consumed
by one transaction only. Every transaction, contended ones included, first
locks its owned inputs with a lane certificate. A contended transaction is
then also ordered by consensus in its domain.

Knowledge-class effects (claims, atoms, bonds) and market orders do not
write shared objects directly. They are **deferred** to the end of the
Pulse, where they are applied in a canonical order (see [Pulses](pulses.md)).
That is why they can travel on the owned path: adding to a set commutes.

## In this implementation

- **Built**: two-sorted state, tombstones, the sparse Merkle state root, the
  RFC 9162 knowledge log, derived classes.
- **Not built**: the architecture's contract language with sorts
  (`resource`, `fact`, `bounded`, `shared`), in which a compiler derives
  each operation's class from its types. The implementation has a fixed set
  of transaction kinds instead; see [Transaction kinds](../reference/transactions.md).
- **Different from the first plan**: shards exist for lane certification
  only. Every validator executes every Pulse in full, so there are no
  cross-shard messages. The implementation plan explains this choice:
  proving, not sharded execution, is how the architecture scales execution.
