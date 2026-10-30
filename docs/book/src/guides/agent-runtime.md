# Agents at work

`sion agent run` puts an [agent identity](agents.md) to work: a language
model plans and calls tools, and the runtime runs each call under rules
you set. The ledger is one of its tools, and the agent's
[charter](charters.md) is the limit no runtime setting can loosen: a
payment outside it is refused by the validators, whatever the runtime
allowed.

Every output below is from a real run on a local devnet (long identifiers
shortened with `…`). The model is a
**scripted** one, which plays a fixed list of tool calls: this
environment has no access to model providers. With a real model the
runtime is the same; only the `--model` changes.

## An agent and its limits

alice registers agent-7: it may pay only bob, at most 10 USD a payment and
50 USD at once, its bucket refilled by 0.05 USD a Pulse, from a budget of
100 USD. The agent's account
pays its transactions' fees, so it gets 1 USD for them:

```text
$ sion agent register alice agent-7 --payees bob --max-payment 10 --burst 50 --rate 0.05 --funding 100
alice signs: Register agent-7 as an agent, funded with 100 USD: only to bob, at most 10 USD per payment, 50 USD at once, refilled by 0.05 USD a Pulse, until Pulse 100116, a receipt claim for every payment, may delegate narrower limits. Fee 0.0011 USD; valid until Pulse 136.
Pulse 118: Done: agent 1e6739d2 registered, signing as agent-7; fee 0.0011 USD.
agent 1e6739d22704e3a8d11f45e383253240c1b4c78217f0f081b37396874e9d1967
…
$ sion faucet agent-7 1
```

## A run

The script checks the agent's budget, pays bob, then tries two payments
its charter forbids: a tip to carol, and 50 USD to bob. At autonomy L6
the runtime lets every call through, so the ledger is what refuses them:

```text
$ sion agent run agent-7 1e6739d2…1967 --task 'Pay bob 5 USD for inference.' --model script:pay.json --autonomy L6
session 7fa7a7ab979c4da7 for agent 1e6739d2
  → ledger_balance {}
  ← {"budget_remaining_usd":"100","burst_usd":"50","can_spend_now_usd":"50","expires_at_pulse":100116,"fee_balance_usd":"1","max_per_payment_usd":"10","payees":["bob"],"refill_usd_per_pulse":"0.05","status":"active"}
  → ledger_pay {"amount":"5","purpose":"inference","to":"bob"}
  ← Pulse 123: Done: agent paid 5 USD to bob (receipt claim 3d512f94); fee 0.0012 USD. Transaction 5350fd4e….
  → ledger_pay {"amount":"5","purpose":"a tip","to":"carol"}
  ✗ refused by the ledger at Pulse 125: Failed (charter caveat violated: payee not allowed); nothing happened except the fee of 0.0011 USD.
  → ledger_pay {"amount":"50","purpose":"a big order","to":"bob"}
  ✗ refused by the ledger at Pulse 127: Failed (charter caveat violated: payment 50 above per-payment limit 10); nothing happened except the fee of 0.0011 USD.
agent: Paid bob 5 USD for inference. The ledger refused a tip to carol, who is not a payee my charter allows, and a 50 USD order, above my 10 USD limit per payment.
session 7fa7a7ab979c4da7: done
```

A script is a JSON array of steps: `{"call": "<tool>", "input": {…}}`,
`{"calls": [ … ]}` for several calls in one turn, and `{"say": "…"}` to
finish. The agent's tools:

