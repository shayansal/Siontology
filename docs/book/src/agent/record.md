# Its record, and a warranty

Someone asked to insure an agent, or to rely on it for something that
matters, needs to know what it may do, what it has done and how often
that went badly, from a source its operator cannot edit. The ledger holds
much of that already. This page is how to hand it over, and how to stand
behind an agent on a bond.

Outputs on this page are from the end-to-end test's runs
(`tests/agent_record.rs`) on a local devnet, with a scripted model and
long identifiers shortened with `…`.

## The record

`sion agent record write <agent>` gathers an agent's record into one
document, in two parts that are never mixed:

```text
$ sion agent record write 2de88785… --out clerk-record.json --wallet alice
agent 2de88785, signed for by 9030d69d, owned by 42424444
what the ledger shows, Pulse 0 to 31:
  1 payment(s) to 1 account(s), 5 USD in all, the largest 5 USD
  tasks: 0 delivered (0 USD), 0 proven false, 0 given back
  suspended 0 time(s), narrowed 0, 0 subagent(s) launched
  0 change(s) kept on sealed tasks, 0 seal(s) published
  bonds of its account and owner: 0 posted, 0 challenge(s), 0 upheld, 0 USD slashed
what its operator says, from 2 session(s) on its machine:
  runs ended: 2 done; 0 finished with their check passed
  2 call(s), 1 failed; 1 payment(s) the ledger refused
  a person was asked 0 time(s): 0 approved, 0 declined, 0 signed
  1 of 2 stated expectation(s) held; 0 result(s) held back as reported attacks
digest …
written to clerk-record.json
committed to in claim e0eccf07…
```

- **What the ledger shows** is added up from the ledger's events over a
  span of Pulses (`--from`, `--to`): the payments the agent made under its
  charter, the times it was suspended or narrowed, the tasks it was paid
  for, gave back or had proven false, the changes it kept on sealed
  tasks, and every challenge and slash against the bonds of its account
  and its owner. The record also holds the charter's terms as they stood:
  whom it may pay, how much at a time, how fast, until when.
- **What its operator says** is what only the operator's machine knows,
  counted from the agent's sessions there: how runs ended, how often a
  person was asked and what they decided, how the agent's stated
  expectations held ([What it expected](checks.md#what-it-expected)),
  what was held back as a reported attack
  ([Attacks reported between operators](security.md#attacks-reported-between-operators)),
  and payments the ledger refused. `--ledger-only` leaves this part out.

With `--wallet`, a claim commits to the record's digest, so that the
record cannot be changed afterwards without it showing.

Whoever is given the file checks it against the ledger, from any node:

```text
$ sion agent record check clerk-record.json
holds: what the record says the ledger shows from Pulse 0 to 31 is what the ledger shows
holds: the charter is as the record has it
holds: its owner committed to this record in claim e0eccf07 at Pulse 35
its operator's word, which the ledger cannot check: what 2 sessions on its machine show (their digest 3c09f9b85e43e0b7…)
```

Every figure of the first part is added up again from the events. A
record that was altered fails, each figure that differs named
(`differs: payments: the record says 9, the ledger shows 1`), and the
command ends with an error. A charter that was suspended, narrowed or
revoked since is said as `changed since`, which is not a failure: the
record says what was so when it was written.

## A warranty

`sion agent record warrant <wallet> <agent> --covers "<what>"` is the
operator standing behind the agent: a claim on its bond `agent-warranty`,
under an adjudicator, saying in the operator's words what it warrants. It
names the record the operator last committed to.

```text
$ sion agent record warrant alice 2de88785… --covers "It pays only for invoices its principal was sent, once each." \
    --adjudicator 5d0c… --bond 200
warranted agent 2de88785: claim a60fa402…, on your bond agent-warranty
  a counterparty relies on it with: sion accept <wallet> a60fa402… <USD>
```

A counterparty relies on the warranty for a value with `sion accept`,
which reserves that value times its safety factor of the bond for it.
Nobody else can rely on the same part of the bond. If the agent does what
the warranty says it will not, the counterparty challenges the bond
(`sion challenge`) and the adjudicator rules (`sion rule`). In the test
the counterparty relied on it for 20 USD, challenged, and the adjudicator
upheld the challenge: the bond was slashed and the counterparty was
compensated from it in the same transaction, with nothing for the
operator to approve. The agent's record shows it from then on:

```text
  bonds of its account and owner: 1 posted, 1 challenge(s), 1 upheld, 200 USD slashed
```

Reliance, challenges and compensation are the ledger's own
([Reservations](../concepts/assurance/reservations.md)); the
warranty is a claim written for them.

## What this is and is not

(`agent:record`)

- **It is evidence, not insurance.** Nothing here prices a risk, pools
  one or pays a premium. It gives whoever does that the agent's limits
  as the ledger enforces them, and a history the operator cannot edit.
- **The first part is the ledger's, the second the operator's.** What
  the operator says of its sessions is its word. It is bound to the
  sessions it was counted from by a digest, so sessions shown later can
  be held to it, but sessions the operator did not keep are not in it.
- **The ledger shows what was done on it.** A payment made another way
  (a card, another chain) is not among the payments counted. A payment
  the charter refused is counted in the operator's part.
- **A warranty covers what an adjudicator will rule on.** Its words are
  the operator's, a challenge is decided by the adjudicator it names with
  nothing rerun, and an upheld challenge slashes the bond: those who
  relied are compensated what they reserved, up to what the bond holds.
- **A record is of one chain and one span.** It says nothing of what the
  agent did before the span, after it, or elsewhere.
