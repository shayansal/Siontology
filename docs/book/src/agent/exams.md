# Sealed exams: what an agent can do, on tasks it could not have seen

A score on a public benchmark says little about an agent. The tasks may
have been seen, trained on, or fitted to by whoever wrote the prompt, and
the number is its maker's word. A sealed exam puts three things in an
order the ledger fixes, so that they cannot have been:

1. **The tasks are sealed.** Their author encrypts them and publishes a
   claim with their digest. The key is split among trustees, and the
   tasks stay unreadable until enough trustees release their shares.
2. **Agents are entered.** An entry is a claim that fixes an agent in
   full: its models, what its operator tells it, and what it has learned.
   It counts only if the ledger issued it before the exam opened.
3. **The exam opens, and anyone runs it.** With the shares released the
   tasks are public. Anyone can run an entry and publish what it found,
   and anyone can run it again.

Outputs on this page are from real runs of the commands on a local
devnet with scripted models, with long identifiers and paths shortened
with `…`.

## Sealing an exam

The tasks are a set like the ones an agent is
[improved against](self-improvement.md): each task has stand-in tools
with fixed answers and checks that a machine applies. The author seals
them and names the trustees:

```text
$ sion agent exam seal books-2026 --tasks books.json \
    --about "Keeping the books: totals and open invoices." \
    --trustee bob --trustee carol --trustee dave --threshold 2 --runs 2 \
    --wallet alice --out exam
sealed exam books-2026: 12 task(s), claim 1d4fe43b…
  the exam, for anyone: exam/books-2026.exam.json
  the share of bob (a27e5e28), for that trustee alone: exam/books-2026.share-1.json
  the share of carol (905cae35), for that trustee alone: exam/books-2026.share-2.json
  the share of dave (9e57d9ed), for that trustee alone: exam/books-2026.share-3.json
  2 of 3 trustees open it (sion agent exam release)
```

The exam's file is for anyone. It holds what the claim says (the name,
the digest of the tasks, how many there are, the trustees, the digest of
each trustee's share, how many runs each task gets) and the encrypted
tasks. It holds nothing that can be read of the tasks: the digest is
taken over the tasks with a random salt that is sealed beside them, so
it does not confirm a guess either.

Each share file goes to its trustee and to no one else. Fewer than the
threshold of them tell nothing about the key.

## Entering an agent

An entrant enters a profile's agent while the exam is sealed:

```text
$ sion agent exam enter exam/books-2026.exam.json careful --wallet erin
entered careful in exam books-2026: claim e5dfb706…
  fixed: its models, what you tell it and what it has learned, in full
```

The entry is public. It holds the agent's models by name, its
instructions and what it has learned, in full, and whether it says what
it expects of its calls. That is what a runner needs to run the same
agent, and it is what stops an entrant from changing the agent once the
tasks are known.

Until the exam opens it cannot be run, by anyone:

```text
$ sion agent exam run exam/books-2026.exam.json e5dfb706
error: exam books-2026 is sealed: 0 of the 2 shares that open it are released
```

## Opening it

Each trustee releases its share in a claim. A share counts only if it is
the one the exam's claim names for that trustee:

```text
$ sion agent exam release exam/books-2026.exam.json exam/books-2026.share-1.json --wallet bob
released your share of exam books-2026: claim 164d7664
  1 of the 2 shares that open it are released
$ sion agent exam release exam/books-2026.exam.json exam/books-2026.share-2.json --wallet carol
released your share of exam books-2026: claim 2d3653e2
  the exam is open, as of Pulse 53: anyone can read its tasks
```

The exam opened in the Pulse of the share that completed the threshold.
From then on the tasks can be read by anyone (`sion agent exam tasks`
writes them to a file), and an agent entered now has had the chance to
see them:

```text
$ sion agent exam enter exam/books-2026.exam.json careful --wallet grace
error: exam books-2026 opened at Pulse 53: an entry made now does not count
```

An entry published anyway, around the command, is on the ledger with its
Pulse, and every reader sees that it came after the opening and does not
count it.

## Running an entry

Anyone runs an entry on the opened tasks, with their own access to the
entry's models, and with `--wallet` publishes what they found:

```text
$ sion agent exam run exam/books-2026.exam.json e5dfb706 --wallet grace
ran careful on exam books-2026: 24/24 runs passed, 12 of 12 tasks every time
published in claim 532883d9…
```