| Tool | Effect | What it does |
|---|---|---|
| `ledger_balance` | read | The budget left, the limit per payment, what the agent's bucket lets it spend now and how fast it refills, the payees allowed, the account's balance for fees, the agent's status |
| `ledger_pay` | spend | Pays from the charter's budget; the receipt records which runtime call made the payment |
| `ledger_transaction` | read | Whether a transaction executed, by digest, to check a payment whose outcome is unknown |
| `read_file`, `write_file`, `list_files` | read, write | Files in the directory `--workspace` gives, and nowhere else |
| `fetch_url` | external | Public web pages, with `--web`; local and private addresses are refused |
| `agent_spawn` | spend | With `--delegate`: a subagent with a budget carved from the agent's, working on a task in a session of its own ([Teams, schedules and channels](agent-teams.md)); those of one turn are spawned together and work at the same time |
| `task_post`, `task_status`, `task_submit` | spend, read, external | With `--delegate`: hire another agent through a Task Charter, follow it, or deliver one you were hired for |
| `shell`, `skill_run` | write, or external with the network | With `--shell`: commands, and a skill's own scripts, in a sandbox that writes only the workspace ([Commands, scripts and MCP servers](agent-tools.md)) |
| `<server>_<tool>` | as reviewed | With `--mcp <server>`: the reviewed tools of an MCP server |
| `storage_keep`, `storage_deals`, `storage_close` | spend, read, write | With `--storage`: keep a workspace file with storage providers, follow and close the deals |

## Models

`--model` names a provider and a model, and can be repeated: if a model is
busy or failing (HTTP 429, 500, 502, 503, 504 or 529) or unreachable, the
next one is asked, and the failing one is left alone for 30 seconds. A
request the provider refuses as malformed or unauthorized is not retried.

| `--model` | Provider |
|---|---|
| `anthropic:claude-opus-5-5` | The Anthropic Messages API, API version 2026-10-21, with automatic prompt caching; the key in `ANTHROPIC_API_KEY` |
| `openai:<model>` | Any OpenAI-compatible chat completions API, including local servers: `OPENAI_BASE_URL` (default `https://api.openai.com/v1`), `OPENAI_API_KEY` if it needs one |
| `script:<file>` | A scripted model, for tests and demos |

Requests go through the same guarded HTTPS client as storage providers,
offering the hybrid post-quantum key exchange first.

## Autonomy

`--autonomy` sets what runs without asking. Each level allows everything
the one below it does:

| Level | Reads | Writes (workspace) | External (web) | Spending |
|---|---|---|---|---|
| L0 | no | no | no | no |
| L1 | yes | no | no | no |
| L2 | yes | ask | no | no |
| L3 | yes | yes | no | no |
| L4 | yes | yes | ask | no |
| L5 (default) | yes | yes | yes | ask |
| L6 | yes | yes | yes | yes, within the charter |

Whatever the level, spending stays within the charter: the levels decide
what the agent tries, the ledger what it can do.

Content from outside, such as a fetched page, **taints** the session for
24 hours: until then, external calls and spending wait for approval even
at L6, so instructions hidden in a page cannot move money on their own.

## Approvals

At L5 the payment waits. The session is saved, and the command returns:

```text
$ sion agent run agent-7 1e6739d2…1967 --task 'Plan the labelling job and pay bob 3 USD for it.' --model script:ask.json --workspace work
session 403d8d22d822eda5 for agent 1e6739d2
  → write_file {"content":"Buy 1 GPU-hour of data labelling from bob for 3 USD.\n","path":"plan.md"}
  ← wrote 53 bytes to plan.md
  → ledger_pay {"amount":"3","purpose":"data labelling","to":"bob"}
  ← waiting for a person's approval
session 403d8d22d822eda5: waiting for you: ledger_pay {"amount":"3","purpose":"data labelling","to":"bob"} (spending at autonomy L5). Answer with `sion agent continue 403d8d22d822eda5 --approve` or `--decline <why>`.
$ sion agent continue 403d8d --approve
  → ledger_pay {"amount":"3","purpose":"data labelling","to":"bob"} (decided)
  ← Pulse 130: Done: agent paid 3 USD to bob (receipt claim 42733002); fee 0.0011 USD. Transaction 4c086f4e….
agent: Wrote the plan to plan.md and paid bob 3 USD for data labelling.
session 403d8d22d822eda5: done
```

An approval covers that one call and lasts 15 minutes. Declined, the call
is not made and the model is told why:

```text
$ sion agent continue ec2bdc --decline 'we already paid bob for this job'
  → ledger_pay {"amount":"3","purpose":"data labelling","to":"bob"} (decided)
  ✗ a person declined this call: we already paid bob for this job
agent: I did not pay bob: you declined, since the job is already paid for.
session ec2bdc0df92e8230: done
```

