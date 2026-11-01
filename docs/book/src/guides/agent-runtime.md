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
alice signs: Register agent-7 as an agent, funded with 100 USD: only to bob, at most 10 USD per payment, 50 USD at once, refilled by 0.05 USD a Pulse, until Pulse 100000, a receipt claim for every payment, may delegate narrower limits. Fee 0.0011 USD; valid until Pulse 20.
Pulse 2: Done: agent bdbdb742 registered, signing as agent-7; fee 0.0011 USD.
agent bdbdb742f506a53671928c83efc1c57321d70dd0636d84ab873575f9f70f60df
…
$ sion faucet agent-7 1
```

## A run

The script checks the agent's budget, pays bob, then tries two payments
its charter forbids: a tip to carol, and 50 USD to bob. At autonomy L6
the runtime lets every call through, so the ledger is what refuses them:

```text
$ sion agent run agent-7 bdbdb742…60df --task 'Pay bob 5 USD for inference.' --model script:pay.json --autonomy L6
session 4c16d4afda9b2fc7 for agent bdbdb742
  → ledger_balance {}
  ← {"budget_remaining_usd":"100","burst_usd":"50","can_spend_now_usd":"50","expires_at_pulse":100000,"fee_balance_usd":"1","max_per_payment_usd":"10","payees":["bob"],"refill_usd_per_pulse":"0.05","status":"active"}
  → ledger_pay {"amount":"5","purpose":"inference","to":"bob"}
  ← Pulse 7: Done: agent paid 5 USD to bob (receipt claim 60bed29d); fee 0.0012 USD. Transaction ef8072e0….
  → ledger_pay {"amount":"5","purpose":"a tip","to":"carol"}
  ✗ refused by the ledger at Pulse 9: Failed (charter caveat violated: payee not allowed); nothing happened except the fee of 0.0011 USD.
  → ledger_pay {"amount":"50","purpose":"a big order","to":"bob"}
  ✗ refused by the ledger at Pulse 12: Failed (charter caveat violated: payment 50 above per-payment limit 10); nothing happened except the fee of 0.0011 USD.
