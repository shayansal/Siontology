# A transaction, from wallet to receipt

This chapter follows one transfer, `sion send dana alice 12.5`, through the
code. Each step names the module that does it.

```text
wallet ──► API ──► gateway ──► lane committee ──► gateway ──► consensus ──► Pulse ──► header certificate
 sign      submit   route       lock + approve     certificate  order        execute    + World Proof
                                                                                         ──► receipt
```

## 1. The wallet builds and signs it

`sion_cli::session` and `sion_exec::wallet`:

1. Look up the sender's account object (`GET /v1/accounts/{id}`). If the
   account does not exist yet, the transaction carries the wallet's genesis
   policy.
2. Select USD coins, largest first, to cover the fee plus the amount. The
   first input is the gas coin.
3. Compute the minimum fee for the body's size and set the expiry to the
   current Pulse plus 20.
4. Sign the digest with ML-DSA-44 and SLH-DSA-SHA2-128f under a signing
   context bound to the chain's configuration.

The CLI prints the sentence it signed:

```text
dana signs: Pay 12.5 USD to alice. Fee 0.0012 USD; valid until Pulse 22.
```

**A batch** (specification v1.13) is built the same way, body by body,
each with the fee floor of a member of a batch of its size
(`Wallet::draft`, or `Keyless::draft` for a keyless subagent); then one
wallet signs the Merkle root over their digests once and gives every member
the authorization, its position and its path (`sion_exec::tx::sign_batch`).

## 2. The API hands it to the gateway

`POST /v1/transactions` with `{"tx": "<hex of the Borsh SignedTransaction>"}`
(`sion_node::api`). The gateway (`sion_node::gateway`) refuses bodies over
256 KiB, derives the sender's shard from its account identifier, and
answers at once with the digest, the route (`lane` for owned paths,
`consensus` for contended ones) and the shard. Every transaction takes the
lane route first; a contended one then also goes through consensus. A whole
batch goes in one `POST /v1/batches`, with its signatures once: the node
checks the members against the root and builds their paths.

## 3. The shard's lane committee locks the inputs

The gateway sends a lane request (the digest, the inputs, and the signed
transaction as the owner's proof) to every member of the shard's lane
committee; a batch's members in one shard go in one `LaneBatch` message,
the batch's signatures once. Each member (`sion_consensus::lanes::LaneValidator`, driven by
`sion_node::validator`):

1. checks what execution would check before charging a fee (expiry, 1 to
   256 distinct inputs, fee, inputs owned at those versions, signatures:
   a batch's the first time, then only each member's path, since the
   validator remembers the batch, `sion_node::owners::Authorized`);
2. locks each input version to this digest, or refuses if one is locked to
   another transaction;
3. writes the approval to its lane log before it leaves the validator;
4. adds the approval to its current batch. Every 25 ms it signs the batch's
   Merkle root once, with a dual signature, and sends each approval (the
   batch header plus a Merkle path) to the gateway.

A member that has not seen an input yet parks the request for up to 4
Pulses instead of refusing it. Once refusals make a quorum impossible (the
refusing members hold more than the committee's total minus the quorum),
the gateway marks the transaction rejected:

```text
{"reason":"object ObjectRef { id: ObjectId(27657e3b), version: Version(0) } is locked to transaction TxDigest(93d6ac50)","state":"rejected"}
```

## 4. The gateway forms a certificate and hands it to consensus

Once approvals reach a quorum, the gateway has a lane certificate and the
transaction is final (certificate finality). Every 20 ms the gateway hands
the certificates collected since the last flush to consensus as one group,
at most 512, with each batch header carried once, and each transaction
batch's authorization once. It sends the group to
f + 1 members of the consensus committee, so at least one honest proposer
includes it. A certified transaction not executed after 4 seconds, or
dropped by a new consensus session, is sent again; duplicates execute once.

While this happens, `GET /v1/transactions/{digest}` answers
`{"state":"certifying","shard":…,"approvals":…}` and then
`{"state":"sequencing"}`.

## 5. Consensus orders it

Validators put payload items in their DAG headers
(`sion_consensus::core`). When an anchor commits, its causal history is
delivered in a deterministic order, and the Pulse builder
(`sion_node::pulses`) cuts the commit sequence into Pulses on round
boundaries.

## 6. Every validator executes the Pulse

`sion_node::validator::execute`:

1. Re-check every certificate in the Pulse against the shard's lane
   committee; drop what does not verify.
2. Split the certified transactions into owned and contended; take the
   forced intents committed in the Pulse.
3. Execute the Pulse (`sion_exec::pulse::execute_pulse`): owned
   transactions in digest order, contended ones in commit order, then the
   deferred phase.
4. Build the Pulse's published data (transaction bodies in execution order,
   plus the posted forced intents) and its 2D Reed–Solomon encoding.
5. Append the Pulse to the write-ahead log (a snapshot every 50 Pulses).
6. Build the header, sign it, and send the signature to the others. A
   validator never signs a header without a data-availability commitment.
7. Derive the committees from the new state; at an epoch boundary, start a
   new consensus session.

When signatures from more than two thirds of the active set's stake arrive,
the header is certified.

## 7. The prover folds it

Validator 0 executes with tracing and hands the chunks, the data, the
certificates and the validator set the Pulse began with to the prover
thread (`sion_node::prover`), which builds a World Proof with stacks A and
B and verifies it. See [The prover](prover.md).

## 8. The client checks the receipt

`GET /v1/transactions/{digest}` now returns `{"state":"executed","record":…}`
with the effects, the outcome, the Pulse, the position, and a Merkle path
from the effects to the receipts root. The CLI prints the outcome as a
sentence. `sion receipt` also fetches the Pulse's certified header and
checks everything as a light client:

```text
$ sion receipt 0b4b87c8ed9db1d35d2548e9dfa2d7c529c680a2cd264d9cfcd683f85b897d39 --chain 6345655cfd2a714ed1dab69c0660f2f48f429ce2981e25629d0467d7b7ee97b7
Pulse 5: Done: paid 12.5 USD to alice; fee 0.0012 USD.
  certified by 300000 of the committee's stake (quorum 266667), dual post-quantum signatures ✓
  receipt proven against the certified header's receipts root ✓
  chain 6345655c (pinned)
```

On the devnet used for this book, the whole path took about 8 seconds
(`sion send` returned after 8,266 ms). See
[Benchmarks](../guides/benchmarks.md) for measured latencies.
