# Proving rules were kept, without showing what was done

An operator can be asked whether its agent kept the rules by someone it
does not want to show its books to: an auditor, an insurer, a
counterparty. A zero-knowledge proof answers that. The operator commits
to the numbers, and proves that they keep a rule; whoever checks the proof
learns that the rule holds and nothing else about the numbers.

Two things are proven this way here: that a list of payments kept its
limits, and that the figures of an agent's record meet a threshold.

Outputs on this page are from real runs of the commands, on a local
devnet where a ledger is involved, with long identifiers and paths
shortened with `…`.

## Payments that kept their limits

`sion agent comply prove` takes a list of payments you will not show and
the rules they were under, and writes a certificate:

```text
$ cat payments.json
[{"amount": "5", "to": "acme-ltd"}, {"amount": "8.50", "to": "birch"}, {"amount": "2", "to": "acme-ltd"}]
$ sion agent comply prove --payments payments.json --max-payment 10 --total 20 \
    --payee acme-ltd --payee birch --payee cedar --for "audit of May" --out may.json
certified 3 payment(s): may.json
the openings are in may.openings.json: keep them to yourself, whoever holds them knows the payments
```

The certificate holds a commitment to each payment's amount and payee,
the rules, and the proofs. It is about 4 KB for these three payments, and
it holds no amount and no payee. Whoever is given it checks it with
nothing but the file:

```text
$ sion agent comply check may.json
for: audit of May
holds: 3 payment(s), each a whole amount of at most 10 USD
holds: they come to at most 20 USD in all
holds: each went to one of 3 payee(s): acme-ltd, birch, cedar
shown: how many payments there were, and that they keep these rules; not the amounts, nor who was paid what
not shown: that these are all the payments there were. That is their maker's word, unless the list was fixed elsewhere when they were made
```

There is no certificate of what is not so. With a limit of 8 USD:

```text
$ sion agent comply prove --payments payments.json --max-payment 8 --total 20 --for "audit of May" --out may.json
error: payment 2 is above the limit of 8 USD: there is no certificate to make
```

A certificate holds for the purpose it names (`--for`) and for no other,
and one whose rules, commitments or purpose were changed does not check.
`--wallet` commits to the certificate in a claim, which puts on the ledger
when the list of payments was fixed.

This ledger's own payments are in the clear, so an agent's record adds
those up for anyone without any of this ([Its record](record.md)). The
certificate is for payments the ledger does not show: by card, or on
another rail.

## A record's figures, sealed

What an operator says of its agent's sessions is part of the agent's
record. `sion agent record write --seal` commits to each of those figures
in place of showing it, and writes what opens them beside the record for
the operator to keep:

```text
$ sion agent record write 39112d7f… --seal --out rec.json --wallet alice
agent 39112d7f, signed for by edfe93d8, owned by 42424444
what the ledger shows, Pulse 0 to 10:
  1 payment(s) to 1 account(s), 5 USD in all, the largest 5 USD
  …
what its operator says, sealed: committed to and not shown (approved, asked, attacks_held, calls, calls_failed, checked, declined, done, expected, expected_held, payments_refused, sessions, signed)
digest 73df38c7…
written to rec.json
what opens its sealed figures is in rec.openings.json: keep it to yourself
committed to in claim 672b88d2…
```

`sion agent record prove` then shows whoever asks that the figures keep a
rule. A rule names figures, whole numbers and products of the two, joined
by `+`, and compares two sides with `<=`, `>=` or `==`:

```text
$ sion agent record prove rec.json --rule "calls_failed*2 <= calls" --rule "payments_refused <= 3" --rule "attacks_held == 0"
proven, without the figures: calls_failed*2 <= calls
proven, without the figures: payments_refused <= 3
proven, without the figures: attacks_held == 0
added to rec.json
$ sion agent record prove rec.json --rule "payments_refused == 0"
error: "payments_refused == 0" does not hold of this record's figures: there is no proof to make
```

The proofs are added to the record without changing what its owner
committed to, so different rules can be proven to different people from
the same record. `sion agent record check` verifies them with the rest:

```text
$ sion agent record check rec.json
holds: what the record says the ledger shows from Pulse 0 to 10 is what the ledger shows
holds: the charter is as the record has it
holds: its owner committed to this record in claim 672b88d2 at Pulse 13
its operator's word, sealed: 13 figures of its machine's sessions committed to and not shown (their digest ddb095f4726f7056…)
holds, proven without the figures: calls_failed*2 <= calls
holds, proven without the figures: payments_refused <= 3
holds, proven without the figures: attacks_held == 0
```

## What these proofs rest on

(`agent:zk-proofs`)

- **A proof is about the numbers committed to.** That those are the
  numbers of what happened is not something it shows. A certificate's
  payments are the ones its maker listed, and a record's figures are what
  its operator's machine counted. What can be shown is when they were
  fixed: a claim that commits to the certificate or the record puts that
  on the ledger.
- **Nothing is learned beyond the rule.** A commitment says nothing about
  its number, whatever computer looks at it. How many payments there
  were, and which rules were proven, is visible.
- **The proofs are sound unless discrete logarithms can be computed** in
  the group they are made in, as a large quantum computer could. They are
  for showing third parties that rules were kept. The ledger does not
  check them, and they are not used for anything that creates or moves
  money: a ledger with post-quantum accounts needs proofs that are sound
  against a quantum computer too.
- **The ledger's own amounts are not confidential.** Payments on it are in
  the clear. Confidential amounts on the ledger, checked by its
  validators, are not built.
- **Amounts are whole numbers of millionths of a USD below 2⁶⁴.** A
  certificate covers at most 1,000 payments, and a list of payees at most
  1,024 names.

How the proofs are made is in the crate's own page
([`sion-zk`](https://github.com/shayansal/Siontology/tree/main/crates/sion-zk)):
Pedersen commitments, Bulletproofs range proofs and a one-of-many proof
of membership, with no trusted setup.
