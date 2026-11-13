# A market for proofs

The architecture has provers do the heavy work of proving each Pulse and
be paid for it. In this implementation a proving node proves its own
Pulses, and the protocol asks nobody for a proof and pays nobody for one.
`sion proving` puts a market beside that, made from pieces the ledger
already has: a request for the proof of one Pulse, a payment in escrow,
verifiers who check what is delivered and answer for it with a bond, and
payment once they have.

Two things to know before the rest:

- **The proofs are replay proofs.** A World Proof here is checked by
  executing its Pulse again, under two independent implementations. It
  stands in for a succinct proof and is labelled as a stand-in in every
  output. No succinct proof system is built
  ([the plan](../research/succinct-proofs.md)). A request names its
  proof system, and the only one is `replay+spec/v1`.
- **Nothing in consensus changed.** A request is a
  [Task Charter](../concepts/task-charters.md). The ledger checks who
  attested, never the proof.

Outputs on this page are from real runs of the commands on a local
devnet, with long identifiers shortened with `…`.

## Asking for a proof

A requester escrows a payment for the World Proof of one Pulse and names
the verifiers who must each attest it. Here rita asks at Pulse 18 for the
proof of Pulse 34:

```text
$ sion proving request 34 3 --verifier vic --verifier vera \
    --adjudicator e1d8032b… --wallet rita
asked for the World Proof of Pulse 34 (replay+spec/v1): 3 USD escrowed in task 8dabe259…
  paid to whoever delivers it once 2 verifier(s) attest it
$ sion proving show 8dabe259…
request 8dabe259 by 0f4894f6: the World Proof of Pulse 34 (replay+spec/v1), 3 USD escrowed
  to be delivered by anyone and attested by: 5c969032, e63c95cc
  open until Pulse 518
```

The task's spec commits to the chain, the Pulse and the proof system.
Its acceptance says that every verifier named must attest a result
before it is paid. The adjudicator is the one who rules if an
attestation is challenged; its atom comes from `sion bond`, as in
[Task Charters from the command line](tasks.md). A verifier needs a
`verifier` bond and a prover a `task-worker` bond, as for any task.

## Delivering one

A prover takes the proof from a node that proves, and checks it before
spending a fee on it:

```text
$ sion proving fetch 8dabe259… --out proof.bin
world proof: 2 leaves verified under stacks ["A", "B"], 9 KB; replay proofs simulate succinct ones ✓
the World Proof of Pulse 34, 9741 bytes: proof.bin
```

Before the Pulse is proven there is nothing to take:

```text
$ sion proving fetch 8dabe259… --out proof.bin
error: this node has no World Proof of Pulse 34. A node keeps the proofs of its last 32 Pulses only, and only if it proves. It said: no World Proof for that Pulse (yet)
```

The prover gives the file to the verifiers. How it reaches them is not
the ledger's business; the proof holds nothing that is not public.

## Checking and attesting

A verifier checks the file against the request and attests it only if
every check passes:

```text
$ sion proving attest 8dabe259… proof.bin --wallet vera --chain 94425d72…
chain: 94425d72 matches the pinned identity; 4 validators, keys derive their identifiers ✓
Pulse 34 header d5d3294a (state root b00da0d1, knowledge root 94e0efcb, 1 claims)
certificate: 3 of 4 validators signed, 300000 of 400000 stake (quorum 266667), each signature ML-DSA-44 + SLH-DSA-SHA2-128f ✓
forced inclusion: no overdue intents (0 pending within their deadline) ✓
receipts: 0 of 0 transactions proven against the receipts root, in the certified order ✓
state: proof skipped (node already at Pulse 36)
availability: 16 of 16 random shares of the 8×8 square verified; a withholding would go unnoticed with probability < 7.7e-5 ✓
world proof: 2 leaves verified under stacks ["A", "B"], 9 KB; replay proofs simulate succinct ones ✓
lane certificates: all 0 executed transactions locked their inputs with a quorum of their shard's committee ✓
attested: claim 566fa3a9…
```

The verifier does not take the Pulse on the node's word. It checks the
Pulse's header as [a light client checks one](light-client.md): the
committee's certificate, the receipts, the data's availability. Then it
checks that the proof is of that header (its state before and after, its
receipts, its data, its beacon), that every leaf verifies under both
proof stacks, and that every executed transaction's lane certificate
holds. The attestation is a claim on the verifier's bond.

`--chain` is the chain identity the verifier trusts. The request names
its chain, and the two must be the same:

```text
$ sion proving attest 8dabe259… proof.bin --wallet vera --chain 00000000…
error: verification failed: the request is for chain 94425d72, not the chain you pinned
```

Without `--chain` the check runs against the chain the request names,
and the first line says whose word that is:

```text
chain: 94425d72 is the chain the request names (NOT PINNED: the request came from the node itself; pass --chain 94425d72… to pin it)
```

Only the request's verifiers attest, and bytes that are no proof get
nowhere:

