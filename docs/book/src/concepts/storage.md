# Storage deals

A **storage deal** pays a provider, round by round, for keeping a body of
data and proving in every round that it still holds it. The ledger never
sees the data, only its Merkle root, and checks a few leaves of it per
round. Any machine with spare disk can be a provider: a desktop running
the [app](../getting-started/app.md), a server, a phone. Clients are
whoever needs data kept: an agent's [memory](memory.md), a robot's camera
footage, a dataset a task depends on.

Storage deals are in the state transition from specification v1.12
(§5.50 to §5.54), executed by both proof stacks.

## The data and its root

The data is split into **leaves** of at most 4 KiB. The deal commits to
the Merkle root over the leaves' hashes (the RFC 9162 tree the knowledge
log uses) and to how many there are. `sion storage prepare` prints both:

```text
notes.txt: 70929 bytes in 18 leaves, root 44057875c2c4475f257879ae3597177b9da0b148b962566eea127e2d131e78c8
```

The deal also carries a **locator**, where providers can fetch the data
(`file://` or `http(s)://`). Nothing on the ledger checks it: a provider
checks what it fetched against the root before it accepts.

## A deal, step by step

| Step | Transaction | Who | What happens |
|---|---|---|---|
| Post | `PostDeal` | The client | Escrows `price × rounds`; names a provider, or none for an **open** deal anyone may take |
| Accept | `AcceptDeal` | The provider | Posts the collateral; gets a **custody record** it owns. Rounds start at the next Pulse |
| Prove | `ProveStorage` | The provider | Each round, answers the round's **challenge** from its custody record |
| Collect | `SettleDeal` | The provider | Paid `price` for every proven round not yet paid |
| Close | `CloseDeal` | Either | Pays what is due, refunds the escrow of unproven rounds to the client, settles the collateral |

A deal's money stays in the deal, a shared object in its own contention
domain: the escrow, and the collateral once accepted. Proofs touch only
the provider's own custody record.

## The challenge

Each round, the provider must show `challenges` leaves (1 to 16, chosen by
the client) with their Merkle paths. Which leaves is drawn from a
**seed** in the custody record:

- the leaf of the `k`-th challenge of round `i` is the first 8 bytes of
  `H(sion/storage/challenge, seed, i, k)`, modulo the number of leaves;
- the seed is set from the beacon when the deal is accepted, and changes
  with the beacon of the Pulse every proof executes in.

So a round's challenge is fixed only once the round before it is proven,
by a beacon nobody could predict. A provider cannot keep just the leaves a
future round will ask for: it cannot know them two proofs ahead.

A provider that lost a fraction `f` of the leaves can answer a round only
if every challenged leaf is among those it kept, which happens with
probability `(1 − f)^challenges` (our arithmetic):

| Lost | 1 challenge | 4 challenges | 16 challenges |
|---|---|---|---|
| 50 % | answers 1 round in 2 | 1 in 16 | 1 in 65,536 |
| 10 % | 9 in 10 | about 2 in 3 (0.66) | about 1 in 5 (0.19) |
| 1 % | 99 in 100 | 24 in 25 | about 85 in 100 |

A provider that lost a little may go unnoticed for some rounds; one that
lost much is caught within a round or two. A client chooses more
challenges for data that matters more; each costs the provider up to about 5 KiB
of transaction (a 4 KiB leaf and its path) and the fee for it.

## Rounds, payment and collateral

A deal has `rounds` rounds of `round_length` Pulses (at least 8). A round
is proven during the round, at most once. At the close:

- the provider is paid for every round it proved;
- the client gets back the escrow of every round not proven;
- the provider gets its collateral back, less `⌊bond / rounds⌋` for each
  round it missed.

**Abandonment.** If a provider proves none of three rounds in a row, the
client may close the deal before its end, and the provider forfeits **all**
of its collateral. The provider is still paid for the rounds it proved.

Forfeited collateral goes to the **treasury**, not to the client: no one
should profit from making a provider fail, for example by flooding its
network so its proofs are late. The client is made whole on the escrow
only; collateral makes a provider's promise credible, it does not insure
the data.

An example (our arithmetic): a deal of 6 rounds at 0.5 USD with 3 USD of
collateral. A provider proving every round earns 3 USD and gets its 3 USD
back. One that proves round 0 and then stops: the client closes during
round 4, the provider is paid 0.5 USD and forfeits 3 USD, and the client
gets 2.5 USD of escrow back. The `storage` demo runs exactly this.

## Built to scale

Storage proofs are the high-volume part: one per deal per round. They go
through a **custody record the provider owns**, so proving is
owned-linear: a lane certificate, not consensus, as with a payment. One
`ProveStorage` proves many deals at once (up to 64 challenged leaves in
all), so a provider with thousands of small deals sends a few
transactions per round. Only accepting, collecting and closing go through
the deal's own contention domain, a few times per deal, and different
deals never contend with each other.

## What a proof shows, and what it does not

- **It shows** that the provider could produce the challenged leaves,
  matching the root, during the round.
- **It does not show** that the provider keeps a copy of its own. A
  provider could fetch the leaves from another copy when challenged (there
  is no proof of replication), so two deals with two providers are not
  proof of two copies.
- **Retrieval is not part of the protocol yet.** A provider proves custody;
  getting the data back is between client and provider, over any channel.
- **The custody record does not know whether its deal was closed.** Proofs
  after a close change nothing but the record.

## Memory kept by storage deals

A [memory log](memory.md)'s head has a `manifest` root, stored but not
checked by the ledger. The `storage` demo sets it to the root of the log's
archive (the Borsh encoding of its atoms) and posts a deal with that same
root: a reader then checks, from two objects on the ledger, that the memory
the robot committed is the data a provider is proving it keeps.

## Parameters

| Constant | Value |
|---|---|
| Leaves per deal | 1 to 2⁴⁰ |
| Leaf size | at most 4,096 bytes |
| Challenges per round | 1 to 16 |
| Rounds | 1 to 1,000,000 |
| Round length | 8 to 1,000,000 Pulses |
| Acceptance window | 1 to 1,000,000 Pulses |
| Locator | at most 256 bytes |
| Leaves per `ProveStorage` | at most 64 |
| Rounds before abandonment | 3 |

See [Storage from the command line](../guides/storage.md) for a walk
through, and the [specification](../specification.md), §5.50 to §5.54,
for the exact rules.
