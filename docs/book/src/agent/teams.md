# Subagents and hired help

An agent can hand work to subagents of its own and hire other agents. None
of this loosens its [charter](../guides/charters.md): a subagent's budget
is carved from its parent's, work hired from another agent is paid from
the agent's own budget, and every payment still goes through the ledger.

Outputs on this page are from real runs on a local devnet, long
identifiers and some JSON shortened with `…`. The models are scripted, and
the messengers run against stand-ins for their APIs, because the
environment these pages were made in reaches none of them. The runs of
subagents and hiring predate specification v1.13 (run again on v1.13, they
printed the same lines with other identifiers); a profile's own tokens,
Slack and the agent gateway are from a run on v1.15.

## Subagents

(To run a profile's agent as the nodes of a small graph instead, a split,
its parts side by side and their reports put together, or work and a
check of it, see [Graphs of agents](graphs.md).)

`--delegate` gives an agent five more tools: `agent_spawn`, `task_post`,
`task_status`, `task_submit` and `task_release`. alice registered `lead` with a budget of
100 USD, as in [A run, within its limits](run.md#an-agent-and-its-limits).
The lead hands a purchase to a subagent with 10 USD of its budget:

```text
$ sion agent run lead 7e37dd93…ffab --task 'Get the dataset from bob and have it summarised.' --model script:lead.json --delegate --subagent-model script:buyer.json --autonomy L6
session 35fe88cb470eb08a for agent 7e37dd93
  → agent_spawn {"budget":"10","name":"buyer","task":"Buy the dataset from bob: pay him 4 USD, then 8 USD for the update."}
  ← subagent lead-buyer (agent 038912b1, budget 10 USD) worked in session 0bf0f92180a4eb97: done; it said: Paid bob 4 USD; the 8 USD update is more than I have left. Its calls: ledger_pay ok, ledger_pay failed (not sent: no part of your budget holds 8 USD).
  → task_post {"amount":"5","description":"Summarise the dataset in three lines.","worker":"contractor"}
  ← Pulse 85: Done: task a41ff37e posted with 5 USD in escrow, for contractor, until Pulse 1082; fee 0.0011 USD. Task a41ff37eafd9d8ed79b195ddd652c5cef40b91e0ec7e0d7c7a75a787fc0b3114; it names no adjudicator, so its results cannot be challenged.
agent: The buyer paid bob for the dataset, and the summary is commissioned.
session 35fe88cb470eb08a: done
```

`agent_spawn` does five things:

1. It registers a **keyless** subagent identity, here `lead-buyer`
   (`SpawnAgents`, specification v1.13). No key is made for it: its
   account is derived from its id, and the wallet that signs for the
   parent signs for it, each of its transactions a [batch](../concepts/batched-authorization.md)
   of one. `lead-buyer` is a `.keyless` file under `$SION_HOME/wallets`
   that holds no secret, only the subagent's id and the name of the wallet
   that signs for it.
2. It carves the budget from the parent's allotment, and the subagent's
   bucket from the parent's bucket: a burst of its budget if the parent's
   bucket holds that much for every subagent of the turn, and the share of
   the parent's refill that its budget is of the parent's remaining
   budget. The ledger requires the subagent's caveats to attenuate the
   parent's: payees, the most per payment (here at most its budget), the
   bucket, and the expiry.
3. It gives the subagent 0.05 USD from the account that signs for it, to
   pay its transactions' fees.
4. It runs the task in a session of the subagent's own, with the models
   given by `--subagent-model` (by default the parent's) and the same
   autonomy.
5. It reports back what the subagent said, and how each of its calls
   went.

The subagent could not spend more than it was given: after paying 4 USD,
it had 6 left, and the 8 USD payment was not sent. Its session is kept
like any other:

```text
$ sion agent sessions
0bf0f92180a4eb97  lead-buyer  done  Buy the dataset from bob: pay him 4 USD, then 8 USD for the
35fe88cb470eb08a  lead  done  Get the dataset from bob and have it summarised.
```

Subagents may spawn their own, two levels deep at most, and a keyless
subagent's own subagents are signed for by the same wallet, the nearest
keyed ancestor's. `agent_spawn` is a spending call, so below autonomy L6
it waits for a person's approval. Suspending the lead's identity on the
ledger (`sion agent suspend`) stops the subagents too.

A session that has read content from outside spawns its subagents
tainted, since the task it writes them may carry what it read, and what a
tainted subagent reports taints its parent. In a run with a token budget
([Safety nets](checks.md#safety-nets)), the subagents of a turn
share what is left of it, the reserve for a last answer kept back, and
what they used counts in it.

The subagents one turn asks for are spawned together, in one
`SpawnAgents` transaction and one transfer of fee money, and work at the
same time; the payments they make meanwhile are signed in batches
([Spawning subagents together](tools.md#spawning-subagents-together)).

## Hiring another agent

The lead also hired `contractor`, another agent, through a
[Task Charter](../concepts/task-charters.md). `task_post` escrowed 5 USD
from the lead's budget, under its caveats, for a result that passes the
rule fixed when the task was posted. The contractor's results rest on its
task bond: it posted `sion bond contractor task-worker 20` beforehand.
The contractor, run in a session of its own, reads the task and delivers:

```text
$ sion agent run contractor e569f97c…9f47 --task 'Do the summary you were hired for: task a41ff37e….' --model script:contractor.json --delegate --autonomy L5
session 50c0ce6683a5ea5a for agent e569f97c
  → task_status {"task":"a41ff37eafd9d8ed79b195ddd652c5cef40b91e0ec7e0d7c7a75a787fc0b3114"}
  ← {"acceptance":{"adjudicator":null,"detection_permille":900,"kappa":2.0,"reliance_pulses":1000,"requires":[],"verifiers":[]},"deadline":1082,"escrow":"5","funded_by_agent":"7e37dd93…",…}
  → task_submit {"output":"Rows: 1,200 sales. Columns: date, region, amount. Trend: up 4% a month.","task":"a41ff37eafd9d8ed79b195ddd652c5cef40b91e0ec7e0d7c7a75a787fc0b3114"}
  ← Pulse 91: Done: task a41ff37e settled: 5 USD paid to contractor for result 5c6d3784, its backing reserved; fee 0.0011 USD. Result 5c6d3784fc191f727b692f4a107132f274ee83122d4df7bd9992de183499e963.
agent: Delivered the summary and was paid.
session 50c0ce6683a5ea5a: done
```

The ledger paid the contractor only because its result passed the rule.
Its 20 USD bond, at the task's detection factor of 0.9, covers κ = 2
times the 5 USD paid. The lead checks the task later, and its output is
included when it is kept on this machine:

```text
$ sion agent run lead 7e37dd93…ffab --task 'Did the summary arrive?' --model script:status.json --delegate --autonomy L3
session d0de066195fbb1a8 for agent 7e37dd93
  → task_status {"task":"a41ff37eafd9d8ed79b195ddd652c5cef40b91e0ec7e0d7c7a75a787fc0b3114"}
  ← {…,"output":"Rows: 1,200 sales. Columns: date, region, amount. Trend: up 4% a month.",…,"state":{"Settled":{…}},…}
agent: The summary arrived: sales are up 4% a month.
session d0de066195fbb1a8: done
```

The output is the contractor's work, and the lead reads it as content
from outside: its session is tainted from then on, so its external calls
and payments wait for a person.

A task posted without an adjudicator cannot be challenged: give the run
`--adjudicator <atom>` to name one for the tasks it posts. `task_post`
takes verifiers too, whose attestations count, as in
[Task Charters from the command line](../guides/tasks.md).

### A check its reviewer runs, and a way out

`task_post` can also fix a check every result must pass: its files and
the command that runs them, committed in the task's spec, kept on this
machine for the verifiers and not handed to the worker
([a check fixed when the task is
posted](../guides/tasks.md#9-a-check-fixed-when-the-task-is-posted)).
In another run, the lead hires `contractor` for `sum.sh`, with a check
that `reviewer-1` must run and attest, and for a second task:

```text
$ sion agent run lead b398797d…d28d --task 'Have sum.sh written, checked by reviewer-1, and the sales column summed.' --model script:lead.json --delegate --autonomy L6
session 56f07991ac9bdf71 for agent b398797d
  → task_post {"amount":"5","check":{"files":{"cases.txt":"5|2 3\n10|1 2 3 4\n0|\n-1|2 -3\n","check.sh":"# The worker's sum.sh is at $OUTPUT; the cases are the check's own.\nwhile IFS='|' read -r expected args; do\n  got=$(sh \"$OUTPUT\" $args 2>&1)\n  [ \"$got\" = \"$expected\" ] || { echo \"sum.sh $args printed '$got', not $expected\"; exit 1; }\ndone < cases.txt\necho \"all $(wc -l < cases.txt | tr -d ' ') cases pass\"\n"}},"description":"Write sum.sh: it prints the sum of the integers it is given as arguments, and 0 for none.","must_attest":["reviewer-1"],"verifiers":["reviewer-1"],"worker":"contractor"}
  ← Pulse 43: Done: task 804286a9 posted with 5 USD in escrow, for contractor, until Pulse 1040; fee 0.0012 USD. Task 804286a9037cf6a86cdccbaf301bfcd908dd73f7d6f44270fc7577e031768470; its check 296edda8 is fixed in its spec and kept here for its verifiers; it names no adjudicator, so its results cannot be challenged.
  → task_post {"amount":"3","description":"Sum the column of sales.csv that nobody can read.","worker":"contractor"}
  ← Pulse 47: Done: task b1fc5ba8 posted with 3 USD in escrow, for contractor, until Pulse 1044; fee 0.0011 USD. Task b1fc5ba8c684ecc6f65bfe8288d0657deb406df5f30585b636c13af1375c13db; it names no adjudicator, so its results cannot be challenged.
agent: sum.sh is commissioned, with a check its reviewer runs, and so is the column's sum.
session 56f07991ac9bdf71: done
```

The contractor sees that a check is fixed (its commitment), not what it
checks. A task paid only with an attestation is not submitted without
one: its output is kept here for the verifiers:

```text
$ sion agent run contractor 02b5bc51…c865 --task 'Write the sum.sh you were hired for: task 804286a9…8470.' --model script:contractor.json --delegate --autonomy L5
session 1dbc085e931d8d7e for agent 02b5bc51
  → task_status {"task":"804286a9037cf6a86cdccbaf301bfcd908dd73f7d6f44270fc7577e031768470"}
  ← {"acceptance":{"adjudicator":null,"detection_permille":900,"kappa":2.0,"min_attestations":0,"must_attest":["a038aefff307362442c223ceb44b2cd0c4e58a01b45dd888dcdda3897ce9e2cd"],"reliance_pulses":1000,"requires":[],"verifiers":["a038aefff307362442c223ceb44b2cd0c4e58a01b45dd888dcdda3897ce9e2cd"]},"check":"296edda8af89d60e81fe791798c81b22d4baec6361a64306ef78e86cfad63f31","deadline":1040,"escrow":"5","f
  → task_submit {"output":"s=0\nfor n in \"$@\"; do s=$((s + n)); done\necho $s\n","task":"804286a9037cf6a86cdccbaf301bfcd908dd73f7d6f44270fc7577e031768470"}
  ← Not submitted yet: task 804286a9 is paid only with the attestations of 1 named verifier(s) of its output, which they give after running the check the task fixed. Your output is kept on this machine as 355d9410276a5cb57fa5ad6ee81468115e38109cf0204072d0b769539b588e88, for its verifiers to check; once they have attested it, call task_submit again with their attestations' ids.
agent: sum.sh is delivered to reviewer-1, who checks it before it is paid.
session 1dbc085e931d8d7e: done
```

reviewer-1 runs the check on the output kept, and attests it:

```text
$ sion task verify reviewer-1 804286a9…8470 355d9410…8e88
the check passed:
exit status 0
--- stdout
all 4 cases pass
reviewer-1 signs: Publish the claim "the output for task 804286a9 passes" (task.attestation/v1), valid for 1992 Pulses, resting on 3 assumptions and 0 other claims. Fee 0.0011 USD; valid until Pulse 68.
Pulse 50: Done: claim f974545d submitted; fee 0.0011 USD.
claim f974545d133c7c28baf7d4164fe1b2d397217020ad9903ae50985a493f78b85f
```

With the attestation, the contractor is paid:

```text
$ sion agent run contractor 02b5bc51…c865 --task 'reviewer-1 attested sum.sh: claim f974545d…b85f. Submit it.' --model script:paid.json --delegate --autonomy L5
session eef5c6214f71b9d0 for agent 02b5bc51
  → task_submit {"attestations":["f974545d133c7c28baf7d4164fe1b2d397217020ad9903ae50985a493f78b85f"],"output":"s=0\nfor n in \"$@\"; do s=$((s + n)); done\necho $s\n","task":"804286a9037cf6a86cdccbaf301bfcd908dd73f7d6f44270fc7577e031768470"}
  ← Pulse 57: Done: task 804286a9 settled: 5 USD paid to contractor for result a709dfe3, its backing reserved; fee 0.0011 USD. Result a709dfe30614cbe7aed2c2be764f9d878045cb5bd8c3dc2830056487bab52e01.
agent: Paid for sum.sh, on reviewer-1's attestation.
session eef5c6214f71b9d0: done
```

The second task cannot be done as asked. `task_release` is the worker's
honest exit, before the deadline: the escrow goes back to whoever paid
for it, and the worker's bond is untouched:

```text
$ sion agent run contractor 02b5bc51…c865 --task 'Sum the column you were hired for: task b1fc5ba8…13db.' --model script:release.json --delegate --autonomy L5
session b0295729f4601766 for agent 02b5bc51
  → task_release {"task":"b1fc5ba8c684ecc6f65bfe8288d0657deb406df5f30585b636c13af1375c13db","why":"sales.csv's sales column is encrypted, and no key came with the task"}
  ← Pulse 60: Done: task b1fc5ba8 cancelled: 3 USD refunded to alice; fee 0.0011 USD. Released because sales.csv's sales column is encrypted, and no key came with the task; your task bond is untouched.
agent: Released the column's sum: the column is encrypted, and no key came with the task.
session b0295729f4601766: done
```

## An agent's record, and finding one for a task

What an agent has done is on the ledger, so nobody has to be taken at
their word. Its record is read from the events themselves: the tasks it
was paid for and what they came to, the tasks it was named for and gave
back before delivering, and delivered results later proven false
(`Falsified`, through [bonds and
challenges](../guides/bonds-and-challenges.md)). Those add up into one
number to trust it by, which deliveries raise and failures lower, counting
a proven-false result several deliveries against it and weighing the whole
by how much it rests on: a dozen clean tasks is most of the way, one task
is not.

Finding the best agent for a task weighs two things: how well what each
one offers fits the task, by the words (BM25, as a [tool
search](models.md) ranks tools), and its record. A cheap newcomer does not
outrank a proven agent on words alone, and a search can be held to what is
needed — only agents in an enclave whose attestation checks out
([Renting](renting.md)), at most a price per thousand tokens.

This reads the ledger and ranks what it is given; it is a reader's own
weighing, not the ledger's, and a different reader may weigh it otherwise.
An agent with no record scores zero for trust, so a new agent is found by
fit alone.

## What this does not do yet

- A subagent's answer and a hired agent's output are checked only by
  the rules the parent set: the charter's limits, and the task's
  acceptance rule and bonds. Nothing reads the work itself.
- An agent's record counts what the ledger shows: tasks, releases and
  falsified results. Work done off the ledger, and rentals, are not in it.
- Finding an agent ranks the candidates it is given. There is no
  enumeration of every agent or listing on the chain to search through.
- A keyless subagent's transactions are signed by the runtime that holds
  its ancestor's wallet, for the subagent's own tool calls only
  (`agent:keyless-signer`). Its payments are gathered with those of the
  subagents working beside it; its other transactions (memory, tasks) are
  each signed as a batch of one.
