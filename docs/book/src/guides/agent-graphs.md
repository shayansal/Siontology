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
fan-out       split → part for each of its parts; part → combine (waits for all)
work-check    work → check; check → work if it fails
draft-review  draft → review; review → draft if it fails
```

- **fan-out**: `split` divides the task into parts (its answer keeps to a
  schema: `{"parts": [...]}`), a `part` runs for each, side by side, and
  `combine` waits for them all and puts their reports together.
- **work-check**: `work` does the task, `check` (which did not do it)
  checks it and answers `{"pass": …, "why": …}`; a failed check goes back
  to `work`, which is told why, at most three times.
- **draft-review**: `draft` writes, `review` reviews as an editor; a
  review that does not pass goes back to the draft.

## A run

```text
$ sion agent graph run team --task 'Summarise the three notes in notes/, and say where they disagree.' --template fan-out
graph aabe3a3e2112a3f6 (fan-out) on profile team
  split (session dd9231a4eb1b036f): {"parts": ["Summarise notes/pricing.md", "Summarise notes/hosting.md", "Summarise notes/support.md"]}
  part [Summarise notes/pricing.md] (session ef9372c47b1cbed5): Pricing: 12 USD a month per seat, billed yearly, phone support included.
  part [Summarise notes/hosting.md] (session 2a030d4728a308b1): Hosting: EU only, in Frankfurt.
  part [Summarise notes/support.md] (session bd3fde3ddf77de77): Support: by email, within one working day; no phone support.
  combine (session c2ac4281cd515b59): 12 USD a month per seat, billed yearly; hosted in Frankfurt only; support by email within a working day. The notes disag
answer: 12 USD a month per seat, billed yearly; hosted in Frankfurt only; support by email within a working day. The notes disagree on phone support: pricing.md includes it, support.md says there is none.
$ sion agent graph show aabe3a3e
graph aabe3a3e2112a3f6 (fan-out) on profile team
  split (session dd9231a4eb1b036f): {"parts": ["Summarise notes/pricing.md", "Summarise notes/hosting.md", "Summarise notes/support.md"]}
  part [Summarise notes/pricing.md] (session ef9372c47b1cbed5): Pricing: 12 USD a month per seat, billed yearly, phone support included.
  part [Summarise notes/hosting.md] (session 2a030d4728a308b1): Hosting: EU only, in Frankfurt.
  part [Summarise notes/support.md] (session bd3fde3ddf77de77): Support: by email, within one working day; no phone support.
  combine (session c2ac4281cd515b59): 12 USD a month per seat, billed yearly; hosted in Frankfurt only; support by email within a working day. The notes disag
