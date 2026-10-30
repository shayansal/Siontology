# Reliable, self-improving agents

**Status: mostly built.** This note sets out how Siontology offers what
the leading open agent runtimes offer for building good agents, and what
its primitives add. Built: identities, budgets, memory with search over
past sessions, consolidation and an archive kept by storage providers,
tasks, storage, the runtime with subagents, hiring, schedules, channels
across machines, skills with a supply chain, self-improvement with
verified promotion, a sandboxed shell and skills' scripts, MCP servers
and storage deals as tools, and the app's Agents tab. Not built: the
parts each section names. Each part says which.

## What the leading runtimes do

Two open-source agent runtimes set the bar in 2026 (sources at the end):

- **Hermes Agent** (Nous Research, MIT). Its distinguishing feature is a
  *learning loop*: the agent writes a **skill** (a `SKILL.md` procedure)
  after a complex task, improves skills while using them, keeps a small
  always-loaded memory, searches its past sessions (SQLite FTS5 with model
  summaries) and builds a model of its user (Honcho). A separate project,
  **Hermes Agent Self-Evolution**, evolves skills offline with DSPy and
  GEPA: it reads execution traces to see why a skill failed, proposes
  variants, evaluates them, and keeps a variant only if it passes gates
  (the full test suite, a size limit, no drift from the skill's purpose,
  and a human reviewing a pull request). Around that: 60+ tools, six
  sandbox backends, MCP, cron jobs, subagents, 20+ chat platforms, and
  trajectory export for reinforcement learning with Atropos.
- **OpenClaw** (MIT). A local gateway that connects an assistant to 20+
  chat channels, with skills in the same `SKILL.md` format, a public
  registry (**ClawHub**, 3,000+ skills), Markdown memory with daily notes,
  hybrid search and a background "dreaming" pass that promotes what
  matters, cron jobs and a heartbeat, multi-agent routing, a browser and a
  canvas, and companion apps. It has no built-in self-improvement loop.

