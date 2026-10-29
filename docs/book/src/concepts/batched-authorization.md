# Batched authorization

Every transaction on Siontology is authorized by dual post-quantum
signatures: ML-DSA-44 and SLH-DSA-SHA2-128f, so that forging one means
breaking both a lattice scheme and a hash-based one. That pair is 19.5 KB
and took 1.3 ms of one core to check on the benchmark machine (measured,
see [Benchmarks](../guides/benchmarks.md)). For people that is a fair price;
for agents acting millions of times a second it is most of the cost of
the ledger (see [Billions of agents](../research/agent-scale.md)).
Specification v1.13 lets one authorization serve up to 256 transactions.

## A batch

A **batch** is `TxBatch { signer, size, root }`: the account that signs,
how many transactions it holds, and the RFC 9162 Merkle root over their
digests. The signer's policy signs the batch's message,
`H("sion/tx/batch", enc(batch))`, in a signing context of its own
(`sion/tx/v1/batch/<configuration digest>`), so a batch's signatures never
pass for a transaction's, or the other way round.

Each member of the batch is an ordinary transaction that carries, instead
of signatures of its own, the batch's authorization, its position and its
inclusion path (about 8 hashes for a batch of 256). The ledger checks the
path from the member's digest to the root, then the signatures over the
root. Everything else about a member is its own: its inputs, fee, expiry
and outcome. Members may execute in different Pulses, or not at all, and
nothing orders them.

| | Signed by itself | A member of a batch of 256 |
|---|---|---|
| What the signatures sign | The transaction's digest | The batch's root |
| Signature bytes it carries on the wire | 19.5 KB | its share of one set, plus its path |
| What a validator checks for it | A dual signature | Its path; the batch's signatures once |
| Fee floor (devnet) | 0.001 USD + 0.0001 USD per KiB | 0.0002 USD + 0.0001 USD per KiB, plus a 256th of 0.0008 USD rounded up to the µUSD |

In the benchmark ([Benchmarks](../guides/benchmarks.md#authorization)), a
member of a batch of 256 cost 30 µs to check, against 4.5 ms for a
transaction signed by itself, and carried 418 bytes of signatures, place
and path, against 19,525 bytes (measured on one machine).

## The fee

The base fee pays mostly for checking signatures. Specification v1.13
says how much: 80 % (`SIGNATURE_SHARE_PERMILLE`), a protocol constant. A
batch member's floor is the usual one less that share, plus its part of
it, rounded up:

```text
share        = ⌊base_fee × 800 / 1000⌋
required_fee = min_fee(size) − share + ⌈share / batch size⌉
```

A batch of one pays what a transaction signed by itself pays. The floor
trusts the size the batch declares, so a signer that declares 256 members
and sends one pays about a fifth of the base fee for a check that
validators run in full. The reference node checks each batch's signatures
once, after the cheap checks of its members, and takes batches whole over
`POST /v1/batches`.

## Keyless subagents

A batch's members can come from different accounts: the signer's own, and
those of its **keyless subagents**. A subagent spawned without an account
gets `H("sion/agent/keyless", id)`, hashed under a domain tag of its own so
that no key's policy hashes to it, and only its nearest keyed ancestor can
sign for it (see [Agent identities and
subagents](agent-identities.md#keyless-subagents)). An orchestrator with a
thousand keyless workers signs four batches for a payment from each of
them, and holds one key pair, not a thousand.

## What the nodes do

- **Validators check once.** A lane request for a member checks the
  member's own path every time and the batch's signatures the first time;
  after that the batch is remembered by its message, with the policy and
  signatures compared byte for byte (`sion_node::owners::Authorized`).
  The executor and both proof stacks remember checked batches the same way,
  so a prover replaying a chunk checks each batch's signatures once.
- **Consensus carries them once.** A group of certified transactions
  carries each batch's authorization once, and each member an index into
  the group's list (`sion_node::payload`).
- **Clients send them once.** `POST /v1/batches` takes the batch, its
  authorization and the members' bodies; the node checks the members
  against the root and builds each path itself.
- **Data availability never carried signatures.** The published data is
  the transaction bodies.

## Forced inclusion

A batch member can be posted as a forced intent like any transaction. Its
obligation records its **signer**, the account whose policy authorized it
(the sender, or the ancestor of a keyless sender), so a rotation of that
account's policy, which may void the batch's signatures, resolves the
obligation instead of blaming the shard.

## Not built

- **Penalties for conflicting batches.** A signer can sign two batches
  whose members spend the same coin. As for any two conflicting
  transactions, the lane locks let at most one of them be certified, and
  nothing penalizes the signer, so a payee waits for its member's lane
  certificate, as it would for any transaction.
- **Lineage ids, cohorts, token buckets and overlays** from the research
  note's plan come in later versions.

## Try it

- `sion demo batch --local`: an orchestrator launches 64 keyless
  subagents and signs once for a payment from each of them.
- `sion bench --only authorization`: what a check and its bytes cost signed
  by itself and in batches of 16, 64 and 256, on your machine.
