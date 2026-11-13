# Datasets of checked work

Models are trained on whatever could be gathered. Of a given example
nobody can say whether the work in it was right, whose it was, or
whether its maker agreed to its use. `sion agent dataset` builds a
dataset the other way round: every example is a session someone chose to
contribute, on stated terms, with a record of what checked the work in
it.

No model is trained here. What this produces is a file of examples and
the means to check where each came from.

Outputs on this page are from real runs of the commands on a local
devnet with scripted models, with long identifiers shortened with `…`.

## A dataset and its terms

A curator opens a dataset. Its card is a claim that says what the
dataset is for, the terms a contributor agrees to, and what counts as
checked:

```text
$ sion agent dataset open bookkeeping --about "Bookkeeping work that was checked." \
    --terms terms.txt --take task --take check --wallet cora
opened dataset bookkeeping: claim 70112342…
  it takes work checked by: task, check; its terms' digest is 2e67d0c7edbf2929…
```

Work can have been checked in three ways, of different strength:

| | What it means | Whose word |
|---|---|---|
| `task` | A [Task Charter](../concepts/task-charters.md) was settled on the ledger with this work's output: the result was accepted under the rule the task's poster fixed, and the contributor or its agent was paid | the ledger's |
| `check` | The session ran with a check (`--verify`), and it passed | its operator's |
| `marked` | Its operator marked the session good | its operator's |

A dataset says which it takes. Unless it says otherwise it takes no
session that read content from outside (a web page, a file someone
sent), since that content may be someone else's.

## Contributing a session

Nothing is contributed without the contributor saying it agrees. Without
`--agree` the command shows the terms and stops:

```text
$ sion agent dataset contribute 70112342… 2b5a6eac… --wallet alice
dataset bookkeeping by 377c5c68 asks its contributors to agree to these terms:

You let anyone train models on what you contribute, and you say it is yours to give.

nothing was contributed: say --agree to contribute on these terms
```

With it, the session becomes an example: a line in chat format with its
tool calls and the tools the model was shown, as fine-tuning tools read
them. Here the session was work for a customer, paid through a task, and
the customer's name is taken out:

```text
$ sion agent dataset contribute 70112342… 2b5a6eac… --agree --task 5e408d94… \
    --redact "Dana Ruiz" --out given --wallet alice
contributed example 93631828 to dataset bookkeeping, on its terms: claim 6269e0a6
  checked by: task 5e408d94, settled on the ledger; its output is in the session
  1 piece(s) of text taken out
  the example is in given/93631828b4fa5deb.example.json: give it to the curator; nothing of it is on the ledger
```

The claim commits to the example by its digest, says what checked it and
agrees to the terms by their digest. The example itself is a file the
contributor gives the curator. `--withhold-system` leaves the
contributor's system prompt out of it.

A session is not contributed when it did not finish, its conversation
was compacted, it held confidential content, it holds a secret this
machine keeps, or nothing checked it. With `--task`, the task must be
settled, must have paid the contributor or an agent it registered, and
must have been settled with the output given.

A session checked only by its own check is contributed the same way,
without `--task`:

```text
$ sion agent dataset contribute 70112342… a588c60a… --agree --out given --wallet alice
contributed example 595bfbd0 to dataset bookkeeping, on its terms: claim 5465c1ce
  checked by: the session's own check, which passed
  the example is in given/595bfbd008b839b2.example.json: give it to the curator; nothing of it is on the ledger
```

## Building a version

The curator builds a version from the example files it was given. Each
file is checked against its contribution on the ledger and against what
it says checked it:

```text
$ sion agent dataset build 70112342… --from given --out built --wallet cora
built version 1 of dataset bookkeeping: 2 example(s), root dde41626097a1eff…, claim 2271dc37
  credit: 42424444 gave 2
  built/bookkeeping-v1.jsonl and built/bookkeeping-v1.manifest.json
```

A version is a claim: how many examples, the root of their digests, how
many each contributor gave and how many were checked in each way. An
example is left out, with the reason, when its file is not what its
contributor committed to, when it was contributed to another dataset or
on other terms, when the task it names did not pay its contributor or
was settled with another output, or when it was withdrawn. An example
contributed twice belongs to whoever contributed it first by the
ledger's order.

## Checking a version before training on it

Whoever is about to train on the file checks it against the ledger:

```text
$ sion agent dataset check built/bookkeeping-v1.manifest.json built/bookkeeping-v1.jsonl
holds: 2 example(s), each the one the version lists, in its order
holds: version 1 of dataset bookkeeping is its curator's (377c5c68), published at Pulse 46
holds: every one was contributed by the account credited with it, which agreed to the dataset's terms (2e67d0c7edbf2929…)
holds: 1 checked by a task: each task is settled on the ledger with the result named
  1 checked by the session's own check: their contributors' word
  credit: 42424444 gave 2
```

A line that was changed, added, left out or moved does not check.

## Withdrawing

A contributor can withdraw an example:

```text
$ sion agent dataset withdraw given/595bfbd008b839b2.example.json --wallet alice
withdrew example 595bfbd0: claim f0f1beea
  versions built from now on leave it out; one already built cannot be called back
$ sion agent dataset build 70112342… --from given --out built --wallet cora
built version 2 of dataset bookkeeping: 1 example(s), root cc28a16f534a10c2…, claim e6a4c6c2
  credit: 42424444 gave 1
  left out: example 595bfbd0: its contributor withdrew it
  built/bookkeeping-v2.jsonl and built/bookkeeping-v2.manifest.json
```

Checking the earlier version still holds, and notes the withdrawal:

```text
note: 1 example(s) were withdrawn after this version was built (595bfbd0): a later version leaves them out
```

## What a dataset shows, and what it does not

(`agent:checked-dataset`)

- **Agreement is the contributing account's.** The claim shows that the
  account agreed to the terms. It does not show that the content was the
  account's to give. A session holds what its user typed and what its
  tools returned, and other people's words can be in either.
- **What checked the work is of three strengths**, as the table says. A
  settled task shows that a result was accepted under the task's rule,
  and that is worth what the rule is worth. That the accepted output is
  in the session is checked word for word, and said where it is not.
- **Taking text out is by the exact text the contributor names.** A
  session with one of this machine's kept secrets is refused. Nothing
  finds personal data by itself.
- **The curator holds the examples.** They are handed over outside the
  ledger, which holds only their digests. Nothing here hides an example
  from the curator or from whoever the curator gives the file to.
- **Who contributed first is the ledger's; whose work it was is not.**
  For work checked by a task, the task's payment ties it to its
  contributor. For work checked only by its operator's word, whoever
  holds the example's file and contributes it first is credited.
- **A withdrawal reaches later versions only.** A version already built,
  and anything trained on it, cannot be called back.
- **Credit is a count.** Each version says how many examples each
  account gave. Nothing pays by it.
- **Nothing is trained here**, and that a model was trained on a version
  and nothing else is not shown by anything here. Zero knowledge is not
  used: digests and claims are enough to check a file, and a proof of
  what a model was trained on is not practical.
- Cards, contributions, withdrawals and versions are found by name in
  their makers' own indexes, in the ledger's state. Reading them does
  not depend on how long a node keeps its events.

This is tested on a devnet with scripted models (`agent_dataset.rs`):
two sessions contributed, one checked by a settled task with a name
taken out and one by its check; a session nothing checked, one only
marked good, a task that paid someone else and another output refused; a
copy contributed by someone else and a changed example left out; the
version checked, and a changed, missing, moved and miscredited one not;
a withdrawal, and the version built after it.
