# Graphs of agents

One agent with good tools is the default, and for most tasks the right
one: on the same budget, several agents do better only when a task
divides into parts that need none of the others' results, or when what
one agent made is better checked by another that did not make it.
`sion agent graph` runs a profile's agent as the nodes of a small graph
for those tasks: each node a session of its own, with its own
instructions (and, if the graph says so, its own tools or model), and
edges that pass a node's answer on.

Every output below is from a real run on a local devnet, with a profile
`team` whose agent works in a workspace of three notes. The model is a
scripted one, so the answers are the script's (each node's by what it is
asked), while the calls the nodes make are real; with a real model only
the profile's `--model` changes.

## The templates

`sion agent graph templates` lists the graphs the runtime knows, and
`sion agent graph templates <name>` prints one as JSON, to start a graph
of your own from:

```text
$ sion agent graph templates
fan-out           split → part for each of its parts; part → combine (waits for all)
work-check        work → check; check → work if it fails
draft-review      draft → review; review → draft if it fails
self-consistency  solve (×3) → vote (votes)
orchestrator      plan → research for each of its questions; plan → verify (waits for all); research → verify (waits for all); verify → write if it passes; verify → plan if it fails
```

- **fan-out**: `split` divides the task into parts (its answer keeps to a
  schema: `{"parts": [...]}`), a `part` runs for each, side by side, and
  `combine` waits for them all and puts their reports together.
- **work-check**: `work` does the task, `check` (which did not do it)
  checks it and answers `{"pass": …, "why": …}`; a failed check goes back
  to `work`, which is told why, at most three times.
- **draft-review**: `draft` writes, `review` reviews as an editor; a
  review that does not pass goes back to the draft.
- **self-consistency**: `solve` runs in three copies side by side, each
  told which attempt it is and to work it out on its own, and `vote`, a
  node that runs no session, answers with the answer given most often
  (case, spacing and a closing full stop aside; a tie goes to the
  earliest) and how many gave it. Answers that agree in other words count
  apart, so it suits questions with one short answer.
- **orchestrator**: `plan` splits the task into questions
  (`{"questions": [...], "known": [...]}`), a `research` node looks each
  up with only the tools that read (files, the ledger, memory: nothing
  that writes, reaches outside or pays), and `verify` waits for them all
  and checks them against each other and the task, as someone who looked
  up none of them. Findings it passes go to `write`; otherwise it goes back
  to `plan` once, with what was missing and what holds, which the plan
  lists as known for the second verification.

## A run

```text
$ sion agent graph run team --task 'Summarise the three notes in notes/, and say where they disagree.' --template fan-out
graph c9cfd9a698d9a752 (fan-out) on profile team
  split (session 747e6f07702b307f): {"parts": ["Summarise notes/pricing.md", "Summarise notes/hosting.md", "Summarise notes/support.md"]}
  part [Summarise notes/pricing.md] (session 2c35d0816345dee1): Pricing: 12 USD a month per seat, billed yearly, phone support included.
  part [Summarise notes/hosting.md] (session 24e0cf1d1ea8f1b3): Hosting: EU only, in Frankfurt.
  part [Summarise notes/support.md] (session 02242d227c4c969d): Support: by email, within one working day; no phone support.
  combine (session 6f89e9de2bf9c976): 12 USD a month per seat, billed yearly; hosted in Frankfurt only; support by email within a working day. The notes disag
answer: 12 USD a month per seat, billed yearly; hosted in Frankfurt only; support by email within a working day. The notes disagree on phone support: pricing.md includes it, support.md says there is none.
$ sion agent graph show c9cfd9a6
graph c9cfd9a698d9a752 (fan-out) on profile team
  split (session 747e6f07702b307f): {"parts": ["Summarise notes/pricing.md", "Summarise notes/hosting.md", "Summarise notes/support.md"]}
  part [Summarise notes/pricing.md] (session 2c35d0816345dee1): Pricing: 12 USD a month per seat, billed yearly, phone support included.
  part [Summarise notes/hosting.md] (session 24e0cf1d1ea8f1b3): Hosting: EU only, in Frankfurt.
  part [Summarise notes/support.md] (session 02242d227c4c969d): Support: by email, within one working day; no phone support.
  combine (session 6f89e9de2bf9c976): 12 USD a month per seat, billed yearly; hosted in Frankfurt only; support by email within a working day. The notes disag
answer: 12 USD a month per seat, billed yearly; hosted in Frankfurt only; support by email within a working day. The notes disagree on phone support: pricing.md includes it, support.md says there is none.
$ sion agent sessions
02242d227c4c969d  agent-9  done  Do your part of the task, and only that part, and report wha
24e0cf1d1ea8f1b3  agent-9  done  Do your part of the task, and only that part, and report wha
2c35d0816345dee1  agent-9  done  Do your part of the task, and only that part, and report wha
6f89e9de2bf9c976  agent-9  done  Put the parts' reports together into one answer to the task:
747e6f07702b307f  agent-9  done  Split the task into parts that can be done apart, each witho
```

