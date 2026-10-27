# Censorship resistance

A censor can be a gateway that drops your transactions or a group of
validators that refuse to certify them. The design makes censorship either
fail or show, and a committee that censors loses its place.

## In the architecture

- **One honest proposer is enough.** Every validator proposes every round,
  so any honest one can include an intent.
- **Forced inclusion.** An intent posted directly to the data layer with its
  fee must be included, or proven invalid, within *k* Pulses. A shard that
  fails is frozen in the next World Proof and its committee rotated, so
  censorship stops one shard visibly instead of succeeding silently, while
  other shards run on.
- **Accountable safety.** Equivocation and approvals of invalid transitions
  leave signed evidence and are slashed. Stalling, censorship and
  withholding cannot be attributed to anyone, so rotation, forced inclusion
  and sampling counter them instead.

## In this implementation

### Posting a forced intent

A user whose transactions are being ignored posts them straight to
consensus as a **forced intent** (`sion send --forced`, or
`POST /v1/forced`). The node hands it to every validator, bypassing the
gateway. Any honest validator proposes it, and consensus orders it.

### The obligation

Forced intents are not executed in the Pulse that commits them. In that
Pulse's deferred phase they are **posted**: each becomes an obligation of
its sender's shard, held in the state (specification §6.3). An intent is
dismissed instead, with the event `ForcedDismissed { intent, reason }`, if:

- the same intent is already pending, or 4,096 obligations are pending;
- it is a faucet transaction, or it expires at this Pulse or earlier;
- it does not name 1 to 256 distinct inputs, or its fee is below the
  minimum;
- its sender is unknown and it carries no valid genesis policy, or its
  signatures do not satisfy the sender's policy;
- an input does not exist at the named version, is spent, or is not the
  sender's, or the first input is not a USD coin covering the fee.

Otherwise the event `ForcedPosted { intent, shard, deadline }` records the
obligation. The **deadline** is the posting Pulse plus the forced-inclusion
window (8 Pulses on the devnet, `sion devnet --forced-window`), or the
intent's expiry if that comes first.

### Meeting it

Honest validators of the shard approve forced intents themselves and
exchange their approvals, so a lane certificate forms **without the
gateway**. Once certified, the intent is proposed and executes like any
transaction.

### The deadline

At the end of the deadline Pulse, the state asks whether the intent could
still execute: every input still exists at its version, unspent and owned
by the sender, and the sender has not rotated its keys since the intent was
posted.

- **No:** the intent executed, or its inputs were spent another way, or
  the sender rotated its keys. The obligation is met: `ForcedResolved`.
- **Yes:** the shard censored it. The event `ForcedOverdue { intent, shard }`
  is emitted, the shard's lane committee is **replaced**, and the
  obligation stands for another window (until the intent expires).

An obligation stays pending until its deadline even if the intent executes
earlier; it is checked only then.

### Replacing the committee

The new committee has `seats` members, drawn by stake from the active set
with a seed derived from the Pulse's beacon: first from validators outside
the committee that failed, and only if too few remain, from inside it. A
shard is rotated at most once per window. The new committee has a new lane
epoch, so approvals made by the old one no longer count. At the next epoch
boundary rotations are cleared and every shard is sampled afresh. The event
is `ShardRotated { shard, rotation }`.

Censorship cannot be pinned on particular members, so nobody is slashed
for it. But a committee captured by censors is replaced by as many fresh
members as the validator set allows.

### What light clients see

Every Pulse header commits to a summary of the obligations after it: how
many are pending within their deadline, how many are overdue, and a Merkle
root over the overdue ones (digest, shard, posting Pulse, next deadline,
deadlines missed). `sion verify` reports it, so censorship by more than a
third of a committee appears in every certified header from then on.

The summary is computed from the state and certified by the committee. The
World Proof covers the obligations themselves, as part of the state, but
does not check the header's summary against them. A client that wants the
obligations proven reads the obligations object with a state proof
(`GET /v1/objects/{id}`).

## On the devnet

From `sion demo censorship --local`:

```text
== Censorship resistance: forced inclusion makes censorship visible ==
           ADVERSARY SIMULATION: the gateway now drops every transaction from dissident, silently
           dissident signs: Pay 5 USD to alice. Fee 0.0012 USD; valid until Pulse 202.
           3 Pulses later: not executed; the gateway acknowledged it and did nothing
Pulse   9  posted as a forced intent: one honest proposer ordered it, the shard's validators certified it among themselves
           Done: paid 5 USD to alice; fee 0.0012 USD.
           ADVERSARY SIMULATION: 2 of the 4 validators now censor dissident as well (a quorum needs 3)
           forced intent posted around Pulse 9; by Pulse 19 it is OVERDUE (window 8 Pulses): the shard owes it and cannot certify it
Pulse  19  the state replaces shard 0's committee (rotation 1), drawing from validators outside it first; this devnet has 4 validators for 4 seats, so the same members return and the intent stays owed
           light client: forced inclusion: 1 intent(s) OVERDUE, censorship detected and certified by the committee (0 pending)
Pulse  23  censorship lifted: the intent executes late, and the record shows it: Done: paid 7 USD to alice; fee 0.0011 USD.
Result: a censoring gateway is routed around; a censoring third of a committee cannot hide, because every certified header counts what it owes.
```

The validators log the same events:

```text
validator: Pulse 19: forced intent 5ea754dc overdue in shard 0: censorship detected
validator: Pulse 19: shard 0's committee rotated (rotation 1)
```

The guide [Forced inclusion and the censorship demo](../guides/forced-inclusion.md)
does the same by hand.

## What is simplified or missing

- **The shard is not frozen.** The architecture freezes a censoring shard
  until it meets the obligation. Here the shard keeps running under its new
  committee.
- **Small validator sets cannot rotate.** With no more validators than
  seats (the default devnet has 4 and 4), the replacement committee is the
  same members, reshuffled, and the intent stays owed.
- **No separate data layer.** Forced intents go through consensus, not to a
  separate data-availability layer.

The censorship controls (`sion censor`, `POST /v1/devnet/censor`) are an
**adversary simulation** for the devnet only.
