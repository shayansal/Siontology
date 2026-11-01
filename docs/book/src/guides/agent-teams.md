# Teams, schedules and channels

An agent can hand work to others, run on a schedule, and be reached over
HTTP, Telegram, Slack, Discord, Matrix or Signal, on its own machine or
through an agent gateway. None of this loosens its [charter](charters.md). A
subagent's budget is carved from its parent's, work hired from another
agent is paid from the agent's own budget, and every payment still goes
through the ledger.

Every output below is from a real run on a local devnet (long identifiers
and some JSON shortened with `…`). The models are scripted, as in
[Agents at work](agent-runtime.md). The Telegram and Slack parts run
against stand-ins for their APIs, because this environment cannot reach
either. The runs predate specification v1.13. Run again on v1.13, the
subagents and hiring part printed the same lines with other identifiers,
and `lead-buyer` was a keyless subagent. A profile's own tokens, Slack
and the agent gateway are from a later run, on v1.15.

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

A session that has read content from outside spawns its subagents
tainted, since the task it writes them may carry what it read, and what a
tainted subagent reports taints its parent. In a run with a token budget
([Safety nets](agent-runtime.md#safety-nets)), the subagents of a turn
share what is left of it, the reserve for a last answer kept back, and
what they used counts in it.

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

The output is the contractor's work, and the lead reads it as content
from outside: its session is tainted from then on, so its external calls
and payments wait for a person.

A task posted without an adjudicator cannot be challenged: give the run
`--adjudicator <atom>` to name one for the tasks it posts. `task_post`
takes verifiers too, whose attestations count, as in
[Task Charters from the command line](tasks.md).

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

## Channels

Each conversation on a channel is a session, continued message by
message. On a messenger that keeps threads apart (a Slack or Mattermost
thread, a Telegram topic), each thread is a conversation of its own.
These commands work on every channel:

| Command | What it does |
|---|---|
| `/stop` | stops what the agent is doing, at once: a model that is writing is cut off there, what it wrote kept |
| `/approve`, `/decline <why>` | decide the call the session waits on; given its request's id first (`/approve 3f2a9c1e`), only if it is still the one waiting |
| `/approve always`, `/approve always <rule>` | approve it, and for the rest of the session the same call again, or the calls of its tool the rule matches (`write_file(notes:*)`, `shell(git status:*)`) |
| `/allowed`, `/allowed off` | what the session may do without asking beyond its autonomy level, and withdrawing it |
| `/new` | starts over, in a new session |
| `/status` | where the session stands |
| `/retry` | answers the last message again |
| `/undo` | forgets the last message and its answer (what its calls did stays done) |
| `/compact` | shortens the session, its start kept as a summary |
| `/usage` | the turns, tokens and messages the session used, its subagents' tokens, and a run's budget if it has one |
| `/sessions`, `/resume <id>` | this conversation's sessions, and going back to one of them |
| `/title <name>` | names the session |
| `/model`, `/model <number>` | the profile's models, and which one the session uses |
| `/system`, `/system <text>`, `/system off` | what the chat asks of the agent, after its operator's instructions |
| `/tools`, `/tools <name> …`, `/tools all` | the only tools the agent may use in the chat |
| `/whoami` | on a bridge: who you are there, and what you may do |
| `/help` | these commands |

`/approve`, `/decline`, `/model`, `/system` and `/tools` are for those
who may decide: on a bridge, those `--approvers` names (anyone in a chat
it answers, if it names no one); on the HTTP channel and through a
gateway, a token made for approvals. The session keeps each decision with
who made it, as the channel names them (`slack:U07`, `http:token
3046d92e`, `cli:<user>` on the command line), and the run shows it:
`(approved by slack:U07)`. What `/approve always` allows is never a
payment, which is approved on its own, nor a call whose argument comes
from content held in quarantine; it is not given while the session
has read content from outside, nor holds then; and it never goes past
what the autonomy level refuses, as for [a skill's
pre-approvals](skill-permissions.md).

While the agent works in a conversation, `/stop`, `/status`, `/help`,
`/usage` and `/sessions` are answered at once; any other command waits
for the agent on a bridge, and is refused on the HTTP channel and through
a gateway (which answer many requests at once, so a `/stop` there reaches
a run another request started).

What a chat chooses holds for its sessions, the one it is in and those it
starts later, and is kept with the channel's state: `/model <number>`
the model they start with (one of the profile's), `/system <text>` what
they are asked after the operator's instructions (`/system off` takes it
back), `/tools <name> …` the only tools they may use of those the
profile gives (`/tools all` gives them all again).

A bridge keeps each message it has taken until it has answered it, so a
message taken when the bridge stopped is answered when it starts again.
A chat whose answer the stop cut off is told so first ("I was restarted
while I worked on what you sent, so I am taking it up again."), and is
answered again: it may have seen part of the first answer.

### HTTP

`sion agent serve` answers `POST /v1/profiles/<name>/messages` with
`{"text": …, "conversation": …}`, and `GET /v1/sessions/<id>`. Every
request needs a bearer token: the machine's, in `SION_AGENT_TOKEN` (at
least 16 characters), which reaches every profile and session, or one of
a profile's own (below):

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

With `"stream": true`, the answer comes as Server-Sent Events while the
agent works: `session` with the session's id as its run begins, `delta`
with the text it wrote since the last (every answer of the run a
paragraph), `tool` with a tool it calls, runs or waits on (`"state"`:
`calling`, `running`, `waiting`), and `done` with what the whole answer
would have been (or `error`).

Requests are handled one at a time.

### A profile's own tokens

`sion agent profile token add <profile>` makes a token that reaches only
that profile, and names the profile's agent. It is shown once, and the
machine keeps only its hash. Its holder may talk to the profile; with
`--approve`, they may also approve or decline what the profile's
sessions wait on. `sion agent serve` takes these tokens with or without
the machine's token:

```text
$ sion agent profile token add lead
token 5957d62c for profile lead, agent 94629049 (messages only); it is shown once, and kept here only as its hash:
sion_ch_c7307346…
$ sion agent profile token add payer
token 2b871571 for profile payer, agent 23e91a81 (messages only); it is shown once, and kept here only as its hash:
sion_ch_3a16ec5f…
$ sion agent profile token add payer --approve
token 3046d92e for profile payer, agent 23e91a81 (messages, approvals and declines); it is shown once, and kept here only as its hash:
sion_ch_ea2edb78…
$ sion agent profile token list
5957d62c  lead  agent 94629049  messages
2b871571  payer  agent 23e91a81  messages
3046d92e  payer  agent 23e91a81  messages, approvals
$ sion agent serve --listen 127.0.0.1:8794
agent channel on http://127.0.0.1:8794 (POST /v1/profiles/<name>/messages; OpenAI's /v1/chat/completions and /v1/models)
```

Below, `$LEAD_TOKEN` is lead's token, `$TALK` payer's first and `$DECIDE`
its second. payer runs at L5, where a payment waits for a person:

```text
$ curl -s -X POST http://127.0.0.1:8794/v1/profiles/payer/messages -H "Authorization: Bearer $LEAD_TOKEN" -d '{"text": "Pay bob 1 USD for invoice 7.", "conversation": "ops"}'
{"error":"this token does not reach this profile"} (HTTP 401)
$ curl -s -X POST http://127.0.0.1:8794/v1/profiles/payer/messages -H "Authorization: Bearer $TALK" -d '{"text": "Pay bob 1 USD for invoice 7.", "conversation": "ops"}'
{"conversation":"ops","files":[],"lines":["  → ledger_pay {\"amount\":\"1\",\"purpose\":\"invoice 7\",\"to\":\"bob\"}","  ← waiting for a person's approval","session 3ffd3b1fd4d6aa36: waiting for you: ledger_pay {\"amount\":\"1\",\"purpose\":\"invoice 7\",\"to\":\"bob\"} (spending at autonomy L5; request e415977a). Answer with `sion agent continue 3ffd3b1fd4d6aa36 --approve` or `--decline <why>`."],"said":"waiting for your approval: ledger_pay {\"amount\":\"1\",\"purpose\":\"invoice 7\",\"to\":\"bob\"} (spending at autonomy L5; request e415977a)","session":"3ffd3b1fd4d6aa36"} (HTTP 200)
$ curl -s -X POST http://127.0.0.1:8794/v1/profiles/payer/messages -H "Authorization: Bearer $TALK" -d '{"text": "/approve e415977a", "conversation": "ops"}'
{"error":"this token may talk to the profile, not approve or decline"} (HTTP 403)
$ curl -s -X POST http://127.0.0.1:8794/v1/profiles/payer/messages -H "Authorization: Bearer $DECIDE" -d '{"text": "/approve e415977a", "conversation": "ops"}'
{"conversation":"ops","files":[],"lines":["  → ledger_pay {\"amount\":\"1\",\"purpose\":\"invoice 7\",\"to\":\"bob\"} (approved by http:token 3046d92e)","  ← Pulse 14: Done: agent paid 1 USD to bob (receipt claim cee25b76); fee 0.0012 USD. Transaction fc22f4f4…3633.","agent: Paid bob 1 USD for invoice 7.","session 3ffd3b1fd4d6aa36: done"],"said":"Paid bob 1 USD for invoice 7.","session":"3ffd3b1fd4d6aa36"} (HTTP 200)
```

lead's token does not reach payer. `$TALK` starts the payment, which
waits, but cannot approve it; `$DECIDE` can, naming the request it was
shown, and the session keeps the approval as that token's.
`sion agent profile token remove <profile> <id>` takes a token back.

### OpenAI's chat API

The same server answers OpenAI's chat API, so apps made for it (Open
WebUI, LibreChat, editors, scripts) can talk to a profile: point the app
at `http://<listen>/v1` with a profile's token as its key, and choose the
profile as the model. `GET /v1/models` lists the profiles the token
reaches. Such apps send the whole conversation with every message; the
server finds the session by what was said before the last message, so a
conversation goes on in its session, approvals included (`/approve` and
`/decline <why>`, with a token made to decide). A conversation it has not
seen (edited in the app, or begun elsewhere) starts a new session, told
what was said before. With `"stream": true`, the answer comes as
Server-Sent Events while the agent works: each chunk carries what it wrote
since the last, every answer of the run a paragraph, and the chunks' `id`
names the session, as a whole answer's does. A scripted model, as here,
writes its answer at once, so it comes in one chunk.

```text
$ sion agent profile token add assistant
token 843839a6 for profile assistant, agent 908f6ddc (messages only); it is shown once, and kept here only as its hash:
sion_ch_0c6b4e7a…
$ sion agent serve --listen 127.0.0.1:8794
agent channel on http://127.0.0.1:8794 (POST /v1/profiles/<name>/messages; OpenAI's /v1/chat/completions and /v1/models)
$ curl -s http://127.0.0.1:8794/v1/models -H "Authorization: Bearer $TOKEN"
{"data":[{"created":0,"id":"assistant","object":"model","owned_by":"sion"}],"object":"list"} (HTTP 200)
$ curl -s http://127.0.0.1:8794/v1/chat/completions -H "Authorization: Bearer $TOKEN" -d '{"model": "assistant", "messages": [{"role": "user", "content": "How much budget is left?"}]}'
{"choices":[{"finish_reason":"stop","index":0,"message":{"content":"100 USD left in the budget.","role":"assistant"}}],"created":1793394351,"id":"chatcmpl-b3362ad0e5d01a89","model":"assistant","object":"chat.completion"} (HTTP 200)
$ curl -s http://127.0.0.1:8794/v1/chat/completions -H "Authorization: Bearer $TOKEN" -d '{"model": "assistant", "messages": [{"role": "user", "content": "How much budget is left?"}, {"role": "assistant", "content": "100 USD left in the budget."}, {"role": "user", "content": "Is anything waiting for me?"}]}'
{"choices":[{"finish_reason":"stop","index":0,"message":{"content":"Nothing is waiting for you.","role":"assistant"}}],"created":1793394351,"id":"chatcmpl-b3362ad0e5d01a89","model":"assistant","object":"chat.completion"} (HTTP 200)
$ curl -s http://127.0.0.1:8794/v1/chat/completions -H "Authorization: Bearer $TOKEN" -d '{"model": "assistant", "stream": true, "messages": [{"role": "user", "content": "How much budget is left?"}]}'
data: {"choices":[{"delta":{"content":"","role":"assistant"},"finish_reason":null,"index":0}],"created":1793394352,"id":"chatcmpl-31547ec245a7868a","model":"assistant","object":"chat.completion.chunk"}

data: {"choices":[{"delta":{"content":"100 USD left in the budget."},"finish_reason":null,"index":0}],"created":1793394352,"id":"chatcmpl-31547ec245a7868a","model":"assistant","object":"chat.completion.chunk"}

data: {"choices":[{"delta":{},"finish_reason":"stop","index":0}],"created":1793394352,"id":"chatcmpl-31547ec245a7868a","model":"assistant","object":"chat.completion.chunk"}

data: [DONE]

 (HTTP 200)
```

The second answer is the same session's (`chatcmpl-b3362ad0e5d01a89`); the
streamed one, a conversation the server had not seen, began another. The
answer's `id` names the session (`sion agent trace`).

### Agents that talk A2A

`sion agent serve --a2a <profile>` gives other agents an Agent2Agent
card for the profile, at `/.well-known/agent-card.json` and
`/v1/profiles/<profile>/agent-card.json`, and answers A2A's JSON-RPC at
`POST /v1/profiles/<profile>/a2a`, with one of the channel's tokens
(`--public-url` is the address the card gives, if not the one the
request came to). Both wire versions are spoken, as the request names
them: 0.3 (`message/send`, `message/stream`, `tasks/get`, `tasks/cancel`)
and 1.0 (`SendMessage`, `SendStreamingMessage`, `GetTask`, `CancelTask`).
A message's `contextId` is a conversation, each a session as on every
channel; each message is a task, `completed` when the agent is done, or
`input-required` while the session waits for an approval, which a token
made to decide gives in the same context (`/approve`). Another agent's
message is content from outside: the session is tainted, so its external
calls and payments wait for a person, as after reading a web page. A streamed task
sends its reply as an artifact that grows, and ends with its state,
marked final. The card names no instructions of the operator's: only
what the agent is. Push notifications are not offered.

### Telegram

`sion agent telegram` answers a profile's chats by long polling, with the
bot token in `TELEGRAM_BOT_TOKEN`. It answers only the chats in `--allow`,
and with `--approvers`, only the Telegram users named may approve or
decline. Here `TELEGRAM_API_BASE` points it at the stand-in, which
returned two messages, from chat 42 and from chat 99:

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

### Slack, Discord, Matrix and Signal

`sion agent bridge <profile> --via <messenger>` answers a messenger's
chats with a profile, through the messenger's own API, with credentials
from the environment:

| `--via` | Receives by | Credentials | `--allow` names |
|---|---|---|---|
| `slack` | Socket Mode, given an app-level token; otherwise polling `conversations.history` | `SLACK_BOT_TOKEN`, and `SLACK_APP_TOKEN` for Socket Mode | channel ids |
| `discord` | its gateway | `DISCORD_BOT_TOKEN` | channel ids |
| `matrix` | long polling `/sync` | `MATRIX_HOMESERVER`, `MATRIX_ACCESS_TOKEN` | room ids |
| `signal` | polling the `receive` endpoint of [signal-cli-rest-api](https://github.com/bbernhard/signal-cli-rest-api) | `SIGNAL_API_BASE`, `SIGNAL_NUMBER` | phone numbers |
| `telegram` | long polling `getUpdates` | `TELEGRAM_BOT_TOKEN` | chat ids |
| `mattermost` | its WebSocket | `MATTERMOST_URL`, `MATTERMOST_TOKEN` | channel ids |
| `irc` | a connection of its own (`ircs://`, or `irc://`), with a SASL login if `IRC_PASSWORD` is set | `IRC_SERVER`, `IRC_NICK`, `IRC_PASSWORD` | `#channels` (joined), and nicks |
| `twitch` | Twitch's chat, over IRC | `TWITCH_NICK`, `TWITCH_TOKEN` | `#channels` (joined) |
| `whatsapp`, `line`, `sms` | signed webhooks to `--listen` ([below](#whatsapp-line-and-sms)) | | numbers, user and group ids |

`sion agent telegram` is the same bridge. It does not answer bots:
Telegram does not show a bot other bots' messages, Slack and Discord mark
them, and on Matrix the bridge skips its own. On Signal it answers direct
messages only. It keeps its place, so a restarted bridge goes on where
it stopped, and the first time it polls Slack or Matrix, it starts from
the latest message rather than answering the history; Discord's gateway
and Slack's Socket Mode pass on only what is said while the bridge
listens. With `--approvers`, only the people named, as the messenger
names them, may approve or decline. With `--require-mention`, the group
chats it answers are answered only where a message mentions the bot or
answers one of its messages; direct messages are always answered. A
chat it does not answer is said to be ignored once. On IRC and Twitch, a
reply goes out as lines of about 400 bytes, eight at most, no faster
than the networks allow (Twitch's, one every 1.6 seconds); someone who
writes `lead: …` or names the bot mentions it.

Here `SLACK_API_BASE` points the bridge at a stand-in for Slack's Web API.
Its first poll found one old message, which the bridge took as where to
start; the second found two new messages and a bot's:

```text
$ SLACK_BOT_TOKEN=… SLACK_API_BASE=http://127.0.0.1:8796 sion agent bridge lead --via slack --allow C042 --approvers U07 --once
$ SLACK_BOT_TOKEN=… SLACK_API_BASE=http://127.0.0.1:8796 sion agent bridge lead --via slack --allow C042 --approvers U07 --once
channel C042 (U07): How much budget is left?
    → ledger_balance {}
    ← {"budget_remaining_usd":"100",…}
  agent: 100 USD left in the budget.
  session 3c0aa9d0beb2afb0: done
to channel C042: 100 USD left in the budget.
channel C042 (U19): /approve
to channel C042: Only U07 may approve or decline here.
```

The stand-in received:

```text
chat.postMessage {"channel": "C042", "text": "100 USD left in the budget."}
chat.postMessage {"channel": "C042", "text": "Only U07 may approve or decline here."}
```

### Discord's gateway and Slack's Socket Mode

Discord passes a bot what is said over its gateway, a WebSocket the
bridge keeps open: it sends heartbeats, resumes after a drop, and
connects again after a pause that doubles up to a minute. The bot needs
Discord's message-content intent, turned on in its developer portal; if
Discord refuses the token or the intent, the bridge stops and says why.
Slack does the same through Socket Mode once the app has an app-level
token (`SLACK_APP_TOKEN`, `xapp-…`, with `connections:write`): the bridge
acknowledges each event as it comes, and a message Slack sends twice, as
a message and as a mention, is answered once. Both pass on direct
messages as well as channels', so both pair. When the bridge stops, it
closes the connection, and Discord shows the bot offline.

Here `DISCORD_API_BASE` and `SLACK_API_BASE` point the bridge at
stand-ins, whose gateway and Socket Mode are WebSockets on this machine.
Discord's sent four messages: one in channel 900 that mentions the bot,
another there that does not, one in channel 901, and a direct message
(channel 1300) from someone the bridge does not know. Slack's sent one
message in C042, as a message and as a mention, and a direct message
(D088):

```text
$ DISCORD_BOT_TOKEN=… DISCORD_API_BASE=http://127.0.0.1:8796/discord sion agent bridge lead --via discord --allow 900 --pairing --require-mention --once
channel 900 (2201): How much budget is left?
    → ledger_balance {}
    ← {"budget_remaining_usd":"100",…}
  agent: 100 USD left in the budget.
  session 6a0b4d998c8b3a40: done
to channel 900: 100 USD left in the budget.
channel 901: not allowed, ignored
channel 1300: not known yet; sent a pairing code (sion agent pairing list)
$ SLACK_BOT_TOKEN=… SLACK_APP_TOKEN=… SLACK_API_BASE=http://127.0.0.1:8796/slack sion agent bridge lead --via slack --allow C042 --pairing --once
channel C042 (U07): How much budget is left?
    → ledger_balance {}
    ← {"budget_remaining_usd":"100",…}
  agent: 100 USD left in the budget.
  session 573b1fd79d14ee81: done
to channel C042: 100 USD left in the budget.
channel D088: not known yet; sent a pairing code (sion agent pairing list)
```

The stand-ins received:

```text
gateway: identify op=2 intents=37377 token=…
POST /channels/900/messages {"content":"100 USD left in the budget."}
POST /channels/1300/messages {"content":"I do not know you yet. To talk with me, ask the person who runs me to let you in with: sion agent pairing approve VDWH-KNS9"}
gateway: closed by the bridge
socket mode: {"envelope_id": "e1"}
socket mode: {"envelope_id": "e2"}
socket mode: {"envelope_id": "e3"}
chat.postMessage {"channel":"C042","text":"100 USD left in the budget."}
chat.postMessage {"channel":"D088","text":"I do not know you yet. To talk with me, ask the person who runs me to let you in with: sion agent pairing approve QNB5-KNPN"}
socket mode: closed by the bridge
```

The intents, 37377, are servers, their messages, direct messages and
message content.

### IRC and Twitch

`--via irc` keeps a connection of its own to `IRC_SERVER` (`ircs://` for
TLS), logs in with SASL when `IRC_PASSWORD` is set, joins the `#channels`
in `--allow`, and answers the server's pings; a nick in `--allow` is a
private conversation. `--via twitch` is the same on Twitch's chat, with
the bot's OAuth token. Here `IRC_SERVER` points the bridge at a stand-in,
where, once the bot had joined, ann wrote to it and bob to the channel:

```text
$ IRC_SERVER=irc://127.0.0.1:8793 IRC_NICK=lead sion agent bridge lead --via irc --allow "#ops" --require-mention --once
channel #ops (ann): How much budget is left?
    → ledger_balance {}
    ← {"budget_remaining_usd":"100",…}
  agent: 100 USD left in the budget.
  session ccc37f29472e2dd6: done
to channel #ops: 100 USD left in the budget.
```

The server received:

```text
NICK lead
USER lead 0 * :sion
JOIN #ops
PONG :irc.example.org
PRIVMSG #ops :100 USD left in the budget.
QUIT :stopped
```

`lead: How much budget is left?` mentions the bot; `lunch at noon?` does
not, and with `--require-mention` it is not answered.

### WhatsApp, LINE and SMS

WhatsApp (Meta's Cloud API), LINE (its Messaging API) and SMS (through
Twilio) send what is said to the bridge as webhooks. The bridge listens
on an address of its own (`--listen`, `127.0.0.1:8797` by default), which
you expose to the service behind TLS, through a reverse proxy or a
tunnel. Each webhook is checked against the service's signature with the
secret you give, and one that is unsigned or signed with another secret
is refused before anything reaches the profile. A service sends again
what it thinks was not received; each message is answered once.

| `--via` | Credentials | The signature checked | Answers through |
|---|---|---|---|
| `whatsapp` | `WHATSAPP_TOKEN`, `WHATSAPP_PHONE_NUMBER_ID`, `WHATSAPP_APP_SECRET`, `WHATSAPP_VERIFY_TOKEN` | `X-Hub-Signature-256`, with the app secret; Meta's check of the address is answered given the verify token | the Cloud API's `/messages` |
| `line` | `LINE_CHANNEL_ACCESS_TOKEN`, `LINE_CHANNEL_SECRET` | `X-Line-Signature`, with the channel secret | a reply, while the message's reply token lasts (50 seconds here), and a push after |
| `sms` | `TWILIO_ACCOUNT_SID`, `TWILIO_AUTH_TOKEN`, `TWILIO_NUMBER`, `TWILIO_WEBHOOK_URL` | `X-Twilio-Signature`, with the auth token, over the public address Twilio is given (`TWILIO_WEBHOOK_URL`, exactly as Twilio has it) | Twilio's `Messages` API |

Voice notes are heard on all three (WhatsApp's audio messages, LINE's,
and audio sent by MMS) and spoken as an audio message on WhatsApp; LINE
and MMS play audio only from a public address of your own, so there the
reply is text.

Here each bridge ran with `--once` while a stand-in sent it what the
service would, and another stood in for the service's API
(`WHATSAPP_API_BASE`, `LINE_API_BASE` and `TWILIO_API_BASE` pointed at
it):

```text
$ WHATSAPP_TOKEN=… WHATSAPP_PHONE_NUMBER_ID=106 WHATSAPP_APP_SECRET=… WHATSAPP_VERIFY_TOKEN=… WHATSAPP_API_BASE=http://127.0.0.1:8798/meta sion agent bridge lead --via whatsapp --allow 15550001111 --listen 127.0.0.1:8797 --once
whatsapp: webhooks on http://127.0.0.1:8797/
number 15550001111: How much budget is left?
    → ledger_balance {}
    ← {"budget_remaining_usd":"100",…}
  agent: 100 USD left in the budget.
  session 010e73028a82599f: done
to number 15550001111: 100 USD left in the budget.
```

What the stand-in for WhatsApp sent the bridge, and the answers:

```text
GET ?hub.mode=subscribe&hub.verify_token=…&hub.challenge=4242: HTTP 200 4242
POST (unsigned): HTTP 401 not signed with the app secret
POST (X-Hub-Signature-256: sha256=…): HTTP 200 ok
```

LINE and SMS, the same way:

```text
$ LINE_CHANNEL_ACCESS_TOKEN=… LINE_CHANNEL_SECRET=… LINE_API_BASE=http://127.0.0.1:8798/line sion agent bridge lead --via line --allow U4af4980629 --listen 127.0.0.1:8796 --once
line: webhooks on http://127.0.0.1:8796/
chat U4af4980629: How much budget is left?
    → ledger_balance {}
    ← {"budget_remaining_usd":"100",…}
  agent: 100 USD left in the budget.
  session 810e6fb01af77fad: done
to chat U4af4980629: 100 USD left in the budget.
$ TWILIO_ACCOUNT_SID=AC0001 TWILIO_AUTH_TOKEN=… TWILIO_NUMBER=+15559990000 TWILIO_WEBHOOK_URL=https://bot.example.org/sms TWILIO_API_BASE=http://127.0.0.1:8798/twilio sion agent bridge lead --via sms --allow +15550001111 --listen 127.0.0.1:8795 --once
sms: webhooks on http://127.0.0.1:8795/
number +15550001111: How much budget is left?
    → ledger_balance {}
    ← {"budget_remaining_usd":"100",…}
  agent: 100 USD left in the budget.
  session 01959b3c06d8963d: done
to number +15550001111: 100 USD left in the budget.
```

```text
POST (X-Line-Signature: another secret's): HTTP 401 not signed with the channel secret
POST (X-Line-Signature: …): HTTP 200 ok
POST (unsigned): HTTP 401 not signed with the auth token
POST (X-Twilio-Signature: …): HTTP 200 <?xml version="1.0" encoding="UTF-8"?><Response></Response>
```

The stand-in for the three APIs received:

```text
POST /meta/106/messages {"messaging_product":"whatsapp","text":{"body":"100 USD left in the budget."},"to":"15550001111","type":"text"}
POST /line/v2/bot/message/reply {"messages":[{"text":"100 USD left in the budget.","type":"text"}],"replyToken":"r-0042"}
POST /twilio/2010-04-01/Accounts/AC0001/Messages.json To=%2B15550001111&From=%2B15559990000&Body=100%20USD%20left%20in%20the%20budget.
```

### Microsoft Teams and Google Chat

Teams and Google Chat send what is said to the bridge as webhooks too,
to its `--listen` address, which you expose to them behind TLS. Neither
signs a webhook with a secret you share: each sends a token (a JWT,
signed with RS256), which the bridge checks against the keys the service
publishes, fetched over TLS and kept a day, before anything reaches the
profile.

| `--via` | Credentials | The token taken | Answers through |
|---|---|---|---|
| `teams` | `TEAMS_APP_ID`, `TEAMS_APP_PASSWORD`, and `TEAMS_TENANT_ID` for a single-tenant bot | the Bot Connector's (`https://api.botframework.com`), for the app ID, by a key endorsed for the channel, naming the activity's service URL | the Connector API at that service URL, with a token Microsoft Entra ID gives the app, as a reply to the message |
| `google-chat` | `GOOGLE_CHAT_AUDIENCE` (the project number, or the endpoint's URL), and `GOOGLE_CHAT_CREDENTIALS` (a service account's key file) to answer at any time | Chat's own (`chat@system.gserviceaccount.com`) for the project number, or Google's ID token of `GOOGLE_CHAT_SENDER` for the URL | the webhook's own answer, while Chat waits (thirty seconds); with a service account's key, the Chat API, in the message's thread |

In a Teams channel the bot is written to by mentioning it, and the
mention is cut from what the agent reads; a personal chat is a direct
one, for pairing. Google Chat's events come in either of the shapes Chat
sends: an interaction event, or a Workspace add-on's. Without a service
account, an answer the agent takes longer over than Chat waits is not
delivered, and the bridge says so; with one, it goes whenever it comes,
and voice notes are fetched to be heard.

Here each bridge ran with `--once` while a stand-in sent it what the
service would, its tokens signed with a key the stand-in for the service
published (`TEAMS_OPENID_URL`, `TEAMS_LOGIN_URL` and
`GOOGLE_CHAT_KEYS_URL` pointed the bridge at it):

```text
$ TEAMS_APP_ID=app-1 TEAMS_APP_PASSWORD=… TEAMS_TENANT_ID=tenant-1 TEAMS_OPENID_URL=http://127.0.0.1:8790/openid TEAMS_LOGIN_URL=http://127.0.0.1:8790 sion agent bridge lead --via teams --allow a:1chat --listen 127.0.0.1:8789 --once
teams: webhooks on http://127.0.0.1:8789/
conversation a:1chat (aad-alice): How much budget is left?
    → ledger_balance {}
    ← {"budget_remaining_usd":"100",…}
  agent: 100 USD left in the budget.
  session c92b696768ee4fb1: done
to conversation a:1chat: 100 USD left in the budget.
$ GOOGLE_CHAT_AUDIENCE=1234567890 GOOGLE_CHAT_KEYS_URL=http://127.0.0.1:8790/jwk/chat sion agent bridge lead --via google-chat --allow spaces/DM1 --listen 127.0.0.1:8788 --once
google-chat: webhooks on http://127.0.0.1:8788/
space spaces/DM1 (users/111): How much budget is left?
    → ledger_balance {}
    ← {"budget_remaining_usd":"100",…}
  agent: 100 USD left in the budget.
  session 4d0f4925547a856c: done
to space spaces/DM1: 100 USD left in the budget.
```

What the stand-ins for Teams and Chat sent the bridge, and the answers:

```text
POST (no token): HTTP 401 no token
POST (a token for another bot): HTTP 401 a token for another audience ("app-2")
POST (the Bot Connector's token): HTTP 200 ok
POST (a token for another project): HTTP 401 a token for another audience ("999")
POST (Chat's token): HTTP 200 {"text":"100 USD left in the budget."}
```

What the stand-ins for Microsoft and Google were asked, the secrets
named and not shown:

```text
GET /openid
GET /keys
POST /tenant-1/oauth2/v2.0/token (client_id, client_secret, grant_type, scope)
POST /amer/v3/conversations/a%3A1chat/activities/act-1 (Authorization: Bearer …) {"replyToId":"act-1","text":"100 USD left in the budget.","textFormat":"markdown","type":"message"}
GET /jwk/chat
```

### DingTalk, Feishu and Nextcloud Talk

| `--via` | Credentials | Receives by | Answers through |
|---|---|---|---|
| `dingtalk` | `DINGTALK_CLIENT_ID`, `DINGTALK_CLIENT_SECRET` (an internal app's robot) | Stream mode: a WebSocket the bridge opens to DingTalk, every frame acknowledged, so there is no address to expose | the message's session webhook, or after it expires the robot's API with the app's token |
| `feishu`, `lark` | `FEISHU_APP_ID`, `FEISHU_APP_SECRET`, `FEISHU_VERIFICATION_TOKEN`; `FEISHU_ENCRYPT_KEY` if the subscription has an Encrypt Key | webhooks to `--listen` carrying the subscription's verification token, and with an Encrypt Key encrypted and signed | a reply to the message through the Open Platform's API, with the tenant token |
| `nextcloud-talk` | `NEXTCLOUD_URL`, `NEXTCLOUD_TALK_SECRET` (a bot installed with `occ talk:bot:install`) | webhooks to `--listen`, signed with the bot's secret by the server named | Talk's bot API, signed the same way, as a reply to the message |

DingTalk transcribes voice messages itself, and the agent is given what
it heard, marked as such. Feishu's voice messages are fetched to be heard
(`--hear`), and its pictures and files to be shown or kept. With an
Encrypt Key, Feishu encrypts each event (AES-256) and signs it: an event
not signed with the key is refused. Talk's webhooks do not say whether a conversation is one person's,
so there is no pairing on Talk; name the conversations (`--allow`, by
their tokens).

Here each bridge ran with `--once` against stand-ins for the three
(`DINGTALK_API_BASE`, `FEISHU_API_BASE` and `NEXTCLOUD_URL` pointed at
them):

```text
$ DINGTALK_CLIENT_ID=ding-app DINGTALK_CLIENT_SECRET=… DINGTALK_API_BASE=http://127.0.0.1:8786 sion agent bridge lead --via dingtalk --allow cid-dm --once
conversation cid-dm (staff7): How much budget is left?
    → ledger_balance {}
    ← {"budget_remaining_usd":"100",…}
  agent: 100 USD left in the budget.
  session 5f964ff5927c0f96: done
to conversation cid-dm: 100 USD left in the budget.
$ FEISHU_APP_ID=cli_a1 FEISHU_APP_SECRET=… FEISHU_VERIFICATION_TOKEN=… FEISHU_API_BASE=http://127.0.0.1:8786 sion agent bridge lead --via feishu --allow oc_dm --listen 127.0.0.1:8785 --once
feishu: webhooks on http://127.0.0.1:8785/
chat oc_dm (ou_alice): How much budget is left?
  (the reply is not shown as it is written: Feishu made no card)
    → ledger_balance {}
    ← {"budget_remaining_usd":"100",…}
  agent: 100 USD left in the budget.
  session 6eccad7c90a2c784: done
to chat oc_dm: 100 USD left in the budget.
$ NEXTCLOUD_URL=http://127.0.0.1:8786 NEXTCLOUD_TALK_SECRET=… sion agent bridge lead --via nextcloud-talk --allow n3xtc10ud --listen 127.0.0.1:8784 --once
nextcloud-talk: webhooks on http://127.0.0.1:8784/
conversation n3xtc10ud (users/alice): How much budget is left?
    → ledger_balance {}
    ← {"budget_remaining_usd":"100",…}
  agent: 100 USD left in the budget.
  session 6e03b59d2b58e874: done
to conversation n3xtc10ud: 100 USD left in the budget.
```

What the stand-in for DingTalk was told, a ping and a message
acknowledged over its WebSocket:

```text
POST /v1.0/gateway/connections/open (clientId, clientSecret, localIp, subscriptions, ua)
WebSocket /connect?ticket=ticket-1
acknowledged: {"code":200,"data":"{\"opaque\":\"x\"}","headers":{"contentType":"application/json","messageId":"p1"},"message":"OK"}
acknowledged: {"code":200,"data":"{\"response\":\"OK\"}","headers":{"contentType":"application/json","messageId":"c1"},"message":"OK"}
POST /robot/sendBySession?session=s1 {"msgtype":"text","text":{"content":"100 USD left in the budget."}}
WebSocket closed by the bridge
```

What the stand-ins for Feishu and Talk sent the bridge, and the answers:

```text
POST (the address's check): HTTP 200 {"challenge":"c-42"}
POST (another app's token): HTTP 401 not the app's verification token
POST (encrypted): HTTP 400 an encrypted event: set FEISHU_ENCRYPT_KEY to the subscription's Encrypt Key
POST (the app's token): HTTP 200 {}
POST (unsigned): HTTP 401 not signed with the bot's secret
POST (signed, from another server): HTTP 401 from another server
POST (signed with the bot's secret): HTTP 200 ok
```

And what their APIs were asked, the secrets named and not shown:

```text
POST /open-apis/auth/v3/tenant_access_token/internal (app_id, app_secret)
POST /open-apis/im/v1/messages/om_1/reactions (Authorization: Bearer …) {"reaction_type":{"emoji_type":"Typing"}}
POST /open-apis/cardkit/v1/cards (Authorization: Bearer …) {"data":"{\"body\":{\"elements\":[{\"content\":\"… ledger_balance\",\"element_id\":\"reply\",\"tag\":\"markdown\"}]},\"config\":{\"streaming_config\":{\"print_frequency_ms\":{\"default\":30},\"print_step\":{\"default\":2},\"print_strategy\":\"fast\"},\"streaming_mode\":true},\"schema\":\"2.0\"}","type":"card_json"}
POST /open-apis/im/v1/messages/om_1/reply (Authorization: Bearer …) {"content":"{\"text\":\"100 USD left in the budget.\"}","msg_type":"text"}
POST /ocs/v2.php/apps/spreed/api/v1/bot/n3xtc10ud/reaction/1567 (signed: random, signature) {"reaction":"⏳"}
POST /ocs/v2.php/apps/spreed/api/v1/bot/n3xtc10ud/message (signed: random, signature) {"message":"100 USD left in the budget.","referenceId":"f91ac0ee1d62106e2839bf19c83d9238c61a02b5211837fd629ab915d4875f66","replyTo":1567}
```

### Email

`--via email` reads the agent's own mailbox over IMAP and answers over
SMTP. Each person who writes is a chat, named by their address
(`--allow alice@example.org`), and the reply goes to them alone, in the
thread of what they wrote. What the agent reads is what they wrote
themselves: the message they quote and their signature are left out, and
the subject comes first where a conversation starts.

| Variable | What it is |
|---|---|
| `EMAIL_ADDRESS` | the agent's address |
| `EMAIL_IMAP_URL` | `imaps://imap.example.org` (TLS, port 993), or `imap://…` (STARTTLS, port 143) |
| `EMAIL_SMTP_URL` | `smtps://smtp.example.org` (TLS, port 465), or `smtp://…` (STARTTLS, port 587) |
| `EMAIL_PASSWORD` | the password: an app password, where the provider has them |
| `EMAIL_TOKEN_COMMAND` | instead of a password, a command that prints an OAuth access token, for providers that want one (XOAUTH2) |
| `EMAIL_USERNAME`, `EMAIL_MAILBOX`, `EMAIL_NAME` | the login if it is not the address, the mailbox read (`INBOX`), and the name replies are sent under |
| `EMAIL_AUTHSERV_ID` | the name your provider signs its `Authentication-Results` with (`mx.google.com`; a domain stands for its servers) |
| `EMAIL_TRUST_FROM` | `1` to take `From` as it is, on a server that checks nothing and where no stranger can write |

Anyone can put any address in `From`. The bridge believes it only as far
as your provider vouches for it: it reads the `Authentication-Results`
the provider adds to each message it receives (RFC 8601), and answers
only where DMARC passed for the sender's domain, or DKIM or SPF passed
for a domain aligned with it. It reads the topmost results, which the
provider writes above everything a sender wrote, or with
`EMAIL_AUTHSERV_ID`, the topmost the provider signed with that name. A
message the provider did not check is not answered; `EMAIL_TRUST_FROM=1`
takes `From` as it is. Mail written by machines is not answered either:
out-of-office replies, bounces and delivery reports, mailing lists and
bulk mail. The replies say `Auto-Submitted: auto-replied` (RFC 3834), so
that two machines do not answer each other.

Nothing is sent in the clear: an `imap://` or `smtp://` server must offer
STARTTLS, unless it is on a loopback address (a bridge on the machine
itself, as Proton Mail's is), and anything a server sends between
agreeing to TLS and the handshake is refused. The bridge reads with
`BODY.PEEK`, so no message is marked read or changed. It keeps its place
by UID, starts after the newest message the first time, and waits for
mail with IDLE where the server has it. Attachments are files for the
agent, ten of a message at most ([Pictures and files](#pictures-and-files));
audio ones are voice notes for `--hear`, and an answer spoken with
`--speak` goes back as an audio attachment.

Here a stand-in for a provider offered STARTTLS on both servers, with a
certificate from a CA made for the recording (`SSL_CERT_FILE` names it).
The first run answered nothing, starting after the message already
there. Then three came: Alice's question; one saying it was from Alice
that her domain's DMARC failed, the sender having written a passing
result below the provider's; and her out-of-office reply.

```text
$ EMAIL_ADDRESS=lead@example.net EMAIL_PASSWORD=… EMAIL_IMAP_URL=imap://localhost:8791 EMAIL_SMTP_URL=smtp://localhost:8792 EMAIL_AUTHSERV_ID=mx.example.net SSL_CERT_FILE=ca.pem sion agent bridge lead --via email --allow alice@example.org --once
$ sion agent bridge lead --via email --allow alice@example.org --once
address alice@example.org: DMARC fail for example.org, ignored
address alice@example.org: an automatic message (Auto-Submitted: auto-replied), ignored
address alice@example.org: Subject: Budget

How much budget is left?
    → ledger_balance {}
    ← {"budget_remaining_usd":"100",…}
  agent: 100 USD left in the budget.
  session 98398c65086bd867: done
to address alice@example.org: 100 USD left in the budget.
```

What the servers were told on the second run, the logins shown as `…`:

```text
imap: s1 STARTTLS
imap: (TLS from here)
imap: s2 CAPABILITY
imap: s3 AUTHENTICATE PLAIN …
imap: s4 CAPABILITY
imap: s5 SELECT "INBOX"
imap: s6 UID FETCH 2:* (UID RFC822.SIZE)
imap: s7 UID FETCH 2 (BODY.PEEK[])
imap: s8 UID FETCH 3 (BODY.PEEK[])
imap: s9 UID FETCH 4 (BODY.PEEK[])
smtp: EHLO example.net
smtp: STARTTLS
smtp: (TLS from here)
smtp: EHLO example.net
smtp: AUTH PLAIN …
smtp: MAIL FROM:<lead@example.net>
smtp: RCPT TO:<alice@example.org>
smtp: DATA
smtp: (the message)
  From: <lead@example.net>
  To: <alice@example.org>
  Subject: Re: Budget
  Date: Fri, 30 Oct 2026 22:10:13 +0000
  Message-ID: <936c7a877f9df40f0d4c6f0d442381a3@example.net>
  In-Reply-To: <c1f3@mail.example.org>
  References: <c1f3@mail.example.org>
  Auto-Submitted: auto-replied
  MIME-Version: 1.0
  Content-Type: text/plain; charset=utf-8
  Content-Transfer-Encoding: quoted-printable

  100 USD left in the budget.
smtp: QUIT
imap: s10 LOGOUT
```

### iMessage

`--via imessage` answers Messages on a Mac through
[`imsg`](https://github.com/steipete/imsg), the command line OpenClaw
drives too: the bridge runs `imsg rpc` and speaks its JSON-RPC, on the Mac
itself or, with `IMSG_COMMAND="ssh mac imsg rpc"`, from another machine.
imsg reads Messages' database (it needs Full Disk Access) and sends
through Messages (Automation): there is no server to expose and no
password to keep.

The bridge subscribes to the messages after its place, a message's row
id, so what came while it was stopped is answered; the first time, it
starts after the latest message. A chat is named by its identifier: a
handle for a direct chat (`--allow +15551234567`, or an Apple ID's
address), `chat…` for a group. What is sent from the Mac itself is the
agent's own, and not answered. Voice notes (Core Audio files) are heard
as imsg converts them, and other attachments are files for the agent
(as imsg converts them too: HEIC pictures to JPEG), where the bridge runs
on the Mac; spoken answers go as audio messages, with their text.
`IMSG_SERVICE` sends replies as `imessage`, `sms` or `auto`.

Here imsg was a stand-in speaking its JSON-RPC (imsg runs only on
macOS). After the first run, which answered nothing, came a question, the
Mac's own reply to an earlier one, and a group's message:

```text
$ IMSG_COMMAND="python3 -I fake-imsg.py messages.json imsg-asked.jsonl" sion agent bridge lead --via imessage --allow +15550001111 --once
$ sion agent bridge lead --via imessage --allow +15550001111 --once
chat +15550001111: How much budget is left?
    → ledger_balance {}
    ← {"budget_remaining_usd":"100",…}
  agent: 100 USD left in the budget.
  session e7382195f8970032: done
to chat +15550001111: 100 USD left in the budget.
chat chat777: not allowed, ignored
```

What imsg was asked over the two runs:

```text
{"method": "chats.list", "params": {"limit": 1}}
{"method": "messages.history", "params": {"chat_id": 1, "limit": 1}}
{"method": "watch.subscribe", "params": {"attachments": true, "since_rowid": 10}}
{"method": "watch.subscribe", "params": {"attachments": true, "since_rowid": 10}}
{"method": "send", "params": {"chat_identifier": "+15550001111", "text": "100 USD left in the budget."}}
```

### WeCom, QQ and Zalo

`--via wecom` answers a WeCom (企业微信) self-built app. WeCom posts each
message to `--listen` encrypted and signed, as its `WXBizMsgCrypt` has it:
AES-256-CBC under the callback's `EncodingAESKey`, and a SHA-1 signature
over the callback's token, the time, a nonce and the ciphertext. A
callback whose signature is not the token's, or whose message does not
decrypt to one for the company, is refused before anything reaches the
profile; the address's check (`echostr`) is answered decrypted, as WeCom
asks. Replies go through the app's API (`message/send`) with an access
token from the corp id and the app's secret, renewed before it lapses;
pictures, voice messages and files come through `media/get`, and what the
agent shares goes up through `media/upload`.

| Variable | What |
|---|---|
| `WECOM_CORP_ID` | the company's id, which every callback names |
| `WECOM_AGENT_ID`, `WECOM_SECRET` | the app's id and secret |
| `WECOM_TOKEN`, `WECOM_AES_KEY` | the callback's token and `EncodingAESKey` |

`--via qq` answers a bot of QQ's bot platform (QQ 机器人) in webhook mode:
QQ signs each event with Ed25519 under a key both sides derive from the
bot's secret, and the bridge answers the address's check (op 13) by
signing its token back, signing nothing that could pass for an event. A
chat is named by its kind: `user:<openid>` for a person's direct
messages, `group:<openid>` for a group that mentions the bot,
`channel:<id>` for a channel of a QQ guild. Replies are QQ's passive
ones, each part numbered (`msg_seq`), while QQ takes them for the message
(a few minutes); later a message is sent unprompted, which QQ allows only
a few times a month (`QQ_APP_ID`, `QQ_APP_SECRET`).

`--via zalo` answers a bot of the Zalo Bot Platform, whose API has
Telegram's shape: `getUpdates` long polled, `sendMessage`,
`sendChatAction` for typing (`ZALO_BOT_TOKEN`). A picture or a voice
message comes as an address on Zalo's servers, fetched to be shown or
heard; the bot sends no files, as Zalo takes a bot's pictures and voice
only from a public address.

### WeChat

`--via weixin` answers a WeChat (微信) account through Tencent's iLink bot
protocol, the one its own ClawBot plugin speaks. `sion agent weixin
login` draws a QR code in the terminal: scan it with WeChat on the phone
and confirm, and the bot's token is kept encrypted beside the secrets
(`sion agent weixin logout` forgets it; `WEIXIN_BOT_TOKEN` gives one
instead). The bridge long polls `getupdates`, and each reply carries the
context token of the message it answers, as iLink asks. Text is taken,
and a voice message as WeChat transcribed it; pictures and files, which
iLink keeps encrypted on its CDN, are named to the agent, not fetched.
The protocol is published only as Tencent's plugin's code and others'
descriptions of it, which is what the bridge speaks.

### X, Synology Chat and SimpleX

`--via x` answers an X account's direct messages, polled through the X
API v2 (`/2/dm_events`) once a minute, as X's smaller tiers allow
(`X_POLL_SECS` says otherwise), and answered in the conversation with
whoever wrote. `X_ACCESS_TOKEN` is a user's OAuth 2.0 token with
`dm.read`, `dm.write`, `users.read` and `tweet.read`; with
`X_REFRESH_TOKEN` and `X_CLIENT_ID` (and `X_CLIENT_SECRET` for a
confidential app) it is renewed when X refuses it. The first poll takes
the bridge's place: what was written before it started is not answered.

`--via synology-chat` answers a Synology Chat chatbot: Synology Chat
posts what people write to the bot to `--listen` (the bot's outgoing
URL), each request carrying the bot's token (`SYNOLOGY_CHAT_TOKEN`), and
the reply goes through the bot's incoming URL
(`SYNOLOGY_CHAT_INCOMING_URL`, as Synology Chat shows it), to the user
who wrote, named by the id Synology Chat gives.

`--via simplex` answers a SimpleX Chat profile through the WebSocket API
of SimpleX's terminal client, run on the same machine with the bot's
profile (`simplex-chat -p 5225`; `SIMPLEX_URL` names another address).
The client's API asks no password, so it listens on localhost only, and
the bridge must be its only program. A chat is `@<contact id>` or
`#<group id>`, as the client numbers them; in a group, the bot answers
what mentions it. The profile's address, and accepting whoever asks to
connect, are set up in the client (`/address`, `/auto_accept on`); the
bridge answers only the chats it is let into, or pairs them. A reply
shows as it is written as a SimpleX live message, updated every two
seconds until it is done.

### Nostr

`--via nostr` answers an account's direct messages on Nostr's relays
(`NOSTR_RELAYS`, comma-separated; `wss://relay.damus.io` and
`wss://nos.lol` by default), with its key in `NOSTR_PRIVATE_KEY` (an
`nsec1…` or hex). Both kinds are read: NIP-17's private messages,
encrypted with NIP-44 and sealed and gift-wrapped as NIP-59 has it,
which hide from relays who writes to whom and when; and NIP-04's older
encrypted messages (kind 4), which do not. A reply goes the way its
message came; a chat that has not written yet is written to as
`NOSTR_DM` says (`nip17`, or `nip04`). A chat is the writer's key, as an
`npub1…` (`--allow npub1…`).

Every event is checked against its signature, and a private message
against who sealed it, so a relay can withhold messages but not forge
them. A private reply goes to the relays its reader names for theirs
(kind 10050) as well as to the bridge's own; an older one to the relays
they read (kind 10002). If the account names no relays for its private
messages, the bridge names its own, so that others' apps know where to
write. A relay that asks who reads (NIP-42) is answered with the
account's key. The bridge's place is the time of the last message it
took: on starting again, what was written meanwhile is answered.

### ntfy

`--via ntfy` treats each topic in `--allow` as a chat: polled for what was
published to it since the bridge's place, and answered by publishing to
it, tagged `robot` and `sion` so that the bridge does not take its own
answers for messages (`NTFY_URL`, `https://ntfy.sh` by default;
`NTFY_TOKEN`, or `NTFY_USER` and `NTFY_PASSWORD`, for a server that asks).
ntfy names no writer: whoever may publish to a topic speaks for you there
(`agent:ntfy-topics`), so use a topic of your own on a server that asks
who publishes, or a name nobody can guess.

### Phone calls

`--via phone` answers calls to a Twilio number. Twilio hears what the
caller says (`<Gather input="speech">`) and speaks the agent's reply
(`<Say>`), each step of the call a webhook to `--listen`, signed with the
account's auth token for the public address Twilio was given
(`TWILIO_VOICE_URL`, the number's voice webhook). While the agent works,
the call waits on two-second pauses, the bridge asked again after each;
then the reply is said and the caller heard again. Only the numbers in
`--allow` are answered: other callers are told so, and hung up on.
`PHONE_LANGUAGE` (`en-US` by default) is the language heard and spoken,
`PHONE_VOICE` one of Twilio's voices, `PHONE_GREETING` what a call is
greeted with. A caller is named by the number Twilio gives, which can be
forged (`agent:phone-callers`).

### Pairing and voice notes

With `--pairing`, a bridge gives someone who writes to it directly, and
whom it does not know, a code instead of an answer. They give you the
code, and `sion agent pairing approve` lets their chat in, on that
channel and for that profile only. The bridge never prints the code; it
goes only to them, so what you approve is the chat of whoever gave it to
you. A code lasts an hour, a chat is given one every ten minutes at most,
a channel has three waiting at most, and five wrong approvals in a row
stop approvals for an hour. Codes are consonants and digits, so none
spells a word. Pairing is for direct chats: a private chat on Telegram,
a direct message on Discord, or on Slack with Socket Mode, a room of the
account and one other on Matrix (where the bridge joins the rooms it is
invited to), and Signal's messages. A group chat the bridge does not
answer stays ignored. Slack without Socket Mode is polled channel by
channel, so no direct message from someone it does not know reaches the
bridge, and it refuses `--pairing` there.

With `--hear`, voice notes are transcribed and reach the profile as
`(voice note) …`: Telegram's voice notes and audio files, Discord's and
Signal's audio attachments, Slack's audio clips (with the `files:read`
scope) and Matrix's audio messages (fetched as authenticated media).
With `--speak`, a voice note is answered aloud: on Telegram as a voice
note with the reply as its caption, on Matrix as a voice message, and
on Discord, Slack (with `files:write`) and Signal as the reply with the
audio attached:

| Flag | What hears or speaks | Credentials |
|---|---|---|
| `--hear openai:<model>` | OpenAI's transcriptions, or a compatible server's at `OPENAI_BASE_URL` | `OPENAI_API_KEY` |
| `--hear groq:<model>` | Groq's transcriptions | `GROQ_API_KEY` |
| `--hear deepgram:<model>` | Deepgram's (`nova-3`, say) | `DEEPGRAM_API_KEY` |
| `--hear elevenlabs:<model>` | ElevenLabs' Scribe (`scribe_v1`, say) | `ELEVENLABS_API_KEY` |
| `--hear command:<program>` | A program on this machine, given the audio's file, which prints the text | |
| `--speak openai:<model>:<voice>` | OpenAI's speech (or a compatible server's), as Ogg Opus | `OPENAI_API_KEY` |
| `--speak elevenlabs:<voice id>[:<model>]` | ElevenLabs, as MP3 | `ELEVENLABS_API_KEY` |
| `--speak deepgram:<voice model>` | Deepgram's Aura (`aura-2-thalia-en`, say), as MP3 | `DEEPGRAM_API_KEY` |
| `--speak command:<program>` | A program given the text on its standard input, which writes Ogg Opus or MP3 | |

Here `TELEGRAM_API_BASE` and `OPENAI_BASE_URL` point the bridge at
stand-ins for Telegram's and OpenAI's APIs. Telegram's returned a voice
note from chat 42 and a message from chat 77, which the bridge does not
answer:

```text
$ TELEGRAM_BOT_TOKEN=… TELEGRAM_API_BASE=http://127.0.0.1:8792 OPENAI_API_KEY=… OPENAI_BASE_URL=http://127.0.0.1:8792/v1 sion agent telegram lead --allow 42 --pairing --hear openai:gpt-4o-mini-transcribe --speak openai:gpt-4o-mini-tts:alloy --once
chat 42: (voice note) How much budget is left?
    → ledger_balance {}
    ← {"budget_remaining_usd":"100",…}
  agent: 100 USD left in the budget.
  session a72dec3af7de6702: done
to chat 42 (voice note): 100 USD left in the budget.
chat 77: not known yet; sent a pairing code (sion agent pairing list)
```

The stand-ins received:

```text
POST /v1/audio/transcriptions model=gpt-4o-mini-transcribe file=voice.ogg (6752 bytes, audio/ogg)
POST /v1/audio/speech {"input":"100 USD left in the budget.","model":"gpt-4o-mini-tts","response_format":"opus","voice":"alloy"}
sendVoice chat_id=42 caption="100 USD left in the budget." voice=voice.ogg (7229 bytes, audio/ogg)
sendMessage {"chat_id":77,"text":"I do not know you yet. To talk with me, ask the person who runs me to let you in with: sion agent pairing approve QBRS-9XRF"}
```

Given the code by the person in chat 77, you let them in:

```text
$ sion agent pairing list
waiting: telegram:lead chat 77 (9), 0 minutes ago: "Hello?"
$ sion agent pairing approve QBRS-9XRF
let in telegram:lead chat 77 (9): it is answered from its next message
$ sion agent pairing list
no codes waiting
let in: telegram:lead chat 77
```

and the bridge answers them from their next message:

```text
$ TELEGRAM_BOT_TOKEN=… TELEGRAM_API_BASE=http://127.0.0.1:8792 OPENAI_API_KEY=… OPENAI_BASE_URL=http://127.0.0.1:8792/v1 sion agent telegram lead --allow 42 --pairing --hear openai:gpt-4o-mini-transcribe --speak openai:gpt-4o-mini-tts:alloy --once
chat 77 (9): Thanks! How much budget is left?
    → ledger_balance {}
    ← {"budget_remaining_usd":"100",…}
  agent: 100 USD left in the budget.
  session ee56d1a1f268678a: done
to chat 77: 100 USD left in the budget.
```

`sion agent pairing revoke telegram:lead 77` lets the chat out again.
Pairings are kept in `agents/pairing.json` under `SION_HOME`, readable
only by your user.

### In the terminal, and by voice

`sion agent talk <profile>` is a conversation in the terminal: what you
type goes to the agent, its reply comes as the model writes it, and each
tool it runs shows on a line of its own. The conversation goes on in its
session, the channel's commands included; `--conversation` names another.
With `--voice`, you are heard: press Enter and speak, recorded until you
stop (`--record`, sox's `rec` by default, stopping after a second and a
half of silence), transcribed with `--hear`; the reply is spoken with
`--speak` and played (`--play`: mpv, ffplay, afplay or paplay, the first
there). Each program is given the audio file's path.

### Pictures and files

What a person sends besides text reaches the agent. A picture (a PNG,
JPEG, GIF or WebP, known by its bytes whatever its name says, of 5 MB at
most) is shown to the model with what they wrote. With a workspace, every
file they send, pictures included, is put in the workspace's `inbox/`
under a name not taken yet, and the agent is told where, on a line of
its own after their words:

```text
(A picture was sent, put in your workspace as inbox/<name>: <media type>, <size> bytes.)
(A file was sent, put in your workspace as inbox/<name>: <media type>, <size> bytes.)
```

Without a workspace a picture is still shown, and another file is named
to the agent but not kept. A file of more than 25 MB is not taken, and a
file the messenger no longer gives is named as one that could not be
fetched. The model is shown the pictures of the person's last message
that has any, ten at most, besides the last three of the conversation
(such as a screenshot the agent took): an album sent at once is seen
whole, however many screenshots follow it. A session keeps the bytes of
those pictures and of the last six; the others stay named by their
digest.

The agent sends files back with `share_file`, given a file of its
workspace of 25 MB at most: it goes after the reply, as each messenger
takes files. With `--image-model openai:<model>` on its profile, and a
workspace, the agent also makes pictures (`image_generate`, through
OpenAI's Images API with `OPENAI_API_KEY`, or a compatible server at
`OPENAI_BASE_URL`): it is shown each, which is put in the workspace's
`images/`, named as it asks, for it to share. Making a picture is a
write, as the model calls are not gated: its description goes only to
the provider the operator chose, and spends the operator's credit there.

| `--via` | What a person sends | What the agent shares |
|---|---|---|
| `telegram` | photos (the largest size) and documents, with their captions | a picture as a photo, anything else (a GIF too) as a document, in the message's topic |
| `slack` | the files a message shares (`files:read`) | uploaded and shared in the message's thread (`files:write`) |
| `discord` | attachments | an attachment of a message |
| `matrix` | `m.image`, `m.video` and `m.file` messages, with their captions (not in encrypted rooms) | uploaded, as an `m.image` or an `m.file` |
| `signal` | attachments | attached |
| `mattermost` | the files of a post | uploaded, and posted in the thread |
| `whatsapp` | pictures, videos, documents and stickers, with their captions | a JPEG or PNG as a picture, anything else as a document |
| `email` | attachments, ten of a message at most | a message of its own in the thread, the file attached |
| `imessage` | attachments, where the bridge runs on the Mac | sent through imsg |
| `line` | pictures, videos and files | nothing: LINE takes a bot's files only from a public address |
| `sms` | an MMS's pictures, through Twilio | nothing: Twilio takes media only from a public address |
| the others | nothing: text and voice notes only | nothing |

Where the agent shares a file the messenger does not send, the person is
told it was shared and could not be sent.

The HTTP channel takes files with a message, `"files": [{"name": …,
"media_type": …, "base64": …}]` (a name and a media type if you like;
32 MB at most, the files with it), and answers with what the agent
shared, `"files": [{"name": …, "media_type": …, "size": …, "base64":
…}]`, in a stream's `done` event too. OpenAI's chat API takes pictures
sent inline, `image_url` parts whose URL is a `data:` one, in the last
message; a picture's own address is not fetched.

### Typing, and replies as they are written

While the agent works on a message, a bridge shows the chat that it is at
work, from the moment it takes the message, and shows the reply as the
model writes it: the models stream (Anthropic's and OpenAI's APIs, over
Server-Sent Events), so the text reaches the chat as it is written, not
when the run ends. Each messenger does it the way its API offers:

| `--via` | Typing | The reply as it is written |
|---|---|---|
| `telegram` | `sendChatAction` every 4 seconds (`record_voice` for a reply it will hear) | in a private chat, an animated draft (`sendMessageDraft`), shown again before Telegram's 30 seconds lapse, which the reply replaces; in a group, a message edited every 3 seconds (Telegram takes twenty a minute there) |
| `slack` | in a thread, Slack's status line (`assistant.threads.setStatus`, for an app that may set it); otherwise an hourglass on the message, taken off at the end | in a thread, Slack's own stream (`chat.startStream`), each tool the agent runs a task on its timeline; otherwise a message edited (`chat.update`) |
| `discord` | `POST /channels/…/typing` every 9 seconds | a message edited |
| `matrix` | typing for 30 seconds at a time, turned off at the end | a message replaced (`m.replace`), its updates marked live (MSC4357), one every 4 seconds and twelve at most: each is an event, and homeservers limit them |
| `mattermost` | `POST /users/me/typing`, in the message's thread | a post patched, in the thread |
| `teams` | `typing` activities every 3 seconds | in a chat with one person, Teams' own streaming: the tool the agent runs as an informative update, then the text |
| `google-chat` | none: Chat has none for apps | with a service account's key, a message patched, one write every second and a half (Chat takes one a second in a space) |
| `feishu`, `lark` | the `Typing` reaction on the message, taken off at the end | a streaming card (CardKit), which Feishu types out |
| `dingtalk` | none | on an AI card, given its template (`DINGTALK_CARD_TEMPLATE_ID`) |
| `whatsapp` | WhatsApp's typing indicator, which marks the message read | none: WhatsApp edits nothing a business sends |
| `line` | LINE's loading animation, in a chat with one person | none |
| `signal` | signal-cli's typing indicator, turned off at the end | none: Signal lets a message be edited ten times |
| `sms` | for a WhatsApp sender on Twilio, Twilio's typing indicator | none |
| `nextcloud-talk` | an hourglass on the message, taken off at the end: a Talk bot has no typing | none: a Talk bot edits nothing |
| `email`, `imessage`, `irc`, `twitch` | none | none |

Updates come no oftener than each platform takes them, the latest text
each time. A message that is edited as it grows is first sent with at
least 40 characters, so that the notification it makes has something to
read; a draft and a platform's own stream show at once, and with
`--stream progress` show the tool the agent runs before it writes. A reply
longer than one message holds is split where it reads best, at a
paragraph, a line, a sentence or a word, a code block closed at the end of
one part and opened again at the start of the next, each part finished as
it fills. A reply sent whole is split the same way: a long reply is no
longer cut.

The reply is everything the agent said in the run, each answer a
paragraph: what it said before it called a tool, and what it said after.
When the run ends, the typing indicator stops and the draft becomes that
reply, the same text, which the chat keeps.

`--stream progress` (the default) shows the text and, while a tool runs,
the tool; `--stream partial` shows the text only; `--stream off` sends
the reply once it is done. `--no-typing` shows no typing indicator.

### While the agent works

A bridge answers each chat on a thread of its own, `--parallel` chats at
once (four by default) and one turn at a time in each, the chat that has
waited longest first. Messages a chat sends within `--debounce`
milliseconds of each other (500 by default) are one turn: someone who
writes in three short messages gets one answer, and in a group each line
says who wrote it. What a chat says while the agent works on it does what
`--busy` says:

| `--busy` | A message that comes while the agent works |
|---|---|
| `steer` (the default) | is told to the agent before its next step, as "(The person said, while you worked:) …", and answered with the rest; if the agent is done before that step, it is the next turn |
| `queue` | is answered once the agent is done, each message a turn of its own |
| `collect` | is answered once the agent is done, all of them one turn |
| `interrupt` | stops the agent, whose reply is what it wrote so far, and is the next turn |

A command or a voice note waits for its turn whatever `--busy` says, but
`/stop` does not wait: the agent stops before its next model or tool
call, a model that is writing is cut off between two words (its
connection closed, so that the provider stops too), what it wrote is kept
in the session, and the reply ends with "Stopped.". What waited in the
chat is dropped.

Each message the bridge takes is marked where the messenger can: 👀 while
the agent will answer it or is at it, taken off once it has answered, and
⚠️ if answering failed. `--no-reactions` marks nothing.

| `--via` | Marks |
|---|---|
| `telegram` | the bot's reaction, 👀, gone once answered, or 😢 (Telegram offers bots no warning sign) |
| `slack` | `:eyes:`, then `:warning:` if it failed |
| `discord` | 👀, then ⚠️ if it failed |
| `mattermost` | `eyes`, then `warning` if it failed |
| `matrix`, `signal`, `whatsapp` | a read receipt, as a person's app sends one, then ⚠️ if it failed |
| the others | none |

One messenger account can serve many profiles: `--route <chat>=<profile>`
(as often as needed) has that chat answered by another profile than the
bridge's, each in sessions of its own.

### When a reply does not go through

A reply that fails in a way that may pass (the service down, overloaded
or slow: a server error, too many requests, a timeout) is tried again,
half a second and then a second later. One the service refuses (a client
error: a chat that is gone, a bot that is not let in) is not tried again,
and its message is marked failed. After five failures in a row the
bridge sends the messenger nothing for a minute, then tries once: if that
fails, nothing for two minutes, then four, at most ten; a reply that goes
through starts the count again. A reply not sent after its three tries,
or while the bridge waits, is kept in an outbox, in the channel state
beside the bridge's cursor: tried again every five seconds and when the
bridge starts again, each chat's replies in the order they were written,
the last 200 kept. A messenger that fails to receive is asked again after
2, 4, 8 seconds and so on, a minute at most; only a failure the first
time, such as a wrong token's, stops the bridge.

### Through an agent gateway

The channels above answer on the machine that runs the profile. An agent
gateway (`sion agent gateway`, not the node's `sion gateway`) lets people
reach profiles served from other machines, without opening those
machines to the internet: it takes the messages, and each machine that
serves profiles connects out to it with `sion agent host`. The gateway
checks the profiles' tokens, so they are made on its machine (here with
a `SION_HOME` of its own). There, `--agent` names each profile's agent:
the gateway routes a profile only to a host that proves that agent's
key, so no other agent can take the profile's name:

```text
$ sion agent profile token add lead --agent 0d7240c5…05aa
token 878455fa for profile lead, agent 0d7240c5 (messages only); it is shown once, and kept here only as its hash:
sion_ch_6822a022…
$ sion agent gateway --name gateway-1 --listen 127.0.0.1:8795
gateway gateway-1 on http://127.0.0.1:8795: hosts connect to /v1/hosts, people write to /v1/profiles/<name>/messages
```

On the agent's machine, the host proves for each profile that it holds the
agent's key. It asks the gateway for a challenge and signs it, with the
gateway's name, the agent and the profile, under the account's policy on
the ledger, so the login cannot be replayed at another gateway. Then it
asks the gateway for messages, each request waiting up to 50 seconds, and
never accepts a connection itself. With `--messages 1` it stops after one:

```text
$ sion agent host http://127.0.0.1:8795 --profile lead --messages 1
serving lead at gateway gateway-1
lead (gateway:lead:ops): How much budget is left?
    → ledger_balance {}
    ← {"budget_remaining_usd":"100",…}
  agent: 100 USD left in the budget.
  session 03f902cd42c4bc7c: done
```

The message it answered was written to the gateway with the token made
there (`$TOKEN`):

```text
$ curl -s -X POST http://127.0.0.1:8795/v1/profiles/lead/messages -d '{"text": "How much budget is left?"}'
{"error":"a bearer token is needed"} (HTTP 401)
$ curl -s -X POST http://127.0.0.1:8795/v1/profiles/lead/messages -H "Authorization: Bearer $TOKEN" -d '{"text": "How much budget is left?", "conversation": "ops"}'
{"conversation":"ops","lines":["  → ledger_balance {}","  ← {\"budget_remaining_usd\":\"100\",…}","agent: 100 USD left in the budget.","session 03f902cd42c4bc7c: done"],"said":"100 USD left in the budget.","session":"03f902cd42c4bc7c"} (HTTP 200)
```

The gateway passes a message only while the ledger has the agent in good
standing, as of at most five seconds before. Once alice suspended lead,
its messages stopped at the gateway, which answered them itself:

```text
$ sion agent suspend alice 0d7240c5…05aa
alice signs: Suspend agent 0d7240c5 and every agent below it. Fee 0.0011 USD; valid until Pulse 38.
Pulse 20: Done: agent 0d7240c5 suspended by alice, with everything below it; fee 0.0011 USD.
$ curl -s -X POST http://127.0.0.1:8795/v1/profiles/lead/messages -H "Authorization: Bearer $TOKEN" -d '{"text": "How much budget is left?", "conversation": "ops"}'
{"conversation":"ops","lines":[],"said":"Agent 0d7240c5 is suspended on the ledger: the gateway passes it no messages.","session":null} (HTTP 200)
$ sion agent resume alice 0d7240c5…05aa
alice signs: Lift the suspension of agent 0d7240c5. Fee 0.0011 USD; valid until Pulse 41.
Pulse 24: Done: agent 0d7240c5 resumed; fee 0.0011 USD.
```

A profile goes to the host that proved the agent's key most recently. A
gateway keeps its routes in memory; when it restarts, its hosts log in
again.

With `"stream": true`, a message through the gateway is answered as the
HTTP channel answers it: `session`, `tool` and `delta` events while the
agent works, then `done` with the answer. The host tells the gateway what
the agent did every quarter of a second (`POST /v1/hosts/progress`), and
a host at work is waited for as long as it keeps telling.

## What this does not do yet

- `/stop` stops an agent before its next model or tool call and cuts off
  a model that is writing, but not a tool that is running: that tool's
  call runs to its end. Steering reaches the agent before its next step,
  never in the middle of one. Marks, retries and the outbox have been run
  against a messenger kept in memory and stand-ins, not the platforms
  themselves. Per-chat instructions and tools are the chat's approvers'
  to set: anyone who may approve there may change what the agent is asked
  in that chat, within what its profile gives it.

- The OpenAI-compatible endpoint takes text and the pictures of the last
  message sent inline: a picture sent as an address, and other files, are
  left out, the files the agent shares are not given back, the app's own
  system prompt reaches a new session only as what was said before, and
  no token counts are given. Through an agent gateway, messages carry
  text only.
- Files have been run only against stand-ins for each messenger's API.
  Only pictures are shown to the model; a video, a PDF or a spreadsheet
  reaches the workspace for the agent's tools to open, not the model.
  Telegram gives bots files of 20 MB at most.
  A shared file is sent once, after the reply, and not kept in the outbox
  if it does not go through. An email's files are held in memory until
  they are answered (the last 64, 64 MB at most), so a restart loses
  those not yet answered. Teams, Google Chat, Feishu, DingTalk, Nextcloud
  Talk, IRC and Twitch carry no files here; LINE and SMS take files in
  and send none.
- What a chat is shown while the agent writes is the model's text before
  the run ends. An answer that fails partway (the provider overloaded,
  the next model asked) may show until the next answer replaces it; where
  the reply can only grow (Slack's stream, the HTTP channel's events,
  OpenAI's chunks), the reply then follows it, a paragraph apart, and
  Slack's message is set to the reply once the stream stops. Teams'
  streaming takes only text that grows, so a reply that changed shows at
  its end. Through an agent gateway, the host tells what the agent did a
  quarter of a second at a time, so a stream from it comes in steps.
  Typing and drafts have been run only against stand-ins for each
  platform's API.
- On IRC there is no typing: IRCv3's `+typing` needs the `message-tags`
  capability, which the bridge does not ask for. Telegram's drafts are
  not offered to stop (`can_stop`): a person cannot cut a run short from
  the draft.
- Whoever holds the machine's channel token can start and continue the
  sessions of every profile on the machine, approvals included; whoever
  holds a profile's token, that profile's, approvals only with a token
  made for them; always within each profile's autonomy and the agent's
  charter. The HTTP channel and the agent gateway speak plain HTTP: keep
  them on a local address, or put them behind TLS
  (`agent:channel-token`).
- A bridge trusts its messenger for who writes in a chat it answers. In a
  group chat, everyone who can write there speaks for you, approvals
  included unless `--approvers` names who may decide
  (`agent:telegram-chats`, `agent:messenger-identity`). An approved
  pairing code lets in whoever gave it to you (`agent:pairing`).
- A spoken reply reaches Discord, Slack and Signal as audio attached to
  the text, not as those apps' own voice messages; on LINE and SMS the
  reply is text.
- On IRC, who wrote is the nick the server gives: on a network that does
  not hold nicks to accounts, anyone can take a nick that is free,
  approvals included where `--approvers` names nicks (`agent:irc-nicks`).
  IRC and Twitch carry no voice notes. Mattermost, IRC and Twitch have
  been run only against stand-ins.
- A webhook is taken as its service's when it carries the service's
  signature with the secret you gave (`agent:webhooks`). The signatures
  name no time: a webhook copied on its way could be sent again later,
  and is answered again once its id has left the last thousand
  remembered. The bridge listens on plain HTTP; TLS is your proxy's.
  WhatsApp, LINE and Twilio have been run only against stand-ins. The services
  `--hear` and `--speak` name hear the voice notes and the replies
  (`agent:voice`). Pairing needs a messenger that shows direct
  messages from people it does not know: not Slack without Socket Mode.
  Discord's gateway and Slack's Socket Mode do not answer later what was
  said while the bridge was not listening, and one Discord bot runs one
  gateway connection (no sharding, which Discord asks of bots in 2,500
  servers or more). Slack, Discord, Matrix and Signal have been run only
  against stand-ins for their APIs (the WebSocket client also against
  Discord's real gateway, which it reached and was greeted by), and voice
  notes only against stand-ins for the speech services'.
- Teams and Google Chat are taken as their services' by their tokens
  (`agent:teams-tokens`, `agent:google-chat-tokens`): a webhook copied
  on its way could be sent again while its token lasts (about an hour),
  and is answered again once its message's id has left the last thousand
  remembered.
  Teams carries no voice notes here (its audio comes as files), and
  Google Chat's spoken answers go as text; a Google Chat answer the agent
  takes more than 25 seconds over goes only with a service account's key.
  Both have been run only against stand-ins for Microsoft's and Google's
  side.
- DingTalk's, Feishu's and Talk's messages are taken as their services'
  (`agent:dingtalk-stream`, `agent:feishu-token`,
  `agent:talk-signatures`): Feishu's verification token is a secret in
  every event, not a signature, unless the subscription has an Encrypt
  Key, which encrypts and signs each event; Talk's signature names no
  time. Spoken answers go as text on all three. They have been run only
  against stand-ins.
- WeCom, QQ, WeChat, Zalo, X, Synology Chat, SimpleX, Nostr, ntfy and
  phone calls have been run only against stand-ins for their services
  (`agent:wecom-callbacks`, `agent:qq-signatures`,
  `agent:synology-chat-token`, `agent:simplex-client`,
  `agent:nostr-relays`, `agent:ntfy-topics`, `agent:phone-callers`).
  Nostr's cryptography is checked against NIP-44's and BIP-340's test
  vectors and NIP-19's examples.
  WeChat's iLink protocol is published only as Tencent's plugin's code
  and others' descriptions of it: the bridge speaks what those
  describe, takes text and WeChat's own transcriptions of voice
  messages, and names the pictures and files it does not fetch (iLink
  keeps them encrypted on its CDN). A WeChat bot only replies, so a job
  cannot write to a chat first. QQ takes a bot's unprompted messages
  only a few times a month: a reply later than a few minutes after its
  message may be refused. Zalo, LINE, SMS, X, Synology Chat and SimpleX
  take no files from the bridge, and SimpleX's pictures, voice messages
  and files are named to the agent, not fetched, as are the files of
  Nostr's private messages. X is answered in direct messages only, not
  in posts or mentions. Nostr's group messages (NIP-17 with more than
  two people) are not answered. On a phone call, what was said is what Twilio heard, and the
  reply is Twilio's voice; a reply longer than a few paragraphs is said
  whole. A call that hangs up loses the reply it was waiting for.
- A2A's push notifications are not offered, a task is answered when the
  agent is done (streamed, as it writes), and tasks are remembered only
  while the server runs. Files in A2A messages are not taken: text only.
- `sion agent talk` has no `/stop` while the agent works (the terminal
  is waiting on it): Ctrl-C ends the conversation, the session kept.
  Voice mode needs programs that record and play audio on this machine.
- Discord's voice channels are not joined: since March 2026 Discord
  admits only clients that speak DAVE, its end-to-end encryption (MLS
  and Opus frames encrypted per sender), which the bridge does not.
- On iMessage, who wrote is the handle Messages shows; an SMS's number
  can be forged (`agent:imessage-handles`). Groups carry no mentions
  here, so `--require-mention` answers nothing in them; a voice note is
  heard only where the bridge runs on the Mac (its file is the Mac's).
  imsg has been run only as a stand-in speaking its JSON-RPC.
- An email is answered as from whom the provider's
  `Authentication-Results` say (`agent:email-from`): a provider that adds
  none, or does not check what it says it checked, leaves the bridge
  nothing to go on. Domains are taken as aligned where one is the other
  or under it, not by the public suffix list. Mailboxes are named in
  ASCII; text is read in UTF-8, Latin-1, Windows-1252 and Latin-9 (other
  character sets as UTF-8, where they agree), and HTML for its text. A
  reply goes in the thread of the last message its writer sent. The
  bridge has been run only against stand-ins for IMAP and SMTP servers.
- An agent gateway sees, and could change, the messages and answers it
  passes. It cannot act as the agent, holding no key, and checks the
  agent's standing as of up to five seconds before (`agent:gateway`).
  Messages waiting for an answer are lost if it restarts.
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
