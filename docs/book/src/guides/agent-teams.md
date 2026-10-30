# Teams, schedules and channels

An agent can hand work to others, run on a schedule, and be reached over
HTTP or Telegram. None of this loosens its [charter](charters.md). A
subagent's budget is carved from its parent's, work hired from another
agent is paid from the agent's own budget, and every payment still goes
through the ledger.

Every output below is from a real run on a local devnet (long identifiers
and some JSON shortened with `…`). The models are scripted, as in
[Agents at work](agent-runtime.md). The Telegram part runs against a
stand-in for Telegram's Bot API, because this environment cannot reach
Telegram. The runs predate specification v1.13. Run again on v1.13, the
subagents and hiring part printed the same lines with other identifiers,
and `lead-buyer` was a keyless subagent.

## Subagents

`--delegate` gives an agent four more tools: `agent_spawn`, `task_post`,
`task_status` and `task_submit`. alice registered `lead` with a budget of
100 USD, as in [Agents at work](agent-runtime.md#an-agent-and-its-limits).
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

The subagents one turn asks for are spawned together, in one
`SpawnAgents` transaction and one transfer of fee money, and work at the
same time; the payments they make meanwhile are signed in batches
([Spawning subagents together](agent-tools.md#spawning-subagents-together)).

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

A task posted without an adjudicator cannot be challenged: give the run
`--adjudicator <atom>` to name one for the tasks it posts. `task_post`
takes verifiers too, whose attestations count, as in
[Task Charters from the command line](tasks.md).

## Profiles

A profile keeps how to run an agent under a name: the wallet, the agent,
the models, the autonomy, and optionally its memory, skills, workspace,
the web and delegation. Schedules and channels start sessions from a
profile:

```text
$ sion agent profile add lead lead 7e37dd93…ffab --model script:check.json --autonomy L3
profile lead: agent 7e37dd93 as lead, script:check.json at L3
```

## Schedules

A job is a task a profile works on once (`--at 2026-11-02T09:00:00Z`),
every interval (`--every 15m`), or on a cron expression (`--cron`, five
fields, in UTC). What the agent said goes to a channel: a file (one JSON
line per run), a webhook (a POST of the same JSON), or a Telegram chat:

```text
$ sion agent schedule add budget-check lead --task 'How much budget is left?' --every 1m --deliver file:reports.jsonl
budget-check: lead runs "How much budget is left?" every 1m; next 2026-10-28T23:39:32Z; to file:reports.jsonl
$ sion agent schedule add weekday-report lead --task 'How much budget is left?' --cron '0 9 * * mon-fri' --deliver webhook:https://hooks.example.com/sion
weekday-report: lead runs "How much budget is left?" on cron "0 9 * * mon-fri" (UTC); next 2026-10-29T09:00:00Z; to webhook:https://hooks.example.com/sion
```

`sion agent scheduler` runs the jobs as they fall due, each in a session
of its own. With `--once`, it runs only the jobs due now and stops, so
that a system timer (cron, systemd) can call it instead:

```text
$ sion agent scheduler --poll 10
job budget-check: How much budget is left?
    → ledger_balance {}
    ← {"budget_remaining_usd":"85",…}
  agent: 85 USD left in the budget.
  session 2dcf5911c22297ca: done
job budget-check: 85 USD left in the budget. (delivered to file:reports.jsonl)
$ cat reports.jsonl
{"at":"2026-10-28T23:39:32Z","job":"budget-check","profile":"lead","said":"85 USD left in the budget.","session":"2dcf5911c22297ca"}
$ sion agent schedule list
budget-check: lead runs "How much budget is left?" every 1m; next 2026-10-28T23:40:32Z; to file:reports.jsonl
  last run 2026-10-28T23:39:32Z: session 2dcf5911c22297ca: 85 USD left in the budget. (delivered to file:reports.jsonl)
weekday-report: lead runs "How much budget is left?" on cron "0 9 * * mon-fri" (UTC); next 2026-10-29T09:00:00Z; to webhook:https://hooks.example.com/sion
```

A job whose session waits for approval reports that it waits, and the
session is answered like any other (`sion agent continue <session>
--approve`). A run that was missed, because the scheduler was not
running, runs once when the scheduler starts again.

## Channels

Each conversation on a channel is a session, continued message by
message. Four commands work on every channel: `/approve` and
`/decline <why>` answer a call that waits for approval, `/new` starts
over, and `/status` says where the session stands.

### HTTP

`sion agent serve` answers `POST /v1/profiles/<name>/messages` with
`{"text": …, "conversation": …}`, and `GET /v1/sessions/<id>`. Every
request needs the bearer token in `SION_AGENT_TOKEN` (at least 16
characters):

```text
$ SION_AGENT_TOKEN=… sion agent serve --listen 127.0.0.1:8791
agent channel on http://127.0.0.1:8791 (POST /v1/profiles/<name>/messages)
```

```text
$ curl -s -X POST http://127.0.0.1:8791/v1/profiles/lead/messages -d '{"text": "How much budget is left?"}'
{"error":"a bearer token is needed"} (HTTP 401)
$ curl -s -X POST http://127.0.0.1:8791/v1/profiles/lead/messages -H "Authorization: Bearer $SION_AGENT_TOKEN" -d '{"text": "How much budget is left?", "conversation": "ops"}'
{"conversation":"ops","lines":["  → ledger_balance {}","  ← {\"budget_remaining_usd\":\"85\",…}","agent: 85 USD left in the budget.","session 37702955dda47c36: done"],"said":"85 USD left in the budget.","session":"37702955dda47c36"}
$ curl -s -X POST http://127.0.0.1:8791/v1/profiles/lead/messages -H "Authorization: Bearer $SION_AGENT_TOKEN" -d '{"text": "/status", "conversation": "ops"}'
{"conversation":"ops","lines":[],"said":"85 USD left in the budget.","session":"37702955dda47c36"}
```

Requests are handled one at a time.

### Telegram

`sion agent telegram` answers a profile's chats by long polling, with the
bot token in `TELEGRAM_BOT_TOKEN`. It answers only the chats in `--allow`.
Here `TELEGRAM_API_BASE` points it at the stand-in, which returned two
messages, from chat 42 and from chat 99:

```text
$ TELEGRAM_BOT_TOKEN=… TELEGRAM_API_BASE=http://127.0.0.1:8792 sion agent telegram lead --allow 42 --once
chat 42: How much budget is left?
    → ledger_balance {}
    ← {"budget_remaining_usd":"85",…}
  agent: 85 USD left in the budget.
  session 9fdce72752d0f32a: done
to chat 42: 85 USD left in the budget.
chat 99: not allowed, ignored
```

The stand-in received one message, for chat 42:

```text
sendMessage {"chat_id": 42, "text": "85 USD left in the budget."}
```

## What this does not do yet

- The HTTP channel's token is one secret for every profile on the
  machine. Whoever holds it can start and continue sessions, approvals
  included, within each profile's autonomy and the agent's charter. The
  channel speaks plain HTTP: keep it on a local address, or put it behind
  TLS (`agent:channel-token`).
- A Telegram bot trusts Telegram for who writes in a chat it is allowed
  to answer. In a group chat, every member speaks for you, approvals
  included (`agent:telegram-chats`). Only text messages are read.
- Jobs run only while the scheduler runs on this machine
  (`agent:local-scheduler`). Schedules are in UTC.
- A subagent's answer and a hired agent's output are checked only by
  the rules the parent set: the charter's limits, and the task's
  acceptance rule and bonds. Nothing reads the work itself.
- A keyless subagent's transactions are signed by the runtime that holds
  its ancestor's wallet, for the subagent's own tool calls only
  (`agent:keyless-signer`). Its payments are gathered with those of the
  subagents working beside it; its other transactions (memory, tasks) are
  each signed as a batch of one.
- There is no gateway that routes channels across machines, and no
  messenger other than Telegram.
