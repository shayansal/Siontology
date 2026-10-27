# The gateway

The gateway (`sion_node::gateway`) is the devnet's front door for
transactions. It routes each one to its shard's lane committee, collects
approvals into a certificate, and hands certificates to consensus. In the
architecture any party can do this; in the devnet it is one thread in the
node process, which is a named simplification.

The gateway holds no authority. It cannot forge an approval, and a
certificate it assembles is checked again by every validator before
execution and by the World Proof. What it can do is ignore a transaction,
which is why forced intents exist ([Censorship resistance](../concepts/censorship-resistance.md)).

## Submitting

For each submitted transaction the gateway:

1. refuses it if the encoded transaction exceeds 256 KiB;
2. derives the sender's shard from its account identifier;
3. answers with the digest, the route (`lane`, or `consensus` for kinds in
   a contention domain) and the shard;
4. sends a lane request to every member of the shard's current lane
   committee and starts a certificate builder.

A transaction it refused before may be submitted again: validators may
have caught up, or its inputs been released. Submitting the same
transaction twice while it is in flight changes nothing.

## Collecting approvals

Approvals arrive from validators' batches. The gateway checks each one
against the lane committee (including its lane epoch) and adds it to the
builder. When the builder reaches a quorum, the transaction is certified
and its status becomes `sequencing`. Refusals are collected too; once they
make a quorum impossible, the status becomes `rejected` with a validator's
reason.

## Handing certificates to consensus

Every 20 ms the gateway sends the certificates collected since the last
flush, at most 512, as one **group**: each signed batch header once, each
approval as a reference (batch, position, Merkle path). It sends the group
to f + 1 members of the consensus committee, rotating through them, so at
least one honest proposer includes it. Validators outside the active set
only observe, so they are skipped.

It keeps every certified transaction until a Pulse executes it:

- after 4 seconds without execution, it sends the certificate again
  (duplicates execute once);
- when a new consensus session starts, it sends every pending certificate
  again, since the new session may have dropped them;
- when a shard's lane committee changes, approvals from the old committee
  no longer count, so it submits those transactions again to the new
  committee.

## The faucet

`POST /v1/faucet` queues grants (at most 10,000 waiting). Every 100 ms, the
gateway turns up to 256 queued grants into one `Faucet` transaction signed
by the faucet wallet, with no fee and an expiry 30 Pulses ahead, and waits
for it to execute before sending the next. Grants of a transaction that
did not succeed go back in the queue.

## Statuses

The gateway remembers the status of the last 100,000 transactions it
handled. `GET /v1/transactions/{digest}` reports `certifying` (with the
shard and the approvals so far), `sequencing`, `rejected` (with a reason)
or, from the validators' records, `executed`. A transaction that executed
long enough ago that its Pulse left the node's 256-Pulse history reads as
`sequencing` again: the gateway forgot it had executed, and the record is
gone.

## Adversary simulation

`POST /v1/devnet/censor` with `gateway: true` makes the gateway acknowledge
the listed accounts' transactions and do nothing with them. It is for
demonstrating forced inclusion on a devnet.
