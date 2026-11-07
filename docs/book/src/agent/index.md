# Your agent

An agent of your own runs on your computer: a language model works as an
[agent identity](../guides/agents.md), with the tools you give it, under
rules the runtime enforces on your machine and a
[charter](../guides/charters.md) the ledger enforces for it. [Run it on
your computer](../getting-started/your-computer.md) sets one up with one
command, `sion setup`; then `sion` talks with it. These pages say what it
does, and how.

## What is enforced where

| What | Where it is enforced |
|---|---|
| What the agent tries: its autonomy level, approvals, taint, quarantine, confidential content | The runtime, on your machine (`agent:local-policy`) |
| What it can do with money: its budget, its largest payment, whom it may pay, how fast it spends | The ledger: validators check every payment against its charter, whatever the runtime allowed |
| What it remembers | Its memory log on your machine, whose head the ledger keeps and lets only grow |
| Where its conversation goes | The model provider you chose, which sees it (`agent:provider-sees-context`) |

## The pages

- [A run, within its limits](run.md): an agent at work, and what the ledger refuses.
- [Models](models.md): providers, fallbacks, signing in with ChatGPT, and what a model is sent.
- [What it does alone, and what it asks](autonomy.md): autonomy levels,
  taint and quarantine, approvals.
- [Memory](memory.md): the memory log, searching past sessions, consolidation and dreaming.
- [Checked before it is done](checks.md): checks a run must pass, budgets, and the nets under it.
- [Security](security.md): confidential content, signed decisions, imitated notes, attacks tried.
- [Commands, scripts and MCP servers](tools.md), [a browser and a
  desktop](browser.md) and [a canvas](canvas.md): its tools.
- [Skills from others](skills.md), [what a skill may
  do](skill-permissions.md) and [skills that get
  better](skill-improvement.md).
- [Improving itself](self-improvement.md): changes it writes from its
  own work, confirmed on tasks sealed on the ledger before they were
  written; the improver improving itself; changes to its own code built
  and scored in a sandbox, released, attested by others and installed
  only by you.
- [Channels](channels.md), [messengers](messengers.md), and [pairing and
  voice](voice.md): where it is reached.
- [Profiles and schedules](schedules.md), [subagents and hired
  help](teams.md), and [graphs of agents](graphs.md).
- [Coming from OpenClaw or Hermes](import.md): an agent brought over,
  its persona, memories, jobs, models, keys and chats, with a plan shown
  first.
- [Run it on a server](server.md): one process that keeps everything
  running, and one command that puts it on a server of yours.
- [Work it takes on by itself](work.md): what it watches on GitHub and
  in chats, who may ask it for work, what it takes on unasked, its
  mandates, what waits for you, and its record, which anyone can read.
- [Renting your agent out](renting.md): listed by the token from an
  enclave whose attestation the renter checks, paid through a stream for
  the tokens the enclave signed for.
- [Selling and buying: ACP and AP2](commerce.md): its work sold to other
  agents through ACP's checkout, paid on the ledger or by card, and
  bought under the user's AP2 mandates.

And, across these pages: improving the whole agent, not only a skill
(`sion agent improve`, [Skills that get better](skill-improvement.md#improving-the-agent-not-only-a-skill)),
projects that run for weeks (`sion agent project`, [Work](work.md#projects-that-run-for-weeks)),
vouching for and revoking shared facts (`sion memory vouch` and `revoke`,
[Memory](memory.md#facts-from-other-owners-who-vouched-and-pulling-one-back)),
a meeting guest that speaks (`sion agent meeting --speak`,
[Pairing and voice](voice.md#speaking-when-it-is-spoken-to)), and finding
an agent for a task (`sion agent find`,
[Subagents and hired help](teams.md#an-agents-record-and-finding-one-for-a-task)).

## How these pages were made

Every output in these pages is from a real run on a local devnet. The
models are scripted, and the messengers and model providers are stand-ins
for their APIs, because the environment the pages were made in reaches
none of them: what is shown is the runtime, which is the same with a real
model. Where a page shows something else, it says so.

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
evaluation set, improved by evolving variants, and released again on the
attestations of verifiers fixed before the better version existed: see
[Skills that get better](skill-improvement.md).

## What this does not do yet

- Memory atoms stay on the machine that wrote them unless the log is
  archived with a storage provider
  ([Memory from the command line](../guides/memory.md#6-archive-with-a-storage-provider)),
  and sessions are searched on the machine that ran them
  (`agent:recall-local`). What scheduled runs and channels do not do
  yet is on their own pages: [Profiles and schedules](schedules.md) and
  [Channels](channels.md).
- Autonomy, approvals, grants and taint are enforced by the runtime on
  the machine it runs on (`agent:local-policy`); only the charter is
  enforced by the ledger.
- A model provider receives the conversation, tool results included
  (`agent:provider-sees-context`). Sessions are stored unencrypted under
  `$SION_HOME/agents/sessions` (`agent:plain-sessions`).
- Commands run in a sandbox only as far as the kernel's namespaces or the
  container runtime hold (`agent:sandbox`); see [Commands, scripts and MCP
  servers](tools.md) for what else they do not stop.
