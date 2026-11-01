# Profiles and schedules

A profile is how to run an agent, kept under a name; schedules, channels
and `sion` itself start its sessions from it.

Outputs on this page are from real runs on a local devnet, long
identifiers and some JSON shortened with `…`. The models are scripted, and
the messengers run against stand-ins for their APIs, because the
environment these pages were made in reaches none of them. The runs of
subagents and hiring predate specification v1.13 (run again on v1.13, they
printed the same lines with other identifiers); a profile's own tokens,
Slack and the agent gateway are from a run on v1.15.

## Profiles

A profile keeps how to run an agent under a name: the wallet, the agent,
the models, the autonomy, and optionally its memory, skills, workspace,
the web, delegation, a token budget for each run (`--budget-tokens`) and
quarantine (`--quarantine`).
Schedules and channels start sessions from a profile:

```text
$ sion agent profile add lead lead 7e37dd93…ffab --model script:check.json --autonomy L3
profile lead: agent 7e37dd93 as lead, script:check.json at L3
```

## Schedules

A job is a task a profile works on once (`--at 2026-11-02T09:00:00Z`),
every interval (`--every 15m`), or on a cron expression (`--cron`, five
fields, in UTC). What the agent said goes to a channel: a file (one JSON
line per run), a webhook (a POST of the same JSON), a Telegram chat, or a
chat on another messenger the bridge reaches, with the bridge's
credentials (`--deliver email:alice@example.org`, `slack:C042`,
`nextcloud-talk:n3xtc10ud`; `sion agent schedule add --help` lists them).
Teams and DingTalk answer only where they are written to, and Google Chat
sends a message of its own only with a service account's key:

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

## What this does not do yet

- Jobs run only while the scheduler runs on this machine
  (`agent:local-scheduler`). Schedules are in UTC.