`sion agent continue <session> --say "…"` tells a finished session more
and lets it go on. `sion agent sessions` lists the sessions on this
machine; `sion agent trace <session>` prints one as a single JSON line in
the chat format with tool calls that fine-tuning tools read, with how it
ended.

## Memory

`--memory` gives the agent its [memory log](memory.md), or `--memory new`
opens one for it. The agent records facts, episodes, procedures,
preferences, goals, plans and reflections as atoms in a log only it can
append to; when the session ends, the session itself is recorded as an
episode with its trajectory's digest, and the head is committed to the
ledger:

```text
$ sion agent run agent-7 ebc55841…ded9e --task "Remember that I like invoices paid on Fridays and that Acme's terms are net 30." --model script:learn.json --memory new --autonomy L3
Pulse 13: Done: memory log 391aa9d2 opened for agent ebc55841; fee 0.0012 USD.
memory log 391aa9d2092afc055d43c6c0751481fbaddfcf254509322df2ab295aad7a6386
session 2dfdae605077d42b for agent ebc55841
  → memory_remember {"kind":"preference","relation":"prefers","subject":"user","value":"invoices paid on Fridays"}
  ← remembered as atom 09517eb8 (1 atom(s) not committed to the ledger yet)
  → memory_remember {"kind":"fact","relation":"terms","subject":"supplier-acme","value":"net 30"}
  ← remembered as atom 1d49c69d (2 atom(s) not committed to the ledger yet)
  → skill_write {"description":"Pay a supplier's invoice after checking it against the order and the supplier's terms.","instructions":"1. memory_recall the supplier's terms.\n2. Check the invoice against the order.\n3. Pay it with ledger_pay, giving the invoice number as the purpose.","name":"pay-invoices"}
  ← wrote skill pay-invoices
agent: Noted: you like invoices paid on Fridays, and Acme's terms are net 30. I also wrote down how I pay invoices, as a skill.
session 2dfdae605077d42b: done
memory committed: Pulse 17: Done: memory log 391aa9d2 now holds 3 atoms (head 1); fee 0.0011 USD.
```

Preferences, goals and procedures are the agent's **core memory**: every
later session starts with them in its system prompt. Everything else is
recalled when needed. `memory_recall` with a subject and a relation returns
every atom of the committed log, proved complete against the index root on
the ledger, so the agent knows nothing was left out; the next session does
that before paying:

```text
$ sion agent run agent-7 ebc55841…ded9e --task "Pay Acme's invoice INV-1042 for 7.50 USD to bob, who collects for Acme." --model script:pay.json --memory 391aa9d2 --autonomy L6
session ac81c7b0646a2d4b for agent ebc55841
  → skill_read {"name":"pay-invoices"}
  ← ---
    name: pay-invoices
    description: "Pay a supplier's invoice after checking it against the order and the supplier's terms."
    ---

    1. memory_recall the supplier's terms.
    2. Check the invoice against the order.
    3. Pay it with ledger_pay, giving the invoice number as the purpose.
  → memory_recall {"relation":"terms","subject":"supplier-acme"}
  ← {"committed":[{"committed":true,"confidence_permille":1000,"id":"1d49c69d…","kind":"fact","seq":1,"statement":"supplier-acme terms \"net 30\"",…}],"complete":"proved complete against the index root on the ledger, as of 3 atoms","not_committed_yet":[],"relation":"terms","subject":"supplier-acme"}
  → ledger_pay {"amount":"7.5","purpose":"Acme invoice INV-1042 (net 30)","to":"bob"}
  ← Pulse 27: Done: agent paid 7.5 USD to bob (receipt claim 71125e1f); fee 0.0011 USD. Transaction 08bcdeb4….
agent: Checked Acme's terms (net 30) and paid invoice INV-1042, 7.50 USD.
session ac81c7b0646a2d4b: done
memory committed: Pulse 30: Done: memory log 391aa9d2 now holds 4 atoms (head 2); fee 0.0011 USD.
```

Its prompt ended with what it remembers and the skills it has:

```text
What you remember and always keep in mind (from your memory log; use memory_recall for more):
- (preference) user prefers "invoices paid on Fridays"

Skills you have (read one with skill_read before using it; its other files with skill_file):
- pay-invoices: Pay a supplier's invoice after checking it against the order and the supplier's terms.
```

