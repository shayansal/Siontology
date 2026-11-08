# What it does alone, and what it asks

What runs without asking is the agent's autonomy level; what it has read
decides what waits for a person anyway; and a person's decisions are kept
with who made them.

Outputs on this page are from real runs on a local devnet, long
identifiers shortened with `…`. The model is a **scripted** one, which
plays a fixed list of tool calls, because the environment these pages were
made in reaches no model provider: with a real model the runtime is the
same, and only `--model` changes.

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
Another agent's message (over [A2A](channels.md#agents-that-talk-a2a)),
a task's output that `task_status` reads, and the task a tainted session
gives a subagent are content from outside too.
A browser or a desktop held to the hosts the operator names is the one
exception: it acts on their pages as the level allows, while the
session's other external calls, and all spending, still wait ([A browser
and a desktop](browser.md#where-it-acts-on-its-own)).

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

### Calls that always wait

Some calls wait for a person whatever the autonomy level, a skill or an
allowance says, unless a person approved that very call:

- Commands that cannot be undone, in the shell and in terminals: a push
  that rewrites or deletes what a remote holds (`git push --force`,
  `--delete`, `+branch`, `:branch`), `git reset --hard`, `git clean -f`,
  `git branch -D`, `rm -rf` of the workspace, a home or the root, `dd` onto
  a device, `mkfs`, `shred`, `wipefs`, SQL that drops or empties a table
  (`DELETE` without `WHERE`), `terraform destroy`, `kubectl delete`, a
  repository or a bucket deleted. What is quoted as text (`echo 'git push
  --force'`) is not a command run. The list is of commands: a script that
  deletes files is not seen in it.
- What the operator names with `--always-ask <rule>` (on `sion agent run`
  and on profiles, repeated for more), a rule as `allowed-tools` writes
  it: `ledger_pay`, `shell(git push:*)`, `send_email`.

The call's wait says why: `it cannot be undone: it rewrites or deletes
what a remote holds`, or `the operator has ledger_pay always wait for a
person`.

### Written rules

`--rules <file>` (on `sion agent run`, and on profiles, which keep the
rules as they were when the profile was added) gives a session the
operator's rules in words: a refund policy, who may be sent what. Before
each call that changes something (a write, an external call, a payment)
is made, a model checks it against them, told the rules, the task, what
the person has said since, word for word, and the call with its input,
and answers `{"allowed": …, "rule": …, "why": …}`. A call it finds against
a rule is not made: the agent is told `not made: it breaks the operator's
rules: "Refunds are at most 5 USD." (9 USD is more)`, and does otherwise
or asks. A check that cannot be made (the model fails, its answer is not
clear, the budget leaves no room) holds the call too. The checking model
is `--rules-model`, or the session's first that is not a script; what it
uses counts against the run's budget, and each check is a step of the
trace. Calls that only read are not checked. A model's reading of rules
is a model's: it can be wrong both ways, which is why the autonomy level
and the charter bound what any call can do.

### Asking you

A session started from the command line or a chat has `ask_person`: the
agent asks you a question when the task needs what only you can say (a
choice of yours, a detail that is missing), with choices if there are a
few. In a chat or a conversation in the terminal, the question is put to
you then and there and your answer comes back to the agent as you said
it; otherwise the run ends with it, `session …: asks you: Which size? (S /
M / L); answer with sion agent continue … --say <answer>`, and goes on
from your answer. What you answer is kept word for word in every summary
([What a model is sent](models.md#what-a-model-is-sent)). Sessions
started by a schedule, for work or as a graph's node have no one to ask
and no such tool; `--no-ask` leaves it out of a run.

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
