# Linear Lanes

Most operations never touch consensus. An object with one owner, such as a
coin, can only be spent by that owner, so the only thing to prevent is the
owner signing two spends of the same version. That needs a quorum
certificate, not a consensus round.

## How a lane works

- **A lane per object.** Every owned object has an owner and a version
  number. Its history is a lane: a chain of versions only its owner can
  extend.
- **One approval per version.** To spend version *v*, the owner sends the
  signed transaction to its shard's committee. Each validator approves an
  `(object, version)` for at most one transaction digest. A second request
  for the same version is refused with a lock conflict.
- **The certificate is finality.** Approvals from validators holding more
  than two thirds of the committee's stake form a **lane certificate**. Two
  certificates for one version cannot both exist, because any two such
  quorums share an honest validator, and it approved only one of them.
- **No global order.** Execution runs per object. Nothing waits for
  unrelated traffic.

This is the consensus-free path of
[FastPay](https://arxiv.org/abs/2003.11506) and of owned objects in Sui
Lutris. Guerraoui et al. showed that such transfers need only reliable
broadcast.

## Batch-root voting

Post-quantum signatures are kilobytes, not bytes: one dual signature
(ML-DSA-44 and SLH-DSA-SHA2-128f) is about 19.5 KB. A validator therefore
never signs individual approvals. It collects its approvals, and every batch
interval (25 ms on the devnet) signs one Merkle root over the batch. An
approval is then that one signature plus a short inclusion path.

The signed batch header names the validator, the chain identity, the lane
epoch (the validator-set epoch and the number of times the shard's
committee was rotated in it), the shard, a sequence number, the Merkle root
and the batch size. An approval from one chain, committee or shard never
counts for another.

Certified transactions travel to consensus in groups that carry each signed
batch header once and each approval as a reference (batch, position, path).
A group of a hundred transactions costs a handful of dual signatures.

## Shards and committees

An account belongs to one shard, derived from its identifier and the number
of shards. Each shard has a lane committee of `seats` validators, drawn by
stake from the active set with the epoch's seed, one seat each. The seed is
the genesis digest in epoch 0 and the beacon of the previous epoch's last
Pulse afterwards. A shard that misses a forced-inclusion deadline gets a
new committee ([Censorship resistance](censorship-resistance.md)). The
shard's committee certifies every transaction its accounts send.

With the devnet defaults (4 validators, 2 shards, 4 seats), every committee
is all four validators and a certificate needs 3 approvals.

## What a validator checks before approving

Before it locks anything, a validator checks everything execution would
check before charging a fee, so nobody gets data published for free:

- the transaction has not expired;
- it names 1 to 256 distinct inputs;
- its fee meets the minimum for its size, and its first input is a USD coin
  that covers the fee (a faucet transaction has its own rules);
- every input exists at the named version, belongs to the sender and is not
  spent;
- the signatures satisfy the sender's authorization policy, signed for this
  chain's configuration.

A validator that is behind (an input it has not seen created yet) parks the
request for up to 4 Pulses instead of refusing it, so lagging validators do
not split an object's locks.

## Locks, restarts and committee changes

A lock is released when its transaction executes (the input version is gone)
or expires. Approvals are written to a durable lane log before they leave a
validator, and locks are restored after a restart, so a certificate's
finality survives a restart: no conflicting spend can be certified, and the
original transaction can be certified again after the restart. See
[Storage](../architecture/storage.md).

Locks belong to validators, not to committees. When a shard's committee
changes, at an epoch boundary or a rotation, members that stay keep their
locks and new members start without them. Approvals from the old committee
no longer count, so the gateway collects approvals again from the new
committee for every certified transaction that has not executed.

## Everything is lane-certified

In this implementation every transaction, contended ones included, first
locks its owned inputs (at least its gas coin) with a lane certificate. A
consensus-ordered spend can therefore never overtake a certified one. An
owned-path transaction's certificate goes to consensus only so that a Pulse
executes it.

## In this implementation

- **Built**: lanes, batch-root voting with dual post-quantum signatures,
  lock conflicts, durable locks, certificates checked again by every
  validator before execution and by the World Proof.
- **Built as well**: slashing of a validator that approves two live
  transactions spending one object version; see
  [Slashing for equivocation](validators/equivocation-slashing.md).
- **Not built**: private resources as notes with nullifiers (amounts and
  balances are public; atom `impl:public-amounts`).
- **Simplified**: the gateway that collects approvals is one component in
  the devnet process.