The memory tools:

| Tool | Effect | What it does |
|---|---|---|
| `memory_remember` | write | Records a subject, a relation and a value, of a kind: fact, episode, procedure, preference, goal, plan or reflection |
| `memory_recall` | read | With a relation, every atom of the committed log about the subject, proved complete, and what is recorded but not committed yet; without one, a local scan |
| `memory_search` | read | Atoms whose subject, relation or value contain the words, newest first |
| `memory_forget` | write | Retracts an atom with a reason: the atom stays in the log, and recall and search leave it out |
| `memory_commit` | write | Commits the head now rather than when the session ends |

Nothing the agent recorded can be rewritten once committed: the ledger
refuses a head that does not extend the last one. That includes the
episodes, so what an agent did, and the digest of how, stays on record.

## Skills

A skill is a directory in the [Agent Skills](https://agentskills.io/specification)
format: a `SKILL.md` whose front matter gives a `name` and a
`description` (what it does and when to use it), then instructions, and
any files the instructions refer to. An agent's skills live in
`$SION_HOME/agents/skills/<agent>` (or `--skills <dir>`). Their names and
descriptions are in every prompt; the model reads a skill's instructions
with `skill_read` when it decides to use it, and its other files with
`skill_file`. With `skill_write` the agent writes down how it did
something, or improves a skill it has; the version it replaces is kept in
the skill's `.history`. After a long task in which it wrote no skill, the
runtime asks it whether to write down how it did the task
([Writing down what worked](skill-improvement.md#writing-down-what-worked));
`--no-reflect` turns that off.

A skill written by the agent stays on its machine. Skills from others
come with bonds behind them and audits that pay when a skill misbehaves:
see [Skills from others](skills.md). A skill can be measured against an
evaluation set, improved by evolving variants, and released again only
once others have verified the better version: see
[Skills that get better](skill-improvement.md).

## Safety nets

- **No accidental repeats.** A call that changed something is recorded
  under a key made of the session, the tool and the input. The same call
  again returns the recorded outcome, saying so, instead of paying twice.
  A payment whose outcome is unknown (the node stopped answering after it
  was submitted) is reported as uncertain and is never repeated: the agent
  is told to check it with `ledger_transaction`. Calls meant to run again
  are not recorded: shell commands, skills' scripts and file writes.
- **Bounded runs.** 12 model turns, 4 more after each turn in which a tool
  call succeeded, at most 32. A call that failed twice with the same input
  is blocked. Long conversations are compacted: the middle is replaced by
  a summary, never separating a call from its result.
- **The kill switch.** `sion agent kill-switch on` stops every agent run
  on this machine before its next model or tool call:

  ```text
  $ sion agent kill-switch on
  kill switch on: every agent run stops before its next call (…/agents/STOP)
  $ sion agent run agent-7 1e6739d2…1967 --task 'Pay bob 1 USD.' --model script:pay.json --autonomy L6
  session c58d85c8dae08969 for agent 1e6739d2
  session c58d85c8dae08969: stopped by the kill switch (…/agents/STOP)
  $ sion agent kill-switch off
  ```

  The kill switch is local. To stop an agent everywhere, suspend its
  identity on the ledger (`sion agent suspend`): every payment it then
  tries is refused, and so are its subagents'.

## What this does not do yet

- Memory atoms stay on the machine that wrote them: keeping the log's
  archive with storage providers is not wired in yet. Scheduled runs and
  channels (HTTP, Telegram) are in
  [Teams, schedules and channels](agent-teams.md).
- Autonomy, approvals, grants and taint are enforced by the runtime on
  the machine it runs on (`agent:local-policy`); only the charter is
  enforced by the ledger.
- A model provider receives the conversation, tool results included
  (`agent:provider-sees-context`). Sessions are stored unencrypted under
  `$SION_HOME/agents/sessions` (`agent:plain-sessions`).
- Commands run in a sandbox only as far as the kernel's namespaces or the
  container runtime hold (`agent:sandbox`); see [Commands, scripts and MCP
  servers](agent-tools.md) for what else they do not stop.