```text
$ sion proving attest 8dabe259… proof.bin --wallet rita
error: rita is not a verifier of this request
$ sion proving show 8dabe259… --proof cut.bin
…
error: verification failed: these bytes are not a World Proof: decode error: Unexpected length of input
```

## Being paid

The prover settles the request with the attestations. With one of the
two the ledger pays nothing:

```text
$ sion proving deliver 8dabe259… proof.bin --attestations 566fa3a9… --wallet pat
error: the ledger did not settle the request: invalid transaction: every verifier the task says must attest has to attest the result
```

With both it pays what was escrowed:

```text
$ sion proving deliver 8dabe259… proof.bin \
    --attestations 566fa3a9…,8a5fdaa2… --wallet pat
delivered and settled: 3 USD paid to pat
$ sion proving show 8dabe259…
request 8dabe259 by 0f4894f6: the World Proof of Pulse 34 (replay+spec/v1), 3 USD escrowed
  to be delivered by anyone and attested by: 5c969032, e63c95cc
  delivered by 84f1cff8 and paid at Pulse 54 (result 0af60778)
```

The prover's result is a claim that commits to the proof's bytes and
rests on its own bond and on the two attestations. The proof itself is
not put on the ledger.

## Naming the prover

An attestation says that these bytes are the proof the request wants. It
does not say who made them. In a request open to anyone, whoever settles
first with the attested bytes is paid. A requester that has agreed with
one prover names it, and the task is then that account's alone to
settle:

```text
$ sion proving request 72 3 --verifier vic --verifier vera --prover pat \
    --adjudicator e1d8032b… --wallet rita
asked for the World Proof of Pulse 72 (replay+spec/v1): 3 USD escrowed in task 76a1aa97…
  paid to 84f1cff8 for delivering it once 2 verifier(s) attest it
```

## A Pulse too far back

A node keeps the proofs of its last 32 Pulses, and a verifier's check
needs a node that still serves the Pulse's data (64 Pulses on a devnet).
So a request is for a Pulse to come or a recent one. One for an old
Pulse says so, and the requester takes the escrow back:

```text
$ sion proving request 3 1 --verifier vic --wallet rita
asked for the World Proof of Pulse 3 (replay+spec/v1): 1 USD escrowed in task 90632dc5…
  paid to whoever delivers it once 1 verifier(s) attest it
  note: Pulse 3 is 69 Pulses back, and a node keeps the proofs of its last 32 only: if no prover kept this one, cancel the request to take the escrow back (sion task cancel)
$ sion task cancel rita 90632dc5…
Pulse 75: Done: task 90632dc5 cancelled: 1 USD refunded to rita; fee 0.0011 USD.
$ sion proving show 90632dc5…
request 90632dc5 by 0f4894f6: the World Proof of Pulse 3 (replay+spec/v1), 1 USD escrowed
  to be delivered by anyone and attested by: e63c95cc
  withdrawn by 0f4894f6 at Pulse 75: its escrow went back
```

## What the market gives, and what it does not

(`impl:proving-market`)

- **A proof is paid for on its verifiers' word, not the ledger's.** The
  ledger checks that every verifier the request names attested the
  delivered bytes. It never checks the proof. A check of a succinct
  proof by the ledger itself is what the architecture describes, and it
  is not built.
- **The requester chooses the verifiers.** All of them together with a
  prover can be paid for bytes that prove nothing. What stands against
  that is each verifier's bond: an attestation is a claim the request's
  adjudicator can rule against, and a ruling slashes the bond
  ([Bonds and challenges](bonds-and-challenges.md)). A request that
  names no adjudicator leaves nobody to rule.
- **The proofs are replay proofs.** Making one costs a prover no more
  than a proving node already spends, and checking one costs as much as
  executing the Pulse. The market's parts (a request, bonded
  verification, payment on delivery) do not depend on that; what it
  trades today does.
- **Provers do not compete here.** There are no bids, no collateral
  forfeited for a missed deadline, no auction of a job again, and no
  classes of latency. A request has a price its requester set and a
  deadline after which the escrow can be taken back.
- **The protocol pays no prover.** A request is paid from its
  requester's own money. Fees do not go to provers.
- **A requester can withdraw an open request** at any time before it is
  settled, and a prover that has already had its proof attested is then
  not paid.
- **A prover's own check takes the Pulse's header on its node's word.**
  It is there to save the prover a fee, not to convince anyone. Part of
  a proof (the witness of the validator set) is bound only by the
  verifier's check of the lane certificates.
- **A verifier's check is a light client's**, with what that rests on
  ([Light-client verification](light-client.md)). A verifier that pins
  no chain has taken the request and its chain from the same node.
- **The market makes nothing final.** A Pulse's proof finality is its
  node's, as before. A paid request shows that named verifiers checked
  one proof of one Pulse and answered for it.

This is tested on a devnet (`proving.rs`): a request, the proof of
another Pulse and a changed proof refused, a chain other than the one
pinned refused, an account that is not a verifier refused, one
attestation of two not settling, the prover paid once, and a request
that names its prover. A challenge to a false attestation is the
ledger's path for any bonded claim, and that test does not run it again.
