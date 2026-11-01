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
graph 1a56c8f5d5cd7deb (fan-out) on profile team
  split (session f4c8ffb3b6e2fc61): {"parts": ["Summarise notes/pricing.md", "Summarise notes/hosting.md", "Summarise notes/support.md"]}
  part [Summarise notes/pricing.md] (session 2cffbe0de8a58491): Pricing: 12 USD a month per seat, billed yearly, phone support included.
  part [Summarise notes/hosting.md] (session 3ff1379d0cc319ff): Hosting: EU only, in Frankfurt.
  part [Summarise notes/support.md] (session f05b098f49aaaa20): Support: by email, within one working day; no phone support.
  combine (session 62f27d1034b98312): 12 USD a month per seat, billed yearly; hosted in Frankfurt only; support by email within a working day. The notes disag
answer: 12 USD a month per seat, billed yearly; hosted in Frankfurt only; support by email within a working day. The notes disagree on phone support: pricing.md includes it, support.md says there is none.
$ sion agent graph show 1a56c8f5
graph 1a56c8f5d5cd7deb (fan-out) on profile team
  split (session f4c8ffb3b6e2fc61): {"parts": ["Summarise notes/pricing.md", "Summarise notes/hosting.md", "Summarise notes/support.md"]}
  part [Summarise notes/pricing.md] (session 2cffbe0de8a58491): Pricing: 12 USD a month per seat, billed yearly, phone support included.
  part [Summarise notes/hosting.md] (session 3ff1379d0cc319ff): Hosting: EU only, in Frankfurt.
  part [Summarise notes/support.md] (session f05b098f49aaaa20): Support: by email, within one working day; no phone support.
  combine (session 62f27d1034b98312): 12 USD a month per seat, billed yearly; hosted in Frankfurt only; support by email within a working day. The notes disag
answer: 12 USD a month per seat, billed yearly; hosted in Frankfurt only; support by email within a working day. The notes disagree on phone support: pricing.md includes it, support.md says there is none.
$ sion agent sessions
2cffbe0de8a58491  agent-9  done  Do your part of the task, and only that part, and report wha
3ff1379d0cc319ff  agent-9  done  Do your part of the task, and only that part, and report wha
62f27d1034b98312  agent-9  done  Put the parts' reports together into one answer to the task:
f05b098f49aaaa20  agent-9  done  Do your part of the task, and only that part, and report wha
f4c8ffb3b6e2fc61  agent-9  done  Split the task into parts that can be done apart, each witho
```

Each node's session is kept like any other, and the graph's run under
`$SION_HOME/agents/graphs`, a wave at a time, so `sion agent graph show`
prints where it stands. A node whose session read content from outside
(a web page, another agent's message) passes that on: the nodes its
answer reaches run as content from outside too, tainted from the start,
as a tainted agent's subagents are.

## Choosing a shape

`--route` asks the profile's model first how to work on the task: one
agent, or one of the templates. The router is told that one agent is
best for most tasks, and a shape it may not give, or an answer it does
not give as asked, is taken as one agent:

```text
$ sion agent graph run team --task 'Summarise the three notes in notes/, and say where they disagree.' --route
route: fan-out (three notes, each read without the others)
graph e3445a13bb757cac (fan-out) on profile team
  split (session 7a02e14a69492de7): {"parts": ["Summarise notes/pricing.md", "Summarise notes/hosting.md", "Summarise notes/support.md"]}
  part [Summarise notes/pricing.md] (session 6763a300eff4aa40): Pricing: 12 USD a month per seat, billed yearly, phone support included.
  part [Summarise notes/hosting.md] (session 262c5312ea95778c): Hosting: EU only, in Frankfurt.
  part [Summarise notes/support.md] (session 40eefa00aa411992): Support: by email, within one working day; no phone support.
  combine (session 8647754c262cfe1a): 12 USD a month per seat, billed yearly; hosted in Frankfurt only; support by email within a working day. The notes disag
answer: 12 USD a month per seat, billed yearly; hosted in Frankfurt only; support by email within a working day. The notes disagree on phone support: pricing.md includes it, support.md says there is none.
$ sion agent graph run team --task 'How much is in the agent's account?' --route
route: single (one lookup: one agent is enough)
session 540c85409275bf16 for agent b60dbf62
  → ledger_balance {}
  ← {"budget_remaining_usd":"100","burst_usd":"50","can_spend_now_usd":"50","expires_at_pulse":100000,"fee_balance_usd":"0","max_per_payment_usd":"10","payees":["bob"],"refill_usd_per_pulse":"0.05","status":"active"}
agent: The account holds 100 USD.
session 540c85409275bf16: done
```

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
--task …`.

## What this does not do

- A node that waits for a person's approval stops the graph there: its
  session waits like any other (`sion agent continue <session>`), and the
  graph's run stays stopped where it was.
- Nodes run on this machine, as sessions of one profile's agent; work for
  another agent is hired with a Task Charter instead
  ([Hiring another agent](agent-teams.md#hiring-another-agent)).
- The router is a model's judgment: it can be wrong, which is why its
  default is one agent.
