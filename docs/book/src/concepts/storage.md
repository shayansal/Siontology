# Storage deals

A **storage deal** pays a provider, round by round, for keeping a body of
data and proving in every round that it still holds it. The ledger stores
only the data's Merkle root, and checks a few leaves of it per round,
which each proof publishes. Any machine with spare disk can be a provider:
a desktop running
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

The deal also carries a **locator**, where providers can fetch the data:
an `https://` URL, or a `file://` path for a provider on the client's own
machine. Nothing on the ledger checks it: a provider checks what it
fetched against the root before it accepts. A locator is written by the
client, so providers treat it as hostile (see [Providers on people's
devices](#providers-on-peoples-devices)).

## A deal, step by step

| Step | Transaction | Who | What happens |
|---|---|---|---|
| Post | `PostDeal` | The client | Escrows `price × rounds`; names a provider, or none for an **open** deal anyone may take |
| Accept | `AcceptDeal` | The provider | Posts the collateral; gets a **custody record** it owns. Rounds start at the next multiple of 8 Pulses |
| Prove | `ProveStorage` | The provider | Each round, answers the round's **challenge** from its custody record |
| Collect | `SettleDeal` | The provider | Paid `price` for every proven round not yet paid |
| Close | `CloseDeal` | Either | Pays what is due, refunds the escrow of unproven rounds to the client, settles the collateral |

A deal's money stays in the deal, a shared object in its own contention
domain: the escrow, and the collateral once accepted. Proofs touch only
the provider's own custody record.

## The challenge

Each round, the provider must show `challenges` leaves (1 to 16, chosen by
the client) with their Merkle paths. Which leaves is drawn from the
**beacon of the round's first Pulse**:

- the round's seed is `H(sion/storage/seed, beacon, deal)`, where
  `beacon` is the beacon of the round's first Pulse;
- the leaf of the `k`-th challenge of round `i` is the first 8 bytes of
  `H(sion/storage/challenge, seed, i, k)`, modulo the number of leaves.

Rounds start on multiples of 8 Pulses, and the chain records the beacon of
every such Pulse in one of 12,500 **beacon records** (the deferred phase
writes it), so a proof reads its round's beacon for up to 100,000 Pulses.
Nobody knows a round's challenge before its first Pulse, and nobody can
change it: the Pulse is fixed when the deal is accepted. A provider must
keep every leaf until then, and it gains nothing by choosing when its
proof executes.

That last point was settled by review. As first written, each proof
fixed the next round's challenge through the beacon of the Pulse it
executed in; a provider that could time its proofs, with a validator's
help, could pick among the beacons of a whole round and steer its
challenges towards the leaves it kept. In a simulation, one keeping half
its data passed 20 rounds of 20. The rules were changed before release.

What remains is the beacon's own bias: a validator that withholds its
reveal for a round's first Pulse chooses between two beacons, and `m`
colluding validators among `2^m`.

A provider that lost a fraction `f` of the leaves can answer a round only
if every challenged leaf is among those it kept, which happens with
probability `(1 − f)^challenges`, or about twice that if a validator
withholds its reveal on its behalf (our arithmetic):

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

A deal has `rounds` rounds of `round_length` Pulses, a multiple of 8 from
8 to 100,000. A round is proven during the round, at most once, and only
once its first Pulse has executed: a provider has `round_length − 1`
Pulses to land its proof, so on a network whose transactions take several
Pulses, rounds of 16 or more are safer. At the close:

- the provider is paid for every round it proved;
- the client gets back the escrow of every round not proven;
- the provider gets its collateral back, less its missed rounds' share
  of it, `⌊bond × missed / rounds⌋`, rounded down once. (As first written,
  each missed round cost `⌊bond / rounds⌋`, which was nothing when the
  collateral was smaller than the number of rounds.)

**Abandonment.** If a provider proves none of three rounds in a row, the
client may close the deal before its end, and the provider forfeits **all**
of its collateral. The provider is still paid for the rounds it proved.

**Open deals ask for collateral.** Anyone may accept an open deal; without
collateral, a stranger could accept it, never prove, and hold the client's
escrow until the deal is abandoned.

Forfeited collateral goes to the **treasury**, not to the client: no one
should profit from making a provider fail, for example by flooding its
network so its proofs are late. The client is made whole on the escrow
only; collateral makes a provider's promise credible, it does not insure
the data.

An example (our arithmetic): a deal of 6 rounds at 0.5 USD with 3 USD of
collateral. A provider proving every round is paid 3 USD and gets its
3 USD back. One that proves round 0 and then stops: the client closes during
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

## Privacy

Each proof publishes the leaves it answers with, in full, on the ledger.
Over many rounds most of a small file ends up there: with 4 challenges a
round, six rounds of an 18-leaf file publish about two thirds of it. Data
that must stay private is encrypted before its root is computed, and the
client keeps the key; providers then keep and prove ciphertext. A
`file://` locator also publishes the file's path.

## Providers on people's devices

A provider runs on a laptop or a phone and acts on deals written by
strangers. The provider in `sion storage serve` and the app:

- **proves first.** Each step proves the round under way for every deal,
  then collects and closes, then accepts deals whose data arrived, then
  starts new fetches in the background (two at a time, a minute at most),
  so a locator that never answers costs a fetch slot, never a round;
- **fetches carefully.** HTTP has a 5-second connect timeout and a
  30-second deadline, follows no redirects, reads no more than the deal's
  size, and does not reach loopback, private or other local addresses;
  TLS offers the hybrid post-quantum key exchange X25519MLKEM768 first.
  `file://` locators are refused unless the provider is told it may read
  local sources (a provider on the client's own machine), and then only
  regular files are read;
- **takes modest deals.** By default: at least 0.01 USD a round and twice
  what proving a round costs in fees, at most 10 USD of collateral a deal
  and 100 USD in all, deals of at most 100,000 Pulses, 1 GiB of copies;
- **keeps copies carefully.** A copy is written once the acceptance went
  through, whole, and deleted when its deal closes. Turned off, it takes
  nothing new but keeps proving the deals it holds.

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
| Round length | a multiple of 8, from 8 to 100,000 Pulses |
| Beacon records | every 8th Pulse, 12,500 slots (100,000 Pulses) |
| Acceptance window | 1 to 1,000,000 Pulses |
| Locator | at most 256 bytes |
| Leaves per `ProveStorage` | at most 64 |
| Rounds before abandonment | 3 |

See [Storage from the command line](../guides/storage.md) for a walk
through, and the [specification](../specification.md), §5.50 to §5.54,
for the exact rules.