answer: 12 USD a month per seat, billed yearly; hosted in Frankfurt only; support by email within a working day. The notes disagree on phone support: pricing.md includes it, support.md says there is none.
$ sion agent sessions
2a030d4728a308b1  agent-9  done  Do your part of the task, and only that part, and report wha
bd3fde3ddf77de77  agent-9  done  Do your part of the task, and only that part, and report wha
c2ac4281cd515b59  agent-9  done  Put the parts' reports together into one answer to the task:
dd9231a4eb1b036f  agent-9  done  Split the task into parts that can be done apart, each witho
ef9372c47b1cbed5  agent-9  done  Do your part of the task, and only that part, and report wha
```

Each node's session is kept like any other, and the graph's run under
`$SION_HOME/agents/graphs`, a wave at a time, so `sion agent graph show`
prints where it stands. A node whose session read content from outside
(a web page, another agent's message) passes that on: the nodes its
answer reaches run as content from outside too, tainted from the start,
as a tainted agent's subagents are.

## Choosing a shape

`--route` asks the profile's model first how to work on the task: one
agent, or a template. It may choose only among the templates that have
beaten one agent in one of the profile's comparisons (below), and none
has yet, so the task goes to one agent without the model being asked:

```text
$ sion agent graph run team --task 'Summarise the three notes in notes/, and say where they disagree.' --route
route: single (no template has beaten one agent in profile team's comparisons (sion agent graph eval))
session 0f586605fb706dfc for agent 1fb0e606
  → read_file {"path":"notes/pricing.md"}
  ← # Pricing

    12 USD a month per seat, billed yearly. Phone support is included.
agent: Pricing: 12 USD a month per seat, billed yearly, phone support included.
session 0f586605fb706dfc: done
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
single           5/15  passed   33.3% [15.2–58.3]  repeats [1, 1, 1, 1, 1]  0 tokens a run
vote-3           5/15  passed   33.3% [15.2–58.3]  repeats [1, 1, 1, 1, 1]  0 tokens a run  vs single +0.0 points [-30.9, +30.9]
fan-out         15/15  passed  100.0% [79.6–100.0]  repeats [3, 3, 3, 3, 3]  0 tokens a run  vs single +66.7 points [+34.4, +84.8]
work-check       5/15  passed   33.3% [15.2–58.3]  repeats [1, 1, 1, 1, 1]  0 tokens a run  vs single +0.0 points [-30.9, +30.9]
earned: fan-out, above both baselines by Newcombe's 95% interval
kept: $SION_HOME/agents/evals/team/9c81beef008297fb2cafdb18e0f0c119d3119d10680606a576de6075bfeb18a9.json
```

Each line gives the runs that passed and their share, with its 95% Wilson
interval, the passes of each repeat (to see how much runs vary), the
tokens a run used (a scripted model reports none) and the difference from
one agent with its interval. The interval treats every run as
independent, though the repeats of a case are not, so it is narrower than
it would be over as many different cases. The comparison is kept under
`$SION_HOME/agents/evals/<profile>`, the latest for each set, and
`--route` now offers the fan-out, while the router still keeps a task that
does not divide to one agent:

```text
$ sion agent graph run team --task 'Summarise the three notes in notes/, and say where they disagree.' --route
route: fan-out (three notes, each read without the others)
graph 04401550d2af437d (fan-out) on profile team
  split (session 7c7a4829b4d99376): {"parts": ["Summarise notes/pricing.md", "Summarise notes/hosting.md", "Summarise notes/support.md"]}
  part [Summarise notes/pricing.md] (session 4d8e0df1857afe94): Pricing: 12 USD a month per seat, billed yearly, phone support included.
  part [Summarise notes/hosting.md] (session 0698e1ab57bf5e32): Hosting: EU only, in Frankfurt.
  part [Summarise notes/support.md] (session 420b2c63e7e67441): Support: by email, within one working day; no phone support.
  combine (session 0cf981b7cfc66b77): 12 USD a month per seat, billed yearly; hosted in Frankfurt only; support by email within a working day. The notes disag
answer: 12 USD a month per seat, billed yearly; hosted in Frankfurt only; support by email within a working day. The notes disagree on phone support: pricing.md includes it, support.md says there is none.
$ sion agent graph run team --task 'How much is in the agent's account?' --route
route: single (one lookup: one agent is enough)
session c1a4a48b05044c47 for agent 1fb0e606
  → ledger_balance {}
  ← {"budget_remaining_usd":"100","burst_usd":"50","can_spend_now_usd":"50","expires_at_pulse":100000,"fee_balance_usd":"0","max_per_payment_usd":"10","payees":["bob"],"refill_usd_per_pulse":"0.05","status":"active"}
  → ledger_balance {}
  ← {"budget_remaining_usd":"100","burst_usd":"50","can_spend_now_usd":"50","expires_at_pulse":100000,"fee_balance_usd":"0","max_per_payment_usd":"10","payees":["bob"],"refill_usd_per_pulse":"0.05","status":"active"}
agent: The account holds 100 USD.
session c1a4a48b05044c47: done
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
$ sion agent judge c1a4a48b05044c47 --model script:judge.json --calibrate
calibration: 3 of 4 labelled steps the judge was not shown, judged as labelled
session c1a4a48b05044c47: 3 steps
  1. progress  ledger_balance {} → {"budget_remaining_usd":"100","burst_usd":"50","can_spend_now_usd":"50","expires: it reads the balance
  2. stalled   ledger_balance {} → {"budget_remaining_usd":"100","burst_usd":"50","can_spend_now_usd":"50","expires: it reads the same balance again
  3. progress  answer: The account holds 100 USD.: it answers with the balance it read
2 of 3 steps moved the task on, 1 stalled, none harmful
```

A judge's verdict is a model's judgment, not a check: it can be wrong,
and what the steps quote can steer it, though it is told that the steps
are data.

## A graph of your own

A graph is a JSON file: its nodes (an id, instructions, and optionally
`tools`, `model`, `output` (a JSON schema its answer keeps to) and
`max_visits`), its edges (`from`, `to`, and `when`: `always`, `pass`,
`fail`, `{"field": {"field": …, "equals": …}}`, or `{"each": {"field":
…}}` to fan out over a list in the answer), its `start`, and optionally
its `end` (the node whose answer is the graph's) and `max_runs` (24 by
default, 100 at most). An edge that depends on a node's answer needs that
node to give one that keeps to a schema; a join (`"kind": "join"`) waits
until nothing that leads to it is left to run. The graph is checked
before it runs: every node reachable, every edge between its nodes, the
limits within reason (16 nodes, a node run at most 12 times, 12 parts to
a fan-out). Run it with `sion agent graph run <profile> --graph <file>
--task …`, and compare it with one agent with `sion agent graph eval
<profile> --graph <file>`.

## What this does not do

- A node that waits for a person's approval stops the graph there: its
  session waits like any other (`sion agent continue <session>`), and the
  graph's run stays stopped where it was.
- Nodes run on this machine, as sessions of one profile's agent; work for
  another agent is hired with a Task Charter instead
  ([Hiring another agent](agent-teams.md#hiring-another-agent)).
- The router is a model's judgment, among what a comparison let through:
  it can be wrong, which is why its default is one agent.