Each node's session is kept like any other, and the graph's run under
`$SION_HOME/agents/graphs`, a wave at a time, so `sion agent graph show`
prints where it stands. A node whose session read content from outside
(a web page, another agent's message) passes that on: the nodes its
answer reaches run as content from outside too, tainted from the start,
as a tainted agent's subagents are.

A node may run in copies (`copies`), each told which attempt it is, and a
vote takes the answer they give most often, without a model. Here the
script's third copy answers otherwise:

```text
$ sion agent graph run team --task 'How many seats of the plan A does 36 USD buy, a month?' --template self-consistency
graph f955b4f1240ef6c6 (self-consistency) on profile team
  solve (attempt 1 of 3) (session cdf8d6593a0d3982): 4
  solve (attempt 2 of 3) (session 6a29ec9671bb3241): 4
  solve (attempt 3 of 3) (session cbadd5a6fc46e2d0): 3
  vote (given by 2 of 3): 4
answer: 4
```

## Replayed, and pinned to its graph

A run is kept with the digest of the graph it began with, and a kept run
whose graph no longer has that digest is refused: the graph cannot be
changed under a run that is going on. `sion agent graph replay <run>` runs
the graph again with each node answered as the run recorded it (the node
asked the same, with the same inputs), and checks that the graph routes
those answers the same way, to the same end. Here a draft is reviewed,
revised once and passed, and the replay goes as the run did:

```text
$ sion agent graph run team --task 'Write a one-line tagline for the Frankfurt hosting.' --template draft-review
graph 093cdef45d1805ed (draft-review) on profile team
  draft (session 0991836a083f659f): Your data stays in Frankfurt.
  review (session aa57aa0026f9e13e): {"pass": false, "why": "say that it never leaves the EU"}
  draft (session 78fb4a396d33d65c): Your data stays in Frankfurt, and never leaves the EU.
  review (session d9c29648af3e8d5b): {"pass": true, "why": "it says where the data stays"}
answer: Your data stays in Frankfurt, and never leaves the EU.
$ sion agent graph replay 093cdef4
graph run 093cdef45d1805ed replays as it ran: 4 node runs, routed the same way, to the same end (graph 7eb5d462)
```

`--graph <file>` replays it through another graph, to see what a change
would have done to a run that happened: without the loop back to the
draft, the run would have ended after the first review. A replay that
goes another way exits with an error, saying at which node run:

```text
$ sion agent graph replay 093cdef4 --graph no-loop.json
error: verification failed: graph run 093cdef45d1805ed replays otherwise at node run 3: the graph ended before draft
[exit 1]
```

A replay shows how the graph routes what was recorded, not that its
models would answer the same again (`agent:graph-replay`).

## A node that waits for a person

A node's session waits for a person's approval like any other session:
the graph's run stops at it, and the rest of its wave is kept. Here a
profile `payer` (the same agent, at autonomy L5, which asks a person
before spending) runs a graph of two nodes from a file: `look` may use
only the tools that read (`"read_only": true`), so the script's attempt to
pay from it finds no such tool, and `pay` waits:

```text
$ sion agent graph run payer --task 'Settle what we owe bob.' --graph look-then-pay.json
graph 7844bedc3e9005e8 (look-then-pay) on profile payer
  look (session 3fd0474915b2508c): 1 USD is owed to bob, for the review.
  pay: failed: session fb652d88382f5a12 waits for a person's approval of ledger_pay
stopped: pay failed: session fb652d88382f5a12 waits for a person's approval of ledger_pay
$ sion agent trace 3fd0474915b2508c | grep -o 'there is no tool named [a-z_]*'
there is no tool named ledger_pay
$ sion agent graph continue 7844bedc
graph 7844bedc3e9005e8 (look-then-pay) on profile payer
  look (session 3fd0474915b2508c): 1 USD is owed to bob, for the review.
  pay: failed: session fb652d88382f5a12 waits for a person's approval of ledger_pay
stopped: pay failed: session fb652d88382f5a12 waits for a person's approval of ledger_pay
session fb652d88382f5a12 waits for a person's approval of ledger_pay: approve its call (sion agent continue fb652d88382f5a12 --approve) or decline it (--decline <why>), then continue this run again
```

Once the session is approved and has run on, `sion agent graph continue`
takes its answer as the node's and goes on with the run:

```text
$ sion agent continue fb652d88382f5a12 --approve
  → ledger_pay {"amount":"1","purpose":"the review","to":"bob"} (approved by cli:operator)
  ← Pulse 10: Done: agent paid 1 USD to bob (receipt claim 99954041); fee 0.0012 USD. Transaction 357f670b….
agent: Paid bob 1 USD for the review.
session fb652d88382f5a12: done
$ sion agent graph continue 7844bedc
graph 7844bedc3e9005e8 (look-then-pay) on profile payer
  look (session 3fd0474915b2508c): 1 USD is owed to bob, for the review.
  pay (session fb652d88382f5a12): Paid bob 1 USD for the review.
answer: Paid bob 1 USD for the review.
```

A read-only node gets the tools whose declared effect is to read, so it
relies on each tool declaring what its calls can do
(`agent:read-only-node`).

## Choosing a shape

`--route` asks the profile's model first how to work on the task: one
agent, or a template. It may choose only among the templates that have
beaten one agent in one of the profile's comparisons (below), and none
has yet, so the task goes to one agent without the model being asked:

```text
$ sion agent graph run team --task 'Summarise the three notes in notes/, and say where they disagree.' --route
route: single (no template has beaten one agent in profile team's comparisons (sion agent graph eval))
session b5a8946ce74718a5 for agent cec99d53
  → read_file {"path":"notes/pricing.md"}
  ← # Pricing

    12 USD a month per seat, billed yearly. Phone support is included.
agent: Pricing: 12 USD a month per seat, billed yearly, phone support included.
session b5a8946ce74718a5: done
```

`--untested` lets the router choose among every template, measured or
not.

## Measured against one agent

`sion agent graph eval` compares ways of working through a set of tasks
with the profile's models: one agent; one agent several times
(`--samples`, three by default), its answers put to a vote; and each
graph given. Every way gets the same token budget for a case
(`--budget-tokens`, its sessions together: a vote's samples split it,
and the nodes of a graph's wave share what is left), every case runs
several times (`--runs`), and the cases' stand-in tools and checks are
those of an [evaluation set](skill-improvement.md), for no skill in
particular. A graph earns the profile's traffic only if its pass rate
beats both baselines by Newcombe's 95% interval for the difference of two
proportions: the lower end of the interval above zero.

Here, on three questions about plans that need one or two lookups each,
the script has one agent look up one thing at a time, so it answers the
question that needs one lookup and misses the two that need two; the
fan-out's parts look each up. The pass rates are therefore the script's,
chosen to show how a comparison reads, and say nothing about any real
model:

```text
$ sion agent graph eval team --set lookups.json --template fan-out --template work-check --runs 5
set plan-lookups: 3 cases, each run 5 times by each way of working, on 30000 tokens a case
single           5/15  passed   33.3% [15.2–58.3]  repeats [1, 1, 1, 1, 1]  0 tokens a run  pass^5 33.3%
vote-3           5/15  passed   33.3% [15.2–58.3]  repeats [1, 1, 1, 1, 1]  0 tokens a run  pass^5 33.3%  vs single +0.0 points [-30.9, +30.9]
fan-out         15/15  passed  100.0% [79.6–100.0]  repeats [3, 3, 3, 3, 3]  0 tokens a run  pass^5 100.0%  vs single +66.7 points [+34.4, +84.8]
work-check       5/15  passed   33.3% [15.2–58.3]  repeats [1, 1, 1, 1, 1]  0 tokens a run  pass^5 33.3%  vs single +0.0 points [-30.9, +30.9]
earned: fan-out, above both baselines by Newcombe's 95% interval
kept: $SION_HOME/agents/evals/team/9c81beef008297fb2cafdb18e0f0c119d3119d10680606a576de6075bfeb18a9.json
```

Each line gives the runs that passed and their share, with its 95% Wilson
interval, the passes of each repeat (to see how much runs vary), the
tokens a run used (a scripted model reports none), pass^k and the
difference from one agent with its interval. pass^k, for k the number of
runs of each case, is the chance that k runs of a case all pass, averaged
over the cases (τ-bench's estimate, C(c, k) / C(n, k) for a case passed
c times in n): a way that passes a case only sometimes scores lower here
than in its pass rate. A script answers the same every time, so its
pass^k is its pass rate. The interval treats every run as
independent, though the repeats of a case are not, so it is narrower than
it would be over as many different cases. The comparison is kept under
`$SION_HOME/agents/evals/<profile>`, the latest for each set, and
`--route` now offers the fan-out, while the router still keeps a task that
does not divide to one agent:

```text
$ sion agent graph run team --task 'Summarise the three notes in notes/, and say where they disagree.' --route
route: fan-out (three notes, each read without the others)
graph f5dd3c9a7b912e80 (fan-out) on profile team
  split (session 0c7a94c011125e25): {"parts": ["Summarise notes/pricing.md", "Summarise notes/hosting.md", "Summarise notes/support.md"]}
  part [Summarise notes/pricing.md] (session 68bfef4fdabeb22a): Pricing: 12 USD a month per seat, billed yearly, phone support included.
  part [Summarise notes/hosting.md] (session 9fa2cd0d303972d8): Hosting: EU only, in Frankfurt.
  part [Summarise notes/support.md] (session 4e5a1b92605e24a1): Support: by email, within one working day; no phone support.
  combine (session 91ad0ac4e1c41bf6): 12 USD a month per seat, billed yearly; hosted in Frankfurt only; support by email within a working day. The notes disag
answer: 12 USD a month per seat, billed yearly; hosted in Frankfurt only; support by email within a working day. The notes disagree on phone support: pricing.md includes it, support.md says there is none.
$ sion agent graph run team --task 'How much is in the agent's account?' --route
route: single (one lookup: one agent is enough)
session 9f10c27e93409d0a for agent cec99d53
  → ledger_balance {}
  ← {"budget_remaining_usd":"99","burst_usd":"50","can_spend_now_usd":"49.2","expires_at_pulse":100000,"fee_balance_usd":"0.9988","max_per_payment_usd":"10","payees":["bob"],"refill_usd_per_pulse":"0.05","status":"active"}
  → ledger_balance {}
  ← {"budget_remaining_usd":"99","burst_usd":"50","can_spend_now_usd":"49.2","expires_at_pulse":100000,"fee_balance_usd":"0.9988","max_per_payment_usd":"10","payees":["bob"],"refill_usd_per_pulse":"0.05","status":"active"}
agent: The account holds 100 USD.
session 9f10c27e93409d0a: done
```

What a template earns, it earns for the profile: the router may propose
it for any task, as the model judges the task. A comparison on stand-in
tools measures the profile's models on those cases, not its real tools.

## Judged step by step

A pass rate says whether runs ended right, not how they got there.
`sion agent judge <session> --model <model>` has a model read a session's
steps (each call, the start of what came back, then the answer) and say
of each whether it moved the task on, stalled or did harm, and why. The
judge is shown labelled steps first, and `--calibrate` measures it on
four labelled steps it was not shown before it judges. Here the scripted
judge misses one of the four, which says how far to trust what it says
next. `sion agent graph eval --judge <model>` judges every run of a
comparison the same way and reports the steps beside the pass rates:

```text
$ sion agent judge 9f10c27e93409d0a --model script:judge.json --calibrate
calibration: 3 of 4 labelled steps the judge was not shown, judged as labelled
session 9f10c27e93409d0a: 3 steps
  1. progress  ledger_balance {} → {"budget_remaining_usd":"99","burst_usd":"50","can_spend_now_usd":"49.2","expire: it reads the balance
  2. stalled   ledger_balance {} → {"budget_remaining_usd":"99","burst_usd":"50","can_spend_now_usd":"49.2","expire: it reads the same balance again
  3. progress  answer: The account holds 100 USD.: it answers with the balance it read
2 of 3 steps moved the task on, 1 stalled, none harmful
```

A judge's verdict is a model's judgment, not a check: it can be wrong,
and what the steps quote can steer it, though it is told that the steps
are data.

## A graph of your own

A graph is a JSON file: its nodes (an id, instructions, and optionally
`tools`, `model`, `output` (a JSON schema its answer keeps to),
`max_visits`, `copies` (runs side by side each time it is reached, at
most its `max_visits`) and `read_only`), its edges (`from`, `to`, and `when`: `always`, `pass`,
`fail`, `{"field": {"field": …, "equals": …}}`, or `{"each": {"field":
…}}` to fan out over a list in the answer), its `start`, and optionally
its `end` (the node whose answer is the graph's) and `max_runs` (24 by
default, 100 at most). An edge that depends on a node's answer needs that
node to give one that keeps to a schema. A join (`"kind": "join"`) waits
until nothing that leads to it is left to run, or, with `"wait":
"first"`, runs as soon as an answer reaches it (with what reached it in
the same wave), or with `"wait": {"quorum": n}` once n have; a join that
does not wait for all runs once, and what reaches it later is not passed
on. A vote (`"kind": "vote"`) is a join that answers without a model. The graph is checked
before it runs: every node reachable, every edge between its nodes, the
limits within reason (16 nodes, a node run at most 12 times, 12 parts to
a fan-out). Run it with `sion agent graph run <profile> --graph <file>
--task …`, and compare it with one agent with `sion agent graph eval
<profile> --graph <file>`.

## What this does not do

- A node that waits for a person's approval stops the graph there until
  its session is decided and `sion agent graph continue` is run: nothing
  goes on by itself.
- Nodes run a wave at a time: a join that takes the first answer takes
  the first wave's, not the first to finish.
- Nodes run on this machine, as sessions of one profile's agent; work for
  another agent is hired with a Task Charter instead
  ([Hiring another agent](teams.md#hiring-another-agent)).
- The router is a model's judgment, among what a comparison let through:
  it can be wrong, which is why its default is one agent.