The run gives the entry's agent the exam's stand-in tools and nothing
else: no ledger, no files, no web. The entry's instructions are a
stranger's words, and this is all they can reach.

## Reading the results

```text
$ sion agent exam results exam/books-2026.exam.json
exam books-2026 by 42424444: 12 task(s), 2 run(s) each; Keeping the books: totals and open invoices.
  opened at Pulse 53: 2 of its 3 trustees released their shares, 2 open it
  entry e5dfb706 by e3b2ce7b: careful on script:…/keeper.json (agent 30770114, entered at Pulse 33)
    run by others: 24/24 by d3411351
    run by its entrant: 24/24 by e3b2ce7b
  entry 7bae260c by d68572ba: hasty on script:…/keeper.json (agent 48073c0e, entered at Pulse 38)
    run by others: 12/24 by d3411351
```

The two agents run on the same model. The careful one had been told
that the tool answers in cents; the hasty one had not, and failed the six
tasks that ask for a total. Results are grouped by who ran them. An entrant's run of its own entry
is one result among others, and it is shown apart: a reader looks at
what runners other than the entrant found, and can run the entry itself.

## Tasks no single party has read

An exam's author has read its tasks. Where that is not acceptable,
several authors each seal an exam of their own and the agent is entered
in all of them. No author has read another's tasks. The same agent is
recognised across exams by the digest of what its entries fix, and
`results` given several exams sets what others found side by side:

```text
$ sion agent exam results exam/books-2026.exam.json exam/ledgers-2026.exam.json
…
across the 2 exams:
  agent 30770114 (careful), as others ran it: books-2026: 24/24; ledgers-2026: 24/24
  agent 48073c0e (hasty), as others ran it: books-2026: 12/24; ledgers-2026: not entered
```

An agent that does well only on one author's tasks shows in that line.

## What an exam shows, and what it does not

(`agent:sealed-exams`)

- **The order is the ledger's.** That the entry came before the opening,
  and that the tasks are the ones committed to before either, is read
  from claims and their Pulses. It does not rest on anyone's word.
- **The author has read the tasks**, and could have told an entrant or
  entered an agent of its own. Several authors bound what any one of
  them can do; nothing here stops all of them from colluding.
- **Any threshold of trustees can read the tasks early** by putting
  their shares together off the ledger. Fewer cannot. If fewer than the
  threshold ever release, the exam never opens.
- **An exam is spent once it opens.** Its tasks are public from then on,
  which is what lets anyone check a result, and why it cannot be used
  again.
- **An entry fixes words and the names of models, not the model behind a
  name.** A hosted model can change under its name, and each runner uses
  its own access to it. Two honest runners can find different results,
  and the results show each runner's.
- **A result is its runner's word.** Nothing reruns it on the ledger.
  What makes it checkable is that the tasks are public and the entry is
  fixed. A runner "other than the entrant" is another account, which may
  be the same person: rely on a runner you know, or on your own run.
- **An entrant can enter several agents.** Every entry is public and
  listed, so entering ten and pointing at the best one shows.
- **An entry is public before the exam opens.** An entrant who does not
  want to show its instructions cannot enter. Entries sealed until the
  opening are not built.
- **The tasks are what a task set can express**: stand-in tools with
  fixed answers and checks a machine applies. It is not an exam of work
  on live systems.
- **Zero knowledge does not help here.** Showing that an agent passed
  tasks that stay hidden would take a proof of the model's whole
  computation, which is not practical for models of this size. So the
  tasks are opened and spent instead
  ([what is proven with zero knowledge](proofs.md)).
- Shares, entries and results are found in the node's events, which are
  not proven: a node that leaves one out hides it from that reader. Each
  one found is read again from its claim. A node keeps the events of its
  last Pulses only (256 on a devnet). An exam is refused from a node that
  no longer answers for the Pulse it was sealed in, so reading an exam
  that ran for long takes a node that kept its history.

This is tested on a devnet with scripted models (`agent_exam.rs`): an
exam sealed, entered, refused while sealed, opened by two of three
trustees, a share from a non-trustee and a wrong share refused, a late
entry not counted, runs by the entrant and by another account, and two
authors' exams side by side. It has not been run with a real model.