Both use the [Agent Skills](https://agentskills.io/specification) format:
a directory with a `SKILL.md` whose YAML front matter has a `name` (1–64
lowercase letters, digits and single hyphens) and a `description` (what it
does and when to use it), loaded progressively: names and descriptions
always, the body when the skill is used, other files on demand.

## Where they are weak

Their weaknesses are trust problems, and trust is what Siontology's
primitives are for:

| Problem | What happened, or what is missing | Siontology primitive |
|---|---|---|
| **Skills are untrusted code from strangers.** | In February 2026 ClawHub was seeded with hundreds of malicious skills (estimates range from 341 to 1,184), one of them ranked first; malware scanning was added afterwards, and reporting on it notes that scanning will not reliably catch prompt-injection payloads hidden in a skill's instructions. | Claims with bonds, attestations, challenges and compensation (the Assurance Algebra) |
| **Self-improvement grades itself.** | A loop that writes, tests and promotes its own skills can drift or overfit its own evaluations; Hermes's offline evolution puts a human pull-request review last for that reason. | Task Charters: acceptance fixed before the work, verifiers on their own bonds, payment once |
| **Limits live in configuration.** | Allow-lists, approval prompts and sandbox settings are files on the machine the agent runs on; a prompt-injected or compromised agent spends whatever its keys reach. | Agent Charters: budgets, payees and caps enforced by validators; one transaction stops a whole tree of agents |
| **Memory is editable text.** | Memory is Markdown or a database the operator (or the agent) can rewrite; nobody can prove what it knew, when, or that nothing was dropped. | Memory logs: append-only heads on the ledger, attested transaction time, complete recall proofs, belief with provenance |
| **Nothing says who an agent is.** | An agent is a process with API keys. | Agent identities: post-quantum accounts with cards, a delegation tree and subagents |

## The design

A Siontology agent is an ordinary agent runtime (model, tools, memory,
skills, schedules, channels) whose every trust decision goes through the
ledger.

### 1. Skills with a supply chain (on today's primitives)

**Status: built** (`sion skill`; see [Skills from others](../guides/skills.md)).
A skill's permissions are declared in the front matter of its `SKILL.md`,
so a release and its audits commit to them through the bundle root, and
the runtime enforces them ([What a skill may do](../guides/skill-permissions.md)).
No change to the state transition: it uses claims, bonds, `Accept`,
`Challenge` and `Rule` as they are.

- **A skill bundle** is an Agent Skills directory. Its files, sorted by
  path, are encoded canonically and split into leaves like any data a
  storage deal keeps; the **bundle root** is their Merkle root. Any copy can
  be checked against it, and a storage deal can keep the bundle.
- **An audit** is a claim (`skill.audit/v1`) by an auditor, on its own
  bond, that a bundle root does what its description says and nothing
  outside its declared permissions: tools, network, files, spending.
- **A release** is a claim (`skill/v1`) by the publisher that commits to
  the name, version, bundle root, declared permissions and the previous
  version. Its formula is the publisher's dual signature and **its bond or
  any of the audits** of that root. Under the Assurance Algebra, an *or* of
  independent backings adds up: forging a release means corrupting the
  publisher and every auditor.
- **Installing** is `Accept` with the value at risk set to **what the
  agent could lose**, which a charter bounds: an agent with a 200 USD
  budget installs a skill only if the backing behind it is at least κ ×
  200 = 400 USD, and that backing is reserved for it.
- **A malicious skill** is challenged with the evidence; when the
  adjudicator upholds it, the publisher's and auditors' bonds are slashed,
  every agent that reserved backing is compensated, and the release is
  falsified for everyone. A scanner warns; a bond pays.
- **The registry** is the knowledge log itself: every release, audit and
  falsification, with lineage from version to version.

Built since: the runtime runs a skill's scripts in the sandbox of part 2
with what the skill declares and nothing more: the hosts it names
(`sion-network`), reached through an egress proxy that refuses the rest
and every private address; the secrets it names (`sion-secrets`), cut out
of what the scripts print; its time limit; and the calls its
`allowed-tools` pre-approve once a session has read it. Installing shows
all of it and asks for consent before anything is signed. Not built:
evidence that ties a skill caught reaching further to the bond behind its
release (the proxy's log of a script's connections is shown to the model
and kept with the session, not signed).

### 2. The runtime

**Status: built, except as the table says** (`sion agent run`; see
[Agents at work](../guides/agent-runtime.md) and
[Teams, schedules and channels](../guides/agent-teams.md)).

| Part | Plan | Status |
|---|---|---|
| Models | One interface; Anthropic and OpenAI-compatible providers; a scripted model for tests and demos; fallback with a cooldown | Built (`sion-agent`) |
| Rules | Tools declare an effect (read, write, external, spend); autonomy levels L0 to L6 decide what runs, what waits for approval and what is refused; content from outside taints the session | Built |
| Reliability | A journal so that nothing is done twice by accident and an unknown outcome is never repeated; turn budgets that grow with progress; blocking a call that keeps failing; compaction; sessions that pause for approval and resume; a kill switch | Built |
| Spending | Every payment goes through the agent's charter; a payment above its limits is refused by the ledger, not by the runtime | Built (`ledger_pay`) |
| Tools | Files in a workspace, web pages, the ledger (balance, payments, transactions), memory and skills; then tasks and storage; MCP servers mounted as tools | Built: files, web pages, the ledger, memory, skills, subagents, tasks, storage deals, MCP servers, over standard input and output or HTTP (with OAuth), each tool pinned by a digest of its reviewed definition, and a browser and a desktop of its own ([A browser and a desktop](../guides/agent-browser.md)) ([Commands, scripts and MCP servers](../guides/agent-tools.md)) |
| Sandbox | Local with an allow-list and approvals; Docker; a skill's declared permissions decide which | Built: bubblewrap, or a Docker image pinned by digest; no network unless the operator gives one; a skill's scripts get only the hosts their skill declares, through a filtering egress proxy (with Docker, the operator's network decides), the secrets it names and its time limit; its `allowed-tools` pre-approve calls |
| Memory | Episodes, facts and a model of the user as memory atoms; a small core rendered into every prompt; full-text search over past sessions; a consolidation pass that promotes what recurs, keeping provenance | Built: atoms in the agent's memory log, preferences, goals and procedures in every prompt, recall proved complete against the ledger, search, retraction, every session an episode; BM25 search over past sessions' text, each hit checked against the episode on the ledger; consolidation into derived notes under gates, with provenance, superseding and never deleting |
| Durable memory | The log's archive kept by storage providers, proven every round | Built: `sion memory archive` and `restore`, the archive encrypted for its writer and named by the head's manifest on the ledger, retrieved only by the deal's client |
| Skills | Progressive disclosure; created and improved by the agent (part 3); installed only through the decision rule | Built: catalog in the prompt, read on use, written and improved with history kept; releases, audits and installation through the decision rule; falsified releases taken out of use; evaluation, evolution and verified promotion (part 3) |
| Schedules | Jobs at a time, every interval or on a cron expression, each run a session whose result goes to a channel | Built (`sion agent schedule`, `sion agent scheduler`): once, every interval or on cron (UTC), each run a session of a profile, its answer to a file, a webhook or Telegram |
| Subagents | Spawned with identities, accounts and budgets carved from the parent's; one suspension stops the tree | Built: `agent_spawn` registers keyless subagents with budgets carved from the parent's allotment, gives them fee money from the parent's account, and runs their tasks in sessions of their own, two levels deep at most; those of one turn are spawned in one transaction and work at the same time, their payments signed in batches |
| Channels | A command line, an HTTP endpoint for webhooks, and Telegram first; the gateway routes each to its agent | Built: the command line, an HTTP endpoint with the machine's token or a profile's own (and OpenAI's chat API on it, for apps such as Open WebUI, as Hermes has), and Telegram, Slack, Discord, Matrix, Signal, Mattermost, IRC, Twitch, WhatsApp (the Cloud API), LINE, SMS (Twilio), Microsoft Teams, Google Chat, iMessage (through imsg, as OpenClaw now does), DingTalk (Stream mode), Feishu and Lark, Nextcloud Talk and email (IMAP and SMTP, answering only what the provider's DMARC, DKIM or SPF results show to be from its sender) behind one bridge (tested against stand-ins for their APIs; WhatsApp, LINE, SMS, Teams, Google Chat, Feishu and Nextcloud Talk as webhooks checked for their services' signatures or tokens; Discord through its gateway and Slack through Socket Mode, as live connections), each conversation a session, approvals answered in the channel, by named approvers if given; in groups, only what mentions the bot if asked; pairing codes for people who write to a bridge directly and whom it does not know, approved by the operator, as Hermes and OpenClaw have them; voice notes transcribed (OpenAI-compatible, Groq, or a command) and answered aloud (OpenAI, ElevenLabs, or a command) on every messenger; an agent gateway passes a profile's messages to the machine that proved the agent's key, while the ledger has the agent in good standing |
| Hiring | An agent hands work to another through a Task Charter, paid only for an accepted result | Built: `task_post` from the agent's budget, `task_status`, and `task_submit` for the worker, on its task bond |
| Traces | Every session exports as a trajectory in chat format with its outcome, and as a digest | Built |
| Installing and running | An installer, packages, a service of the user's own, a doctor (as `hermes doctor`, `openclaw doctor` and their daemons) | Built: `scripts/install.sh` (a checkout, the repository, or a release checked against its SHA-256 and SSH signature), a `Dockerfile`, a Homebrew formula (head) and a Nix flake, `sion agent service` (systemd user units, launchd agents), `sion doctor`. Not built: released binaries and code signing; the image, the formula and the flake were not built here |

### 3. Self-improvement that cannot fool itself

**Status: built** (`sion skill eval`, `evolve`, `vouch`, `bounty`,
`verify`, `submit` and `lineage`, and `sion agent evolve`; see
[Skills that get better](../guides/skill-improvement.md)). Requiring a
named verifier's attestation, such as an overseer's (step 4), came with
specification v1.15; the rest uses claims, bonds and Task Charters as
they are.

1. **Experience.** Every session is a trajectory: messages, tool calls,
   results, and an outcome (the user's verdict, a check, a task's
   acceptance). Its digest joins the agent's memory log, so the record of
   what happened cannot be rewritten afterwards. *Built: every finished
   session is an episode in the log, with its trajectory's digest.*
2. **Skill creation.** After a successful task that took many steps, the
   agent writes the procedure as a skill, or improves the skill it used.
   Drafts stay private. *Built: after a finished task with at least five
   successful tool calls and no skill written, the runtime asks the agent
   whether to write one, with only its skill tools.*
3. **Evolution.** For a skill with an evaluation set, the runtime reads the
   traces of its failures, asks the model for variants that address them,
   scores each on the set, and keeps the variants that are better and no
   larger (a Pareto front, as GEPA does), under the same gates Hermes uses:
   tests pass, a size limit, the purpose unchanged. *Built: evaluation
   sets of cases with stand-in tools and checks; the gates are the name
   and description unchanged, 15,000 bytes, a version not tried before,
   and no case lost that the parent passed. A variant may change the
   instructions and the skill's other files, never its scripts; a
   profile's own instructions, after the fixed system prompt, evolve the
   same way (`sion agent evolve`). The system prompt itself and the
   tools' descriptions do not evolve.*
4. **Promotion, verified by others.** A variant becomes the new version
   only when **verifiers who did not write it** attest that it passes the
   evaluation, through a Task Charter whose acceptance rule was fixed
   before the variant existed. The release then rests on their attestations
   and points to its parent, so anyone can follow a skill's lineage and the
   evidence for each step. An overseer's approval can be one of the
   required claims, as a human review is in Hermes. *Built: a bounty's
   spec names the release, the set's digest and the cases to pass; its
   verifiers run the set themselves; the release rests on the settled
   result and records it. Since specification v1.15 a bounty can require
   a particular verifier's attestation, an overseer's say
   (`--must-attest`), and the ledger refuses an attestation by the
   version's own author.*
5. **Training data.** Trajectories export as JSON Lines in the chat format
   with tool calls, labelled with their verified outcomes, for fine-tuning
   or reinforcement learning; evaluations and rollouts can be bought from
   others as Task Charters. *Built: `sion agent trace` for sessions,
   `sion skill eval --trajectories` for evaluation runs labelled with their
   results.*

### 4. In the app

**Status: partly built.** The app's Agents tab lists the agents the
device watches, each one's standing and budget proven as a light client
proves them (its charter and the overlays of its whole lineage, against
one certified header), and suspends, resumes or revokes those the device
oversees ([The app](../getting-started/app.md)). Suspending an agent
stops everything below it. Not built: its skills and memory in the tab,
and one switch for all of the device's agents at once.

## What this does not claim

- The runtime and the self-improvement loop are tested here with a scripted
  model: this environment has no access to model providers. Real models
  plug in through the providers above.
- A bond makes lying about a skill costly; it does not make a skill
  correct. An installer still chooses how much to rely on it, and κ.
- Detecting a malicious skill still needs someone to find and prove it;
  what changes is that the finder is paid and the victims are compensated.
- Hermes Agent and OpenClaw have far more integrations (channels, tools,
  sandboxes, voice, apps) than this plan's first version will.

## Sources

- [Hermes Agent documentation](https://hermes-agent.nousresearch.com/docs) and [repository](https://github.com/NousResearch/hermes-agent)
- [Hermes Agent Self-Evolution](https://github.com/NousResearch/hermes-agent-self-evolution)
- [OpenClaw overview, agentic-ai documentation](https://agentic-ai.readthedocs.io/en/latest/AgentPlatforms/openclaw/)
- [Turing Post: Hermes Agent and OpenClaw compared](https://www.turingpost.com/p/hermes)
- [TechInformed: OpenClaw adds VirusTotal scanning for ClawHub skills](https://techinformed.com/openclaw-adds-virustotal-scanning-for-clawhub-skills/)
- [Agent Skills specification](https://agentskills.io/specification)
