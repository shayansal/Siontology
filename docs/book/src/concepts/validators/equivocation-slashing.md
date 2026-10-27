# Slashing for equivocation

Proofs stop invalid state transitions. They do not stop a validator from
signing two conflicting messages, which is how a Byzantine third of a
committee certifies two conflicting histories. Equivocation leaves signed
evidence, so it can be punished: anyone who holds the two messages reports
them, the offender forfeits its whole stake, and the reporter is paid a
share.

## The evidence

`ReportEquivocation { evidence }` (specification §5.30) carries one of two
kinds of evidence. Signatures are checked with the keys the validator set
records for the signer.

| Evidence | What it shows | Conditions |
|---|---|---|
| `Headers { first, second }` | Two consensus headers by one author for the same round | Same author, chain, epoch, shard and round; different digests; both signatures verify |
| `Approvals { first, second }` | Two lane approvals by one validator for two transactions that spend the same object version | Same validator, chain, epoch and shard; different transactions sharing an input `(id, version)`; both transactions unexpired at the reporting Pulse; both approvals verify |

An honest validator approves a second spend of an object version only once
the first transaction can no longer execute: it succeeded or failed, which
moves every input it names to a new version, or it expired. So two
approvals of live transactions are proof; once either transaction has
expired, the evidence no longer counts.

A **rejected** transaction is the case to watch. It wrote nothing, so its
inputs keep their versions, and it may still execute later. The node
therefore keeps its locks until it expires, and refuses a second spend of
its inputs until then. Releasing them at the rejection, as the reference
node did before an independent review found it, let an owner frame honest
validators: get a transaction rejected by spending one of its other inputs
elsewhere, then spend the rest again and report every validator that
approved both ([Security reviews](../../security/reviews.md)).

## The penalty

If the evidence holds and the offender is neither already slashed nor
withdrawn:

1. Its whole stake and its unclaimed rewards are forfeited, and its status
   becomes **Slashed**.
2. The reporter receives `reporter_permille` thousandths of each (5 % on
   the devnet).
3. The rest goes to the treasury. It is not a fee, so it is never paid
   out as rewards ([Rewards](rewards.md)).
4. The event `ValidatorSlashed { validator, amount, reporter, reward }` is
   emitted.

The reporter's share is small on purpose: a validator that reports itself
to recover its stake still loses 95 % of it.

The offender **keeps its seat**, and its weight in quorums, until the epoch
ends; at the boundary it is not Active, so it leaves the active set
(`impl:slashed-seat`), even if no other validator is eligible. A chain
whose every eligible validator was slashed then halts
([Epochs](epochs.md)). An Exited validator still waiting out its unbonding
period can be slashed; a Withdrawn one has nothing left to slash.

## Evidence is not bound to a chain or a time

The two messages must agree with each other on the chain, but nothing
requires that chain to be this one, and header evidence never expires. An
offence is an offence wherever it was signed. So an operator must not use
a validator's keys on more than one network: an equivocation on a test
network, where it costs nothing, would slash the stake here
(`impl:evidence-scope`).

## Who reports

Anyone with the two signed messages. A validator that receives two
headers from one author for one round logs the offender (`validator <i>
detected Byzantine behavior by …`), but it does not submit the report
itself. In the staking demo, an account called `watchdog` does.

## On the devnet

`sion demo staking --local` makes the newcomer equivocate. This is an
adversary simulation: the demo signs two headers with the validator's
devnet keys, which anyone can derive.

```text
           ADVERSARY SIMULATION: validator a6f32863 signs two different consensus headers for the same round
Pulse  31  watchdog reports both signed headers: Done: validator a6f32863 slashed: 50,000 USD forfeited, 2,500 USD of it to watchdog for the report; fee 0.005 USD.
           epoch 2 begins with 4 validators: the slashed validator lost its seat
           light client: still following, 2 boundary proof(s) from the pinned genesis ✓
Result: who certifies is part of the state transition, proven like any balance; stake buys seats, misbehaviour is evidence anyone can submit, and a client needs nothing but the genesis it pinned.
```

## When a report is refused

| Situation | Reason |
|---|---|
| Two headers with different authors, chains, epochs, shards or rounds | `the headers are not for the same author and round` |
| The same header twice | `the headers are the same` |
| A header signature fails | `a header signature does not verify` |
| Two approvals by different validators, chains, epochs or shards | `the approvals are not by the same validator` |
| The same transaction twice | `the approvals are of the same transaction` |
| One of the transactions has expired | `an approved transaction has expired` |
| No shared input version | `the transactions spend no common object version` |
| The offender was already slashed or has withdrawn | `the validator was already slashed or has withdrawn` |

## Not covered

Equivocation is the only validator offence that can be slashed. The
architecture also slashes validators that certify an invalid state
transition; that is not built: a header whose World Proof fails is caught
by verification, but no transaction slashes its signers. Stalling,
censorship and withholding cannot be attributed to anyone, so the
architecture counters them with rotation, forced inclusion and sampling
instead ([Censorship resistance](../censorship-resistance.md)).