agent: Paid bob 5 USD for inference. The ledger refused a tip to carol, who is not a payee my charter allows, and a 50 USD order, above my 10 USD limit per payment.
session 4c16d4afda9b2fc7: done
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
| `task_post`, `task_status`, `task_submit`, `task_release` | spend, read, external, external | With `--delegate`: hire another agent through a Task Charter (with a check its verifiers run, if you fix one), follow it, deliver one you were hired for (with the attestations it needs), or give one up that cannot be done |
| `shell`, `skill_run` | write, or external with the network | With `--shell`: commands, and a skill's own scripts, in a sandbox that writes only the workspace ([Commands, scripts and MCP servers](agent-tools.md)) |
| `<server>_<tool>` | as reviewed | With `--mcp <server>`: the reviewed tools of an MCP server |
| `storage_keep`, `storage_deals`, `storage_close` | spend, read, write | With `--storage`: keep a workspace file with storage providers, follow and close the deals |
| `share_file` | read | With a workspace: a file of it sent to the person the agent answers, after its reply, where the channel takes files ([Pictures and files](agent-teams.md#pictures-and-files)); a run in the terminal names it |
| `image_generate` | write | With `--image-model openai:<model>` on the profile, and a workspace: a picture made through OpenAI's Images API (`OPENAI_API_KEY`, `OPENAI_BASE_URL`), put in the workspace's `images/` and shown to the agent |
| `ha_states`, `ha_state`, `ha_services` | read | With `--home-assistant` on the profile: Home Assistant's entities and their states, one entity's state, and the services it offers, through its REST API (`HOME_ASSISTANT_URL`, `HOME_ASSISTANT_TOKEN`); what they say is from outside, as a web page is (`agent:home-assistant`) |
| `ha_call` | external | With `--home-assistant`: a Home Assistant service called (a light turned on, a thermostat set), as the token's user may |

## Models

`--model` names a provider and a model, and can be repeated: if a model is
busy or failing (HTTP 429, 500, 502, 503, 504 or 529) or unreachable, the
next one is asked, and the failing one is left alone for 30 seconds. A
request the provider refuses as malformed or unauthorized is not retried.

| `--model` | Provider |
|---|---|
| `anthropic:claude-opus-5-5` | The Anthropic Messages API, API version 2026-10-21, with automatic prompt caching; the key in `ANTHROPIC_API_KEY` |
| `openai:<model>` | Any OpenAI-compatible chat completions API, including local servers: `OPENAI_BASE_URL` (default `https://api.openai.com/v1`), `OPENAI_API_KEY` if it needs one |
| `gemini:gemini-3-pro-preview` | Google's Gemini API, natively (`generateContent`, streamed): `GEMINI_API_KEY` (or `GOOGLE_API_KEY`), `GEMINI_BASE_URL` for another endpoint. Its thinking is not kept; the thought signature it gives with a call goes back with the call, as Gemini 3 requires, and a call another model made goes with the value Google gives for one it did not sign |
| `openai-responses:<model>` | OpenAI's Responses API (`POST /v1/responses`), streamed and not stored: `OPENAI_API_KEY`, `OPENAI_BASE_URL` |
| `chatgpt:<model>` | The Responses API on the ChatGPT plan of whoever signed in on this machine (`sion agent chatgpt login`, below); `sion agent chatgpt models` lists the plan's models |
| `script:<file>` | A scripted model, for tests and demos |

A key variable may hold several keys, separated by commas: each call
starts with the next key; a key the provider limits (HTTP 429) rests a
minute and one it refuses (401, 402, 403) an hour, while the call goes on
with the next key at once. With one key, a limited call is the model's
failure, and the next model is asked.

Requests go through the same guarded HTTPS client as storage providers,
offering the hybrid post-quantum key exchange first.

### Signing in with ChatGPT

`sion agent chatgpt login` signs in with a ChatGPT account, so that agents
run on its plan (`--model chatgpt:<model>`) rather than on an API key. It
is OpenAI's sign-in for open-source apps: your browser opens OpenAI's page
("Continue with ChatGPT"), which comes back to `http://127.0.0.1:1455`
(another port if that one is taken); on the first sign-in on a machine,
OpenAI registers Siontology there, with no client secret. The ID token is
checked against OpenAI's published keys (its issuer, audience, expiry and
nonce), and the plan is used only if you allowed it
(`chatgpt.tokens.use.direct`). The tokens are kept encrypted beside the
secrets, renewed before they lapse (the refresh token comes back new each
time, and is kept with them), and renewed again if the API refuses one.
`sion agent chatgpt status` says who is signed in, `models` lists the
plan's models, `logout` forgets the account.

Requests on a plan are streamed and not stored, as OpenAI asks of this
sign-in; a plan's usage limit, reported partway, counts as the model being
busy, and the next `--model` is asked. Other providers' subscriptions are
not offered: Anthropic, for one, does not let other apps sign in to
Claude.ai plans.

## What a model is sent

Each model call is sent the tools, the system prompt and the conversation
so far, and the start of that stays the same from call to call, so that a
provider's cache of it holds (with Anthropic's models, prompt caching is
asked for). For that, a session's system prompt keeps what the agent knew
of itself, its core memory and the skills it has, as it was when the
session began, or at the first run after its conversation was compacted:
what it records or learns meanwhile is in the conversation. What a chat
asks (`/system`) comes after it, so that changing that leaves the start
as it was. The input tokens a provider read from its cache, or wrote to
it, count as the input they are, in a run's budget as in `/usage`, which
tells them apart, as a trace does
(`gen_ai.usage.cache_read.input_tokens` and
`gen_ai.usage.cache_write.input_tokens`, on a model call's span).

A conversation is kept within 120,000 characters and 36 messages. Past
the characters, the results of calls before the last 12 messages that
are longer than 400 characters are cut first: the calls stay, and each
such result becomes a line saying what it was and how long, since the
call can be made again (one the journal recorded is answered from it).
If that is not enough, or past the messages, the middle is replaced by a
summary, never separating a call from its result. A summary begins with
what the session did that lasts: its last 40 calls that changed something
or reached outside (the files it wrote, the commands it ran, the payments
it made, the messages it sent), earlier ones counted by tool, kept apart
from the conversation so that no summary of a summary loses them.

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
Another agent's message (over [A2A](agent-teams.md#agents-that-talk-a2a)),
a task's output that `task_status` reads, and the task a tainted session
gives a subagent are content from outside too.
A browser or a desktop held to the hosts the operator names is the one
exception: it acts on their pages as the level allows, while the
session's other external calls, and all spending, still wait ([A browser
and a desktop](agent-browser.md#where-it-acts-on-its-own)).

### Quarantine

A tainted session asks before every external call, since what it read may
steer the model. `--quarantine` (on `sion agent run` and on profiles)
keeps what tools return from outside out of the conversation instead: the
model is told what was held, under a handle, and asks questions of it with
`quarantine_read`, giving a JSON schema for the answer. A separate call of
the model, given no tools and told that the content is data, reads it and
answers; the runtime checks the answer against the schema before the
model sees it. Here the reader is the scripted model too, so the answer
it gives is the script's, not a reading of the page:

```text
$ sion agent run agent-7 9dea3a0f…1e98 --task 'What is tiny_http's latest version, and its license? Then look at its sources.' --model script:held.json --web --quarantine --autonomy L5
session 97ec1566b9b1912b for agent 9dea3a0f
  → fetch_url {"url":"https://docs.rs/crate/tiny_http/latest"}
  ← [Held in quarantine as q1: 41726 characters from fetch_url, content from outside. Read what you need from it with quarantine_read: the handle, a question, and a JSON schema for the answer.]
  → quarantine_read {"handle":"q1","question":"What is the latest version of tiny_http, and what is its license?","schema":{"properties":{"license":{"enum":["MIT","Apache-2.0","MIT OR Apache-2.0","another"],"type":"string"},"version":{"format":"version","type":"string"}},"required":["version","license"],"type":"object"}}
  ← [read from q1 without tools]
    {"license":"MIT OR Apache-2.0","version":"0.12.0"}
  → fetch_url {"url":"https://docs.rs/crate/tiny_http/latest/source/Cargo.toml"}
  ← [Held in quarantine as q2: 48582 characters from fetch_url, content from outside. Read what you need from it with quarantine_read: the handle, a question, and a JSON schema for the answer.]
  → fetch_url {"url":"https://docs.rs/crate/tiny_http/0.12.0/source/"}
  ← waiting for a person's approval
session 97ec1566b9b1912b: waiting for you: fetch_url {"url":"https://docs.rs/crate/tiny_http/0.12.0/source/"} (an argument comes from content held in quarantine (0.12.0); request d70d438b). Answer with `sion agent continue 97ec1566b9b1912b --approve` or `--decline <why>`.
```

An answer made of booleans, numbers, strings from an enum the model gave,
and strings of a strict format (a date, a time, an email address, a URL,
a UUID, hex digits, a version) cannot carry instructions, so the session
is not tainted, and its next fetch ran as L5 allows. A free string can
(`{"type": "string", "maxLength": 200}`), so an answer with one taints
the session, as reading the content would have. The strings an answer
gives are labelled as from outside, so the fetch whose address carries
`0.12.0` waited: an external call or a payment whose input carries one
waits for a person, whatever the level, a skill or an allowance says,
since its argument came from content no one checked, and a person
approves each such call on its own (`--always` is not offered). Without
`--quarantine`, the first page taints the session and the next fetch
waits, whatever it carries:

```text
$ sion agent run agent-7 9dea3a0f…1e98 --task 'What is tiny_http's latest version? Then look at its sources.' --model script:open.json --web --autonomy L5
session 923af375a850d5cf for agent 9dea3a0f
  → fetch_url {"url":"https://docs.rs/crate/tiny_http/latest"}
  ← <!DOCTYPE html>
    <html lang="en">
        <head>
            <meta charset="UTF-8">
            <meta name="viewport" content="width=device-width, initial-scale=1">
            <meta name="generator" content="docs.rs 0.0.0 (ca50bb9ce3590453e5c5a422d0cdc34eeef12c58 2026-10-27 )"><link rel="canonical" href="https://docs.rs/crate/tiny_http/latest" /><link rel="stylesheet" href="/-/static/vendored.css?0-0-0-ca50bb9ce3590453e5c5a422d0cdc34eeef12c58-2026-10-27" media="all" />
            <link rel="stylesheet" href="/-/static/style.css?0-0-0-ca50bb9ce3590453e5c5a422d0cdc34eeef12c58-2026-10-27" media="all" />
            <link rel="stylesheet" href="/-/static/font-awesome.css?0-0-0-ca50bb9ce3590453e5c5a422d0cdc34eeef12c58-2026-10-27" media="all" />

            <link rel="search" href="/-/static/opensearch.xml" type="application/opensearchdescription+xml" title="Docs.rs" />

            <title>tiny_http 0.12.0 - Docs.rs</title><script nonce="PlT0lFZkRwK7zSFa9gnddwKH20d9db+CfQqQcDulWWiUyJhr">(function() {
    …
  → fetch_url {"url":"https://docs.rs/crate/tiny_http/latest/source/Cargo.toml"}
  ← waiting for a person's approval
session 923af375a850d5cf: waiting for you: fetch_url {"url":"https://docs.rs/crate/tiny_http/latest/source/Cargo.toml"} (the session has read content from outside; external calls and spending need approval until the taint expires; request 2aea6228). Answer with `sion agent continue 923af375a850d5cf --approve` or `--decline <why>`.
```

What a quarantined read stops, and what it does not
(`agent:quarantine`): the content cannot instruct the model, whose
conversation never holds it, and an answer kept to values cannot either.
But an answer is what the content made the reader say: a number or an
enum's value can be wrong because the page lied. A string is matched
in an argument as it was given, so an argument the model writes from one
in another form (spelled differently, or a part shorter than four
characters) is not caught. And a browser, a desktop and the shell are not
held, since the model acts on what they show: their results taint the
session as before.

## Approvals

At L5 the payment waits. The session is saved, and the command returns:

```text
$ sion agent run agent-7 bdbdb742…60df --task 'Plan the labelling job and pay bob 3 USD for it.' --model script:ask.json --workspace work
session 9795b3a33b6e299b for agent bdbdb742
  → write_file {"content":"Buy 1 GPU-hour of data labelling from bob for 3 USD.\n","path":"plan.md"}
  ← wrote 53 bytes to plan.md
  → ledger_pay {"amount":"3","purpose":"data labelling","to":"bob"}
  ← waiting for a person's approval
session 9795b3a33b6e299b: waiting for you: ledger_pay {"amount":"3","purpose":"data labelling","to":"bob"} (spending at autonomy L5; request 74f23e21). Answer with `sion agent continue 9795b3a33b6e299b --approve` or `--decline <why>`.
$ sion agent continue 9795b3 --approve
  → ledger_pay {"amount":"3","purpose":"data labelling","to":"bob"} (approved by cli:operator)
  ← Pulse 16: Done: agent paid 3 USD to bob (receipt claim 3722068c); fee 0.0011 USD. Transaction 6500057e….
agent: Wrote the plan to plan.md and paid bob 3 USD for data labelling.
session 9795b3a33b6e299b: done
```

The call that waits names its request, `74f23e21`, the start of its
key: it stays the same for as long as the call waits. A decision that
names it (`--request 74f23e21`, or `/approve 74f23e21` on a channel) is
refused if another call waits by then, so it cannot decide a call you did
not see. An approval covers that one call and lasts 15 minutes. The
session keeps each decision, with who made it (here `cli:` and the user
name, on a channel as the channel names them) and when. Declined, the
call is not made and the model is told why:

```text
$ sion agent continue 834618 --decline 'we already paid bob for this job'
  → ledger_pay {"amount":"3","purpose":"data labelling","to":"bob"} (declined by cli:operator: we already paid bob for this job)
  ✗ a person declined this call: we already paid bob for this job
agent: I did not pay bob: you declined, since the job is already paid for.
session 834618219452230b: done
```

### Calls like it, for the rest of the session

`--always` approves the call and, for the rest of the session, the same
call again: a command to run again after a fix, say. `--always <rule>`
approves the calls of its tool that the rule matches, a rule as a skill's
`allowed-tools` writes it ([Skill permissions](skill-permissions.md)):
`write_file(notes:*)`, `shell(git status:*)`, `fetch_url(domain:docs.rs)`.
At L2, where each write waits, the first of two notes is approved with
the writes under `notes/`; the second runs without asking, and the model
is told who allowed it; a write elsewhere still waits:

```text
$ sion agent run agent-7 bdbdb742…60df --task 'Write down the sources and the plan in notes/.' --model script:notes.json --workspace work3 --autonomy L2
session 7e977e07449b5888 for agent bdbdb742
  → write_file {"content":"- the labelling vendor's price list\n","path":"notes/sources.md"}
  ← waiting for a person's approval
session 7e977e07449b5888: waiting for you: write_file {"content":"- the labelling vendor's price list\n","path":"notes/sources.md"} (a write at autonomy L2; request a72bac04). Answer with `sion agent continue 7e977e07449b5888 --approve` (`--always` for the same call again this session) or `--decline <why>`.
$ sion agent continue 7e977e --approve --request a72bac04 --always 'write_file(notes:*)'
  → write_file {"content":"- the labelling vendor's price list\n","path":"notes/sources.md"} (approved by cli:operator, with write_file(notes:*) for the rest of the session)
  ← wrote 36 bytes to notes/sources.md
  → write_file {"content":"1. Label 1,000 images.\n","path":"notes/plan.md"}
  ← [run without asking: allowed for this session by cli:operator: write_file(notes:*)]
    wrote 23 bytes to notes/plan.md
  → write_file {"content":"See notes/.\n","path":"README.md"}
  ← waiting for a person's approval
session 7e977e07449b5888: waiting for you: write_file {"content":"See notes/.\n","path":"README.md"} (a write at autonomy L2; request 7533eb1b). Answer with `sion agent continue 7e977e07449b5888 --approve` (`--always` for the same call again this session) or `--decline <why>`.
$ sion agent continue 7e977e --decline 'leave the README as it is'
  → write_file {"content":"See notes/.\n","path":"README.md"} (declined by cli:operator: leave the README as it is)
  ✗ a person declined this call: leave the README as it is
agent: Wrote the notes; the README is left as it was.
session 7e977e07449b5888: done
```

What a person allows has the limits of a skill's pre-approval: it never
covers a payment, which is approved on its own (`--always` on one is
refused), nor a call whose argument comes from content held in
[quarantine](#quarantine); it cannot be given while the session has read
content from outside, and one given before does not hold until the taint
expires; and
it never goes past what the autonomy level refuses
(`agent:session-allowance`). On a channel, `/approve always [rule]` does
the same, `/allowed` lists what the session may do without asking, and
`/allowed off` withdraws it.

`sion agent continue <session> --say "…"` tells a finished session more
and lets it go on. `sion agent sessions` lists the sessions on this
machine; `sion agent trace <session>` prints one as a single JSON line in
the chat format with tool calls that fine-tuning tools read, with how it
ended. `sion agent trace <session> --otel` prints its steps as
OpenTelemetry spans, OTLP's JSON, for a tracing backend: the session
(`invoke_agent`), each model asked (`chat`, with its tokens) and each call
(`execute_tool`, with its outcome), named as OpenTelemetry's generative-AI
conventions name them. Those conventions are still at their Development
stage, so the names may change; a call's effect and key, and whether the
session was tainted, are under `sion.`.

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
| `session_search` | read | The agent's past sessions on this machine that match the words, each checked against the episode its memory log recorded |

Nothing the agent recorded can be rewritten once committed: the ledger
refuses a head that does not extend the last one. That includes the
episodes, so what an agent did, and the digest of how, stays on record.

### Searching past sessions

An agent with a memory log can search its own past sessions on this
machine with `session_search`: BM25 over what it was asked, what it said
and called, and what its tools answered. Two sessions finished, each
recorded as an episode; a third is asked about Acme:

```text
$ sion agent run agent-9 6a9bfd01…c96f --task "Who gets Acme's invoices?" --model script:third.json --memory d5809f5a…e155 --autonomy L3
session c44d10d5a8d5f733 for agent 6a9bfd01
  → session_search {"query":"acme invoices"}
  ← {"found":[{…,"record":"checked",…,"session":"8b511dee66e1ce8b","snippet":"From now on, pay Acme's invoices to acme-ltd, not acme-old.",…},…],…}
agent: acme-ltd: you told me so in an earlier session, and that session is the one its episode on the ledger recorded.
session c44d10d5a8d5f733: done
memory committed: Pulse 24: Done: memory log d5809f5a now holds 3 atoms (head 3); fee 0.0011 USD.
```

Each hit gives the session, its task, the message that matched best and a
snippet, and `record`: how the session as stored stands against the
trajectory digest its episode recorded in the memory log, whose head is
on the ledger.

| `record` | Meaning |
|---|---|
| `checked` | Its first messages hash to the digest, and the episode is committed to the ledger |
| `uncommitted` | They hash to the digest, but the episode is not committed yet |
| `changed` | They do not: the stored session was edited after its episode was recorded |
| `compacted` | The session was compacted since: the messages the digest covers are not all kept |
| `unrecorded` | The log holds no episode of it: it has not finished, or ran without the log |

`sion agent sessions search` does the same from the command line, by the
agent's wallet name or id:

```text
$ sion agent sessions search agent-9 acme invoices
{
  "found": [
    {
      "matches": 2,
      "message": 0,
      "record": "checked",
      "record_detail": "its first 2 messages hash to the trajectory digest its episode (atom 0 of the memory log) committed to the ledger",
      "score": 1.11,
      "session": "8b511dee66e1ce8b",
      "snippet": "From now on, pay Acme's invoices to acme-ltd, not acme-old.",
      "task": "From now on, pay Acme's invoices to acme-ltd, not acme-old.",
      "who": "person"
    },
    {
      "matches": 4,
      "message": 0,
      "record": "checked",
      "record_detail": "its first 4 messages hash to the trajectory digest its episode (atom 2 of the memory log) committed to the ledger",
      "score": 1.09,
      "session": "c44d10d5a8d5f733",
      "snippet": "Who gets Acme's invoices?",
      "task": "Who gets Acme's invoices?",
      "who": "person"
    }
  ],
  "query": "acme invoices",
  "sessions_searched": 3
}
```

Edit a stored session, here the first one's answer, and its record no
longer checks out:

```text
$ sion agent sessions search agent-9 acme-old
{
  "found": [
    {
      "matches": 2,
      "message": 0,
      "record": "changed",
      "record_detail": "changed: the session as stored does not hash to the digest its episode recorded",
      "score": 2.0,
      "session": "8b511dee66e1ce8b",
      "snippet": "From now on, pay Acme's invoices to acme-ltd, not acme-old.",
      "task": "From now on, pay Acme's invoices to acme-ltd, not acme-old.",
      "who": "person"
    }
  ],
  "query": "acme-old",
  "sessions_searched": 3
}
```

An episode records how many messages its digest covers and how many
times the session had been compacted, so a session that went on after its
episode is checked on the messages the episode covered.

### Consolidation

`sion memory consolidate` reads the episodes recorded since the last
consolidation and the notes the log keeps, and asks a model what is worth
keeping for good: facts, procedures and preferences, each citing its
episodes, proposed with the `memory_note` tool. Gates keep a note only if
it cites episodes it was shown and nothing else, replaces only notes it
was shown, is a fact, a procedure or a preference, fits the limits (120
bytes for a subject or a relation, 1,000 for a value), is not one the
agent keeps already, and is one of at most 16 in the run. Here the model
is scripted, and one of its proposals cites an episode that does not
exist:

```text
$ sion memory consolidate agent-9 d5809f5a…e155 --model script:consolidate.json
agent-9 signs: Commit memory log d5809f5a at 6 atoms, proving it only grew. Fee 0.0011 USD; valid until Pulse 44.
{
  "committed": "Pulse 26: Done: memory log d5809f5a now holds 6 atoms (head 4); fee 0.0011 USD.",
  "consolidated_through": 2,
  "episodes": 3,
  "episodes_left": 0,
  "kept": [
    {
      "atom": 3,
      "from": [
        "atom 0",
        "atom 2"
      ],
      "kind": "fact",
      "note": "supplier-acme invoices-paid-to: acme-ltd",
      "replaces": [],
      "revision": null,
      "untrusted": false,
      "why": "the person said so, and it was relied on later"
    },
    {
      "atom": 4,
      "from": [
        "atom 1"
      ],
      "kind": "preference",
      "note": "user prefers: plain answers about what I cannot do",
      "replaces": [],
      "revision": null,
      "untrusted": false,
      "why": "how the weather question went"
    }
  ],
  "log": "d5809f5a…e155",
  "marker": 5,
  "model": "script:consolidate.json",
  "notes_shown": 0,
  "size_on_ledger": 6,
  "turned_down": [
    {
      "proposal": {
        "kind": "fact",
        "relation": "is",
        "sources": [
          "e7"
        ],
        "subject": "acme",
        "value": "a supplier",
        "why": "no episode says so"
      },
      "reason": "e7 is not one of the episodes shown"
    }
  ]
}
```

Each note kept is a derived atom with a `derived_from` link to every
episode it cites and an extraction trace naming the model and the
consolidator. A note that replaces an older one comes with a revision
that supersedes it: the older one stays in the log, and recall, search
and the prompt leave it out. A summary atom records how far the episodes
were consolidated, so the next run reads only what came after:

```text
$ sion memory consolidate agent-9 d5809f5a…e155 --model script:consolidate.json
{
  "consolidated": 0,
  "log": "d5809f5a…e155",
  "note": "nothing to consolidate: no episode after atom 2"
}
```

The preference is core memory now, in every later session's prompt:

```text
What you remember and always keep in mind (from your memory log; use memory_recall for more):
- (preference) user prefers "plain answers about what I cannot do"
```

### Dreaming

`sion memory dream` reads the sessions themselves, where consolidation
reads their episodes, a line or two each: the sessions of the episodes
recorded since the last dream, those on this machine (12 at most a run),
each read whole by a reader of its own, a model call without tools that
is told the transcript is data, four at a time. A reader proposes facts,
procedures and preferences, each with the words of the session that show
it. Before anything is kept, those words must be in the session, as they
are written there (spacing aside) and 12 characters at least, and a
session that read content from outside yields facts only, kept as a
prompt-injection risk; then consolidation's gates and its recording take
over, each note citing its session's episode. Here one session paid an
invoice for a person who asked to see that payee's invoices first, and
the other read a web page; the readers are scripted, each answering for
its own session:

```text
$ sion memory dream agent-7 0b87c5d6 --model script:dream.json
agent-7 signs: Commit memory log 0b87c5d6 at 5 atoms, proving it only grew. Fee 0.0011 USD; valid until Pulse 35.
{
  "committed": "Pulse 18: Done: memory log 0b87c5d6 now holds 5 atoms (head 3); fee 0.0011 USD.",
  "dreamt_through": 1,
  "kept": [
    {
      "atom": 2,
      "from": [
        "atom 1"
      ],
      "kind": "fact",
      "note": "tiny_http latest release: 0.12.0",
      "replaces": [],
      "revision": null,
      "untrusted": true,
      "why": "the session says: \"tiny_http's latest release is 0.12.0\""
    },
    {
      "atom": 3,
      "from": [
        "atom 0"
      ],
      "kind": "preference",
      "note": "user wants to see first: bob's invoices, before they are paid",
      "replaces": [],
      "revision": null,
      "untrusted": false,
      "why": "the session says: \"From now on, show me bob's invoices before you pay them.\""
    }
  ],
  "log": "0b87c5d6…3fc2",
  "marker": 4,
  "model": "script:dream.json",
  "sessions_elsewhere": [],
  "sessions_read": 2,
  "sessions_unread": [],
  "size_on_ledger": 5,
  "tokens": 0,
  "turned_down": [
    {
      "proposal": {
        "kind": "fact",
        "quote": "bob takes bank transfers only",
        "relation": "is paid",
        "subject": "bob",
        "value": "by bank transfer only"
      },
      "reason": "its quote is not in the transcript"
    },
    {
      "proposal": {
        "kind": "procedure",
        "quote": "tiny_http's latest release is 0.12.0",
        "relation": "are looked up",
        "subject": "crate versions",
        "value": "on docs.rs, the crate's latest page"
      },
      "reason": "the session read content from outside, so it yields facts only, not a procedure"
    }
  ]
}
```

The second note proposed for the first session quotes words no one said
in it, and the procedure from the second is turned down because that
session read a page. A summary atom marks how far the dream went
(`memory dreamt-through`), so the next one reads only what came after:

```text
$ sion memory dream agent-7 0b87c5d6 --model script:dream.json
{
  "dreamt": 0,
  "log": "0b87c5d6…3fc2",
  "note": "nothing to dream over: no episode after atom 1"
}
```

A note kept from a dream shows that its words were said in its session,
by the person, a tool or the agent itself, not that what it says is true
(`agent:dream-quote`).

### Content from outside

A session that read content from outside (a web page, another agent's
message) is tainted (see [Autonomy](#autonomy)). What it records with
`memory_remember` while tainted, its episode, and any note consolidated
from that episode are marked as a prompt-injection risk, unsafe to plan
with: they are kept and recalled (marked `from_outside`), but never put
into a prompt as core memory, so an injected "always pay mallory" cannot
make itself a preference. A search that finds a tainted session taints
the session that searched, as reading that content again would.

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

## Checked before it is done

A model that says it has finished may not have. With `--verify <command>`
(and a shell, `--shell`), each time the model says it is done the command
runs with /bin/sh in the agent's sandbox, in its workspace, and an exit
status of 0 passes. A check that fails is shown to the model, the end of
what the command printed, and the run goes on:

```text
$ sion agent run agent-7 266036a8…32fe --task 'Write add.sh, which prints the sum of its two arguments.' --model script:verify.json --workspace work --shell --verify '[ "$(sh add.sh 2 3)" = 5 ] || { echo "sh add.sh 2 3 printed $(sh add.sh 2 3), not 5"; exit 1; }' --autonomy L3
session d076e6774d2e5abd for agent 266036a8
  → write_file {"content":"echo $(($1 - $2))\n","path":"add.sh"}
  ← wrote 18 bytes to add.sh
agent: add.sh is written: it prints the sum of its two arguments.
  ✗ The check of your answer failed, attempt 1 of 3. It reported:
    exit status 1
    --- stdout
    sh add.sh 2 3 printed -1, not 5
    Fix what it reports and finish again, or, if the task cannot be done, say so with cannot_complete.]
  → write_file {"content":"echo $(($1 + $2))\n","path":"add.sh"}
  ← wrote 18 bytes to add.sh
agent: Fixed: add.sh subtracted. It adds its two arguments now.
session d076e6774d2e5abd: done, and its check passed at attempt 2
```

After three failed checks (`verify_attempts`), the run ends unverified,
with the best of the answers the model gave: the one whose check printed
the highest last line `score: <0 to 1>`, for a check that passes by
degrees, or the later of two that scored the same. The model is also
given `cannot_complete`, to say that the task cannot be done and why: the
run then ends there, as not done, rather than with an answer made to look
finished:

```text
$ sion agent run agent-7 266036a8…32fe --task 'Make the integration tests pass.' --model script:cannot.json --workspace work2 --shell --verify 'sh tests/integration.sh' --autonomy L3
session 9e7ffee20a4ae380 for agent 266036a8
  → shell {"command":"psql -h db.internal -c 'select 1'"}
  ← exit status 2
    --- stderr
    psql: error: could not translate host name "db.internal" to address: Temporary failure in name resolution
  → cannot_complete {"why":"the integration tests need the Postgres server db.internal, and commands here have no network"}
  ← Noted: the run ends here, as not done.
session 9e7ffee20a4ae380: the agent cannot complete the task: the integration tests need the Postgres server db.internal, and commands here have no network
```

The check runs where the agent works, so the agent can change what it
reads or runs there (a test, a script). A check's own files are kept out
of its reach with `--verify-files <dir>`: a directory apart from the
workspace and the skills, bound into the sandbox read-only at `/check`
while the check runs, and only then, so the agent's commands neither see
nor change it. Here the cases are the check's, and the agent cannot list
them:

```text
$ sion agent run agent-7 6a3e150a…6549 --task 'Write sum.sh, which prints the sum of the numbers it is given.' --model script:files.json --workspace work --shell --verify 'sh /check/check.sh' --verify-files hidden --autonomy L3
session ce6f69e7ab63b2f5 for agent 6a3e150a
  → shell {"command":"ls /check"}
  ← exit status 2
    --- stderr
    ls: cannot access '/check': No such file or directory
  → write_file {"content":"echo $(($1 + $2))\n","path":"sum.sh"}
  ← wrote 18 bytes to sum.sh
agent: sum.sh is written: it prints the sum of the numbers it is given.
  ✗ The check of your answer failed, attempt 1 of 3. It reported:
    exit status 1
    --- stdout
    sh sum.sh 1 2 3 4 printed '3', not 10
    Fix what it reports and finish again, or, if the task cannot be done, say so with cannot_complete.]
  → write_file {"content":"s=0\nfor n in \"$@\"; do s=$((s + n)); done\necho $s\n","path":"sum.sh"}
  ← wrote 49 bytes to sum.sh
agent: Fixed: sum.sh added only its first two numbers. It adds them all now.
session ce6f69e7ab63b2f5: done, and its check passed at attempt 2
```

What the check still reads from the workspace (the work itself, a test
runner's settings, a module it imports) the agent can change, so a check
that passed shows that its command exited with status 0 on what the
agent left there, not that the work is right (`agent:verifier`). Work
another agent is paid for is checked by someone else, with a check its
task fixed when it was posted
([Task Charters from the command line](tasks.md#9-a-check-fixed-when-the-task-is-posted)).

## Safety nets

- **No accidental repeats.** A call that changed something is recorded
  under a key made of the session, the tool and the input. The same call
  again returns the recorded outcome, saying so, instead of paying twice.
  Calls meant to run again are not recorded: shell commands, skills'
  scripts and file writes.
- **Nothing lost in a crash.** A call that changes something is written
  down before it runs, and the session is kept on disk then, after
  whatever the call notes on the way, and after it ends. A session resumed
  after a crash finds the call unknown, not forgotten: the turn it was in
  is answered from the journal, and the call is not made again until its
  outcome is known.
- **What "exactly once" means here.** A completed call is never repeated.
  A call whose outcome is unknown is repeated only by a tool that keeps
  its effect from happening twice, as each tool declares: one whose
  service drops a repeated key, or one that writes the same thing again,
  is made again; a payment notes its transaction's digest and the last
  Pulse it may execute in before it is sent, so the ledger says later
  what it did (executed: its receipt, and it is not paid again; past its
  last Pulse: it never will be, and it may be paid again; still pending:
  it waits); any other call waits for a person. `sion agent settle
  <session>` lists the calls whose outcome is unknown, and settles one:
  `--done <what it did>`, or `--not-done` so that it may be made again.
- **Bounded runs.** 12 model turns, 4 more after each turn in which a tool
  call succeeded, at most 32. A call that failed twice with the same input
  is blocked. Long conversations are cut down
  ([What a model is sent](#what-a-model-is-sent)).
- **Going round in circles.** When a call gives the same result three
  times in a row, or two calls take turns with the same results for three
  rounds, the model is told so with the last result; two more, and the run
  stops. Here a model waits for a build by reading a file that does not
  change:

  ```text
  $ sion agent run agent-7 bdbdb742…60df --task 'Tell me when build 7 has finished.' --model script:poll.json --workspace work4
  session 857bcdcab22dae24 for agent bdbdb742
    → read_file {"path":"build-7.txt"}
    ← build 7: running
    → read_file {"path":"build-7.txt"}
    ← build 7: running
    → read_file {"path":"build-7.txt"}
    ← build 7: running

      [read_file gave the same result 3 times in a row: calling again will not change that. Change your approach, or say what stops you.]
    → read_file {"path":"build-7.txt"}
    ← build 7: running

      [read_file gave the same result 4 times in a row: calling again will not change that. Change your approach, or say what stops you.]
    → read_file {"path":"build-7.txt"}
    ← build 7: running
  session 857bcdcab22dae24: stopped going round in circles (read_file gave the same result 5 times in a row); `sion agent continue 857bcdcab22dae24 --say <more>` to go on another way
  ```

  A run's limits set the count (`stuck_after`; 0 turns the check off).
- **A budget of tokens.** `--budget-tokens <n>` (on `sion agent run` and
  on profiles) caps what each run may use, its subagents' tokens
  included. Before each model call, what it sends (estimated at a token
  for every three bytes) and the most it may answer must fit in what is
  left, besides a reserve of 2,000 tokens for a last answer. When they do
  not, the model is asked, within what is left, to say what it did, what
  it found and what remains, and a call it makes then is not made; the
  run ends with `its run's token budget is spent`. The subagents of a
  turn share what is left, the reserve kept back, and what they used
  counts against it. The estimate is not a provider's count: some text,
  and pictures, cost more tokens than it says (`agent:token-estimate`).
- **The kill switch.** `sion agent kill-switch on` stops every agent run
  on this machine before its next model or tool call:

  ```text
  $ sion agent kill-switch on
  kill switch on: every agent run stops before its next call (…/agents/STOP)
  $ sion agent run agent-7 bdbdb742…60df --task 'Pay bob 1 USD.' --model script:pay.json --autonomy L6
  session 8c993ac4876029d0 for agent bdbdb742
  session 8c993ac4876029d0: stopped by the kill switch (…/agents/STOP)
  $ sion agent kill-switch off
  kill switch off
  ```

  The kill switch is local. To stop an agent everywhere, suspend its
  identity on the ledger (`sion agent suspend`): every payment it then
  tries is refused, and so are its subagents'.

## What this does not do yet

- Memory atoms stay on the machine that wrote them unless the log is
  archived with a storage provider
  ([Memory from the command line](memory.md#6-archive-with-a-storage-provider)),
  and sessions are searched on the machine that ran them
  (`agent:recall-local`). Scheduled runs and channels are in
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
