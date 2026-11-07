# Changelog

Notable changes to Siontology, newest first. There are no releases yet:
everything below is on the default branch, unreleased. Once releases begin,
they will follow [Semantic Versioning](https://semver.org/). The state
transition specification (`docs/spec/state-transition.md`) has its own
version, given below where it changed.

Commit messages before the rename use the working name, Fold.

## Unreleased

### What the agent's work gives for training an open model

- **`sion agent improve export <profile> --out <dir>`** writes the
  profile's sessions that went well as examples to fine-tune on
  (`sft.jsonl`, `sft-validation.jsonl`: OpenAI's chat format with tool
  calls and the tools the model was shown, which sessions now keep), and
  its tasks as environments to reinforce in (`tasks.json`,
  `environments.jsonl`). Sessions that did not finish, were compacted,
  held confidential content or a secret the machine keeps, or whose task
  is sealed are passed over. Nothing sealed is exported, and a later seal
  never seals a task exported.
- **`sion agent improve env --tasks <file> --case <id>`** runs one task as
  an episode with the policy outside (`sion_agent::episode`): requests in
  OpenAI's chat format on standard output, the policy's turns on standard
  input, and the reward last, the share of the task's checks that held.

Tested on a devnet (the export of a sealed profile, an episode driven
through the binary to the whole reward, a second seal keeping the tasks
trained on out) and with outside policies that earn all, part and none of
the reward. No model has been trained on them here.

### A canvas the agent shows you things on

- **`--canvas`.** A page on this machine, by a token of 128 bits, where
  the agent shows what it works on as A2UI (v0.8) messages: text,
  pictures, rows, columns, lists bound to a data model, cards, tabs,
  dividers, buttons, check boxes, text fields, sliders and choices,
  checked and applied all or none, watched live as they change
  (`sion_agent::canvas`). A button's action, with what the person
  entered, comes back for `canvas_wait`. The page runs nothing the agent
  sends and reaches only its own address; showing is a write, not an
  external call. The canvas stays at its address for the session's runs
  in the process that serves it.

Tested on the server and in a real run with a browser as the person; a
book page ([A canvas](docs/book/src/agent/canvas.md)). Trust atom
`agent:canvas`.

### A desktop used in code as well as with the pointer

- **Code actions.** The `computer` tool's `run_code` runs a Python
  program or a bash script on the desktop: its display, the workspace,
  the desktop's sandbox with no network, 30 seconds unless it asks for
  more within the sandbox's limit, the program passed in a variable. It
  answers with how the program ended and what it printed, and a
  screenshot. `--computer-no-code` leaves it out.
- **Windows by name.** `windows` lists the windows open with their ids,
  names, places and sizes; `focus` brings one forward.

Run for real in the tests (Xvfb and bubblewrap: a window opened with the
X library, found and focused, programs that print, fail and run too
long), and in the book's transcript with Chrome on the desktop.

### Models on the cloud platforms, and providers by name

- **AWS Bedrock** (`--model bedrock:<model>`): the Converse API natively,
  each request signed with Signature Version 4 (an access key from the
  environment or AWS's own files, a session token with it) or carrying a
  Bedrock API key (`AWS_BEARER_TOKEN_BEDROCK`); pictures, tool calls and
  their results, Claude's signed thinking given back, Claude's and Nova's
  prompts cached at two points (`sion_agent::cloud::Bedrock`).
- **Google Vertex AI** (`--model vertex:<model>`): Gemini and Claude,
  streamed, with a token given or minted from a service account's key (a
  JWT signed with RS256) or a person's sign-in from `gcloud auth
  application-default login`, renewed before it lapses; Gemini in express
  mode with an API key (`sion_agent::cloud::Vertex`).
- **Azure OpenAI** (`--model azure:<deployment>`): the v1 API at the
  resource's endpoint, its key as `api-key`.
- **Providers by name**: `openrouter:`, `deepseek:`, `moonshot:`, `xai:`,
  `groq:`, `together:`, `fireworks:`, `mistral:` and `cerebras:`, each with
  its own address and key (`<NAME>_API_KEY`), so that several mix in one
  agent's models; their keys can be kept on the machine, and OpenClaw's
  and Hermes's settings naming them are imported as they are.

Tested against stand-ins of each API (the signatures against those AWS's
SDK computes for the same request); not run against AWS, Google Cloud or
Azure. Trust atom `agent:cloud-credentials`.

### The agent improving itself: its own changes, sealed tasks, the improver, its own code

- **Tasks from its own work.** `sion agent improve mark` says how a
  session went; `harvest` turns the profile's sessions with a known
  outcome (marked, or their `--verify` check passed) into cases whose
  stand-ins answer what the session's tools answered and whose checks are
  that a run makes the changes a good session made, or none of a bad
  one's (`sion_agent::harvest`).
- **Sealed before anything is tried.** `seal` splits a set by a draw from
  its digest and a seed, adds canary cases to the sealed part, and allows
  a number of confirmations, each held to 5% split among them all; with
  `--wallet` the seal's digests are published as a claim
  (`agent.seal/v1`) first.
- **Changes it writes.** `run`: a proposer model, shown the tasks the
  agent failed among those to choose on and never a sealed task, proposes
  what the agent should learn and which models to run on. Each change is
  gated (size, allowed models, nothing tried twice, no run of eight words
  shared with a sealed task, a canary's code word stopping the loop), run
  once on every task and then in full, screened at 95% against the agent
  as it is, and confirmed on the sealed tasks at the seal's level with no
  sealed task lost and no canary passed. Variants form an archive drawn on
  by Thompson sampling over each line's passes (the Huxley–Gödel
  Machine's clade metaproductivity), kept between runs
  (`sion_agent::archive`). What the agent learns is kept apart from its
  operator's words; `--apply` or `adopt` makes a change the profile's and
  `undo` takes it back. Kept changes are recorded with their evidence and,
  with `--wallet`, published as claims (`agent.improvement/v1`) naming
  their seal; `lineage --check` holds each to its claim and its seal.
- **The improver improving itself.** `improver` grades the proposer's
  instructions and settings by how much the agent improves on held-out
  tasks when it writes the changes, over problems given; it proposes its
  own changes under its own instructions, and one is kept only when the
  agent improves more with it beyond chance there and on problems held
  out, at a corrected level. The improver kept proposes next.
- **Its own code.** `code`: a coder model edits a copy of the source
  without its `.git`, its commands in the sandbox with no network; the
  harness takes the diff in a fresh clone with git's hooks, filters and
  user settings off, refuses symbolic links and git settings of the
  copy's own, and holds the patch to the paths it may touch, a size and
  added lines without `unsafe`, network or new processes
  (`sion_agent::guard`); build and tests run in the sandbox, and the
  candidate is scored over its standard input and output by the harness,
  which answers every model and tool call (`sion_agent::remote`; this
  runtime's side is `sion agent case --stdio`). A confirmed change is a
  release (`agent.release/v1`); `attest` lets another account rebuild and
  score it and publish what it found (`agent.release-attestation/v1`), and
  `install` applies it to a checkout only with attestations by verifiers
  you name (`--undo` takes it out).
- **Statistics.** `sion_agent::compare` gains `z_for`, `wilson_with` and
  `newcombe_with`: intervals at any level, for corrected confirmations.
- Trust atoms `agent:self-improvement` and `agent:code-release`. Tested on
  a devnet with scripted models, and for code with a small harness in
  Python in bubblewrap; not run on this repository's own code or with a
  real model.

### Improving the agent, projects, shared facts, a speaking guest and finding an agent, as commands

The five pieces that were only in the library now have commands, and
the documentation says so.

- **`sion agent improve try`** tries words added to a profile's
  instructions (`--instructions`), the models a route reaches for
  (`--route <route>=<models>`, `default` for its own) and a vote
  (`--vote`), each on a set of tasks against the agent as it is; one is
  kept only when better beyond chance and failing no held-out task
  (`--held-out`) it passed, recorded with its evidence, and `--apply`
  makes the best words and models the profile's. A route's models are
  tried against that route's. `sion agent improve kept` reads them back.
- **`sion agent project`** starts, works on, shows, reports, pauses,
  resumes, finishes and stops a project. A session on a project has
  `project_note` and `project_milestone`, and each of its payments is held
  to what the project has left before it is sent, given back if it is not
  sent or the ledger refuses it, or if a payment whose outcome was unknown
  turns out never to have been made; never held twice for one call. A
  project's session has no subagents, tasks or storage deals, and the
  project's file is changed under a lock across processes. `sion agent scheduler` makes the
  check-ins (working on a project once first with `--work-at-check-in`);
  a report that could not be sent is made again at the next check-in.
- **`sion memory vouch`, `revoke` and `standing`.** A vouching is now
  named in the voucher's index (`memory-vouched:<atom>`), and a claim the
  ledger does not issue is said with its reason. A session that keeps
  another owner's log in mind brings what it knows of the log's vouchings
  and revocations up to date from the ledger's events before each run: a
  fact its writer revoked, and whatever was drawn from it, leave its
  prompt; a vouched one says who vouched, while its vouching stands. A
  voucher's revocation withdraws only its own vouching, and anyone else's
  counts for nothing.
- **`sion agent meeting --speak`** gives the guest that joins by a link a
  voice and a PulseAudio microphone of its own (`--answer-to` for the
  words that name it; by default its name, and its name without what is
  in brackets): it answers a line that asks it something by name,
  from the transcript alone, into the meeting and the transcript. Its
  microphone and a meeting were not run here; such a guest has no camera.
- **`sion agent find`** ranks the rental listings and agents named for a
  task by the fit of their offers and their records, held to an enclave
  that checks out (`--enclave`) or a price (`--max-per-1k-output`), the
  records read from the node's events page by page.

- **Payments**: a node's answer to a payment that could not be read now
  leaves its outcome unknown, to be found from the ledger by its noted
  transaction, rather than counting it as not sent.

Tested with scripted models: `crates/sion-cli/tests/agent_improve.rs`, and
on a devnet `crates/sion-cli/tests/agent_colleagues.rs`. A review by an
agent that had not written the code found holds that could leak, a
voucher able to pull any fact for a small bond, route models compared
with the wrong baseline and reports that could repeat entries; all are
fixed here.

### Documentation: the agent first, and what no command reaches yet

- **The README** now leads with the agent: where it is reached, what it
  works with, its limits, what it knows and how it gets better, help and
  work of its own; then selling, buying and renting (ACP, AP2, x402,
  streams, renting from an enclave, the public reads), the ledger
  underneath, and the app. Its figures are current: nineteen crates, about
  215,000 lines under `src/` and 51,000 in `tests/`, 117 pages in the book.
- **What no command reaches yet** is said wherever it is described:
  improving the whole agent, projects, vouching for and revoking shared
  facts, a meeting guest that speaks, and finding an agent for a task are
  in `sion-cli`'s library and unit-tested only. The implementation status
  had called them built; its summary now gives them a row of their own,
  partly built, beside new rows for the work the agent takes on and for
  renting, records and the public reads. The book's introduction, the
  agent pages, the crate map (now with `sion-enclave`), `ARCHITECTURE.md`,
  the documentation index and `sion-cli`'s README follow it.

### Agentic commerce: the rest of ACP, and AP2's certificates and recurrence

- **Cards**: the merchant takes ACP's `dev.acp.tokenized.card` handler,
  charging a delegated card token through Stripe's shared payment tokens
  (built to Stripe's documentation, tested against a stand-in) or through
  an ACP PSP. `sion agent commerce psp serve` is a PSP's delegate payment
  API: a card vaulted for one checkout, its total, one merchant and a
  short time, kept only in memory and encrypted, its acquirer simulated.
  Trust atom `agent:acp-card`.
- **3-D Secure**: a merchant can ask for it (`authentication_metadata`)
  and charges a card only with an acceptable `authentication_result`.
  `sion agent commerce three-ds serve` is ACP's Delegate Authentication
  API, with its 3DS Method and challenge, its issuer simulated. The buyer
  authenticates when the merchant or the issuer asks.
- **Request signing**: `Signature` and `Timestamp`, checked per client, as
  ES256 or Ed25519 over the body's RFC 8785 canonical JSON or as
  HMAC-SHA256 over `timestamp.body`; `sion agent commerce keygen` makes a
  key. Trust atom `agent:acp-signing`.
- **Orders and webhooks**: orders in ACP's shape, sent as `order_create`
  and `order_update` webhooks signed with `Merchant-Signature`, retried
  and delivered in order; `sion agent sell orders`, `fulfill` and `refund`
  through the merchant's admin API, a ledger refund a transfer back to the
  payer. `sion agent commerce webhooks` receives them.
- **Carts and feeds**: ACP's cart API on the merchant; the agent-hosted
  Feed API (`sion agent commerce feeds serve`), and `sion agent sell feed`
  to publish a catalog to one or write its offline snapshot.
- **Kept across restarts**: a merchant's sessions, carts, orders,
  idempotency keys, AP2 history and webhook outbox are kept in a file and
  taken back, quotes included; a payment left settling is finished from
  its verified receipt.
- **AP2**: a root mandate's key from an `x5c` certificate chain to a
  trusted root (RFC 5280 path checks); `payment.agent_recurrence` against
  the payments recorded under the same mandate, with calendar periods and
  `max_occurrences`; budgets in major units, exactly.
- **Checked against outside work**: 34 requests and answers from a live
  run meet ACP's published JSON Schemas. The check found four faults here,
  now fixed; among them, the ledger instrument's credential is now carried
  as ACP's `token` (the old `transaction` is still taken). Scripts for
  AP2's reference SDK are in `scripts/interop/`. JSON numbers now parse
  exactly (`serde_json`'s `float_roundtrip`), which RFC 8785 needs.

### Agentic commerce: ACP checkout and AP2 mandates

- **Selling through ACP**: `sion agent sell serve` serves the Agentic
  Commerce Protocol's checkout API (API version 2026-04-17) for a catalog
  of agent work, with ACP's headers, idempotency and errors, and a payment
  handler of its own, `org.siontology.ledger`: the session's total as an
  x402 quote, paid with a signed Siontology transaction, the order made
  only once its receipt verifies. `sion agent buy` buys from such a
  merchant within a cap, from a wallet or under an Agent Charter. Trust
  atom `agent:acp-checkout`.
- **AP2 mandates**: the Agent Payments Protocol v0.2: SD-JWT (RFC 9901)
  with ES256 and Delegate SD-JWT chains, checkout and payment mandates
  checked as AP2's merchant and payment side must, every constraint it
  defines, signed receipts, and issuing and closing mandates for a
  shopping agent. A merchant takes them through the ACP extension
  `org.siontology.ap2` and checks them before any money moves. Checked
  against the encoded tokens of AP2's specification. Trust atom
  `agent:ap2-mandate`.

### Improving itself, shared memory, projects and the trust layer

The first four of these were written in `sion-cli`'s library and
unit-tested, and no command reached them yet (see the documentation entry
above); the trust layer is served.

- **Improving the whole agent**: `self_improve` tries changes to the
  agent itself — the words its operator gives it, the models a kind of
  task reaches for, how it works a task — and keeps one only if it is
  better beyond chance on the tasks tried (Newcombe's 95% interval) and
  fails no held-out task the agent already passes. Each kept change is
  recorded with its evidence. Trust atom `agent:measured-improvement`.
- **Shared memory with sources**: an account vouches for another owner's
  memory atom with a claim on its own bond; the writer or a voucher
  revokes one with a claim named under it in the index. A revoked fact,
  and everything derived from or assuming it however deep, is left out, so
  a poisoned fact can be traced and pulled. Trust atom
  `agent:vouched-memory`.
- **Projects that run for weeks**: a goal, a budget, milestones and a
  check-in that reports what the agent did since the last one. A project
  that is paused, ended or out of budget takes no more work. Trust atom
  `agent:project-budget`.
- **A meeting guest that speaks**: given a microphone of its own
  (PulseAudio), it answers lines that name it, found anywhere in the line
  and matched as leniently as a wake phrase. Trust atom
  `agent:meeting-voice`.
- **The trust layer**: the HTTP channel serves three public reads for
  agents on other frameworks, without a token — an agent's record, a
  transaction's outcome with how to verify its receipt, and a rental
  listing with whether its enclave checks out. `GET /v1/trust` says what
  it serves.

### Renting an agent out, and agents' records

- **Agents in enclaves**: a new crate, `sion-enclave`, checks an AMD
  SEV-SNP attestation report against AMD's chain for Milan, Genoa and
  Turin (its root and signing keys built in), matches the chip and
  firmware versions the chip's key names, refuses a guest launched
  debuggable, and checks the 64 bytes binding the session's keys. Reports
  are asked for through the kernel's configfs-tsm files. Tested against a
  real Milan report and chip keys.
- **Renting by the token**: `sion agent rent serve` lists a profile's
  agent with a price per thousand input and output tokens, attests the
  enclave it runs in and serves sealed requests; `sion agent rent use`
  checks the attestation before anything is sent or paid, escrows in a
  payment stream and pays for the tokens the enclave signed for; `sion
  agent rent show` reads a listing and says whether its enclave checks
  out. A rented session gets none of the owner's memory, files, skills or
  reach. Trust atoms `agent:rented-enclave`, `agent:metered-tokens`.
- **An agent's record**: read from the ledger's own events — tasks
  delivered and what they earned, tasks released, delivered results later
  proven false — and turned into one number to trust it by. Finding an
  agent for a task weighs how well its offer fits the task against that
  record, held to what is needed (an enclave, a price). Trust atom
  `agent:agent-record`.

### The harness, the voice and the browser

- **Reasoning kept across tool calls**: what a model thought is given back
  to it with the next request, as its provider gave it (Anthropic's signed
  thinking and redacted thinking, the Responses API's encrypted reasoning
  items, Gemini's thought signatures on text as on calls, a chat server's
  reasoning text within the task), and to no other model; a provider that
  refuses it is asked again without it. How hard a model thinks is written
  after its name: `anthropic:claude-opus-5-5#high`, `openai:o3#low`,
  `gemini:gemini-2.5-pro#8000` (an effort, or a budget of tokens), on
  `--model` and on a route, with the room to think counted in the run's
  budget. Trust atom `agent:kept-reasoning`.
- **Tool search**: past 40 tools (several MCP servers', Home
  Assistant's), a session keeps its own in view and the rest behind
  `tools_search`, ranked by BM25 over their names, descriptions and
  inputs; what a search finds is loaded and called by name from the next
  call on (or at once through `tools_call`).
- **Programs that call tools**: `run_code` runs a Python program in the
  shell's sandbox, without network, that calls the session's tools that
  only read (`tools.read_file(path=…)`), each call checked as the model's
  own, up to 200 of them; only what it prints comes back. Trust atom
  `agent:code-mode`.
- **Hooks**: `sion agent hooks add --event before-tool|after-tool|run-start|run-end`
  runs a command of yours around calls and runs; one before a call may
  block it, with the reason given to the model, and one that fails blocks
  it. Trust atom `agent:hooks`.
- **Subagents in worktrees**: `agent_spawn` with `worktree` gives a
  subagent a git worktree of its parent's repository, its work committed
  on a branch of its own (`sion/<name>`) for the parent to merge. Trust
  atom `agent:worktrees`.
- **Turns told by what was said**: a live conversation ends your turn
  after half a second of quiet, unless what you said sounds unfinished
  ("Book a flight to…"), which waits a second and a half for the rest
  (`--turns semantic`, the default; `--turns silence` as before).
- **Voice on this machine**: `--hear whisper:<model file>` (whisper.cpp),
  `--speak piper:<model file>` (Piper), and `local:` for a server that
  speaks OpenAI's API (speaches, Kokoro-FastAPI, LocalAI), on every
  messenger, in `sion agent talk` and in meetings; any of them can hear a
  wake word.
- **Joining a meeting by its link**: `sion agent meeting --join <link>`
  joins as a guest that listens (Chromium with a silent microphone and a
  black camera), gives its name (`--name`), asks to join and waits to be
  let in, and hears the meeting through a PulseAudio sink of its own.
  Trust atom `agent:meeting-guest`.
- **Who said what**: `--diarize openai:gpt-4o-transcribe-diarize` (or
  Deepgram's, or ElevenLabs') records the meeting and has it heard again
  by a transcriber that tells speakers apart; the notes are written from
  the transcript by speaker. `--keep-recording` keeps the recordings.
- **A browser signed in as you**: `sion agent browser login <name>` opens
  Chromium for you to sign in; `--browser --browser-profile <name>` gives
  that profile to an agent's browser, one browser at a time, its cookies
  kept. Trust atom `agent:signed-in-browser`.
- **The page handed to you**: `browser_ask_person` hands the browser's page
  to you, on a page of this machine's known by a token, for a captcha, a
  code sent to you or a sign-in; the agent gets your reply and the page.
  A page that shows a captcha says so. Trust atom `agent:browser-handover`.
- **Where the tree says too little**: `browser_screenshot` with `marks`
  boxes and names the tree's references on the picture;
  `browser_click_at` clicks at a point of it and `browser_type_focused`
  types into what has the focus.

### Coming over, teaching, and talking

- **A model for each task**: `sion agent routes` gives a profile routes
  that choose its models for each task: pictures, work items and long
  tasks by rule, other tasks sorted by a small model among routes that say
  what they are for; what each route was given is logged, and its models
  are compared on a set of tasks (`routes eval`).
- **Skills from registries**: `sion skill search` lists ClawHub's and
  skills.sh's skills, and `sion skill import` reads one from ClawHub
  (`clawhub:owner/slug[@version]`), skills.sh or a site's well-known index,
  checked against the digests the registry lists, with the registry's
  verdicts shown, audited for 24 patterns (credential paths, persistence,
  words aimed at the model, hidden characters, programs and archives),
  the worst holding an install back until `--accept-risks`; hosts and
  secrets granted with `--grant-network` and `--grant-secret`.
- **Coming from OpenClaw or Hermes**: `sion agent import` brings an
  OpenClaw or Hermes Agent over to a profile: its persona and memories
  into the profile's instructions, its jobs as schedules, and with
  `--models`, `--keys` and `--bridges` its models, model keys and chats;
  a plan is shown first, `--apply` does it.
- **Teach a task**: `sion skill teach` records you doing a task once in a
  browser window (never a password, a secret field or a login's user
  name; at most ten minutes) and writes it down as a draft skill for the
  agent's browser, installed through the import's audit;
  `--write-with <model>` has a model generalize it.
- **X beyond direct messages**: `sion agent x-mentions` answers the posts
  that mention the account, in their threads, from the posts they reply
  to and the web search's results for their words, citing them, with
  their addresses in the last post; an answer that rests on nothing it was
  given is not posted. `x_search` searches recent posts. X's rotating
  refresh tokens are kept sealed across restarts.
- **Better live voice**: speech is told from other sound by RNNoise's
  network (`--detector`); the reply's echo from loudspeakers is taken out
  of what the microphone hears (raw samples played at their pace, the
  delay found by correlation, a 64 ms adaptive filter; `--no-echo-cancel`,
  `--play-raw`); `--realtime openai:<model>` talks through a realtime
  speech model whose one tool asks the agent.
- **Sessions that answer strangers** (X's mentions, a meeting's notes)
  run with the profile's models and instructions only: no memory, skills,
  files or tools. A tainted session's skill writes now wait for a person
  in every channel, since a skill reaches every later session's
  instructions.
- **Meeting notes**: `sion agent meeting` dials in to a meeting through
  Twilio (its keys pressed, its transcription posted back, signed) or
  hears it on this machine (the microphone, and what it plays), writes
  the transcript as it goes, and has the profile's agent write the notes:
  decisions, action items, open questions.

### What agents reach, and where they run

- **Web search**: `web_search`, through Brave Search, Tavily or a SearXNG
  instance of your own (`SION_SEARCH`, `BRAVE_SEARCH_API_KEY`,
  `TAVILY_API_KEY`), for sessions with `--web`; its results taint the
  session.
- **Tools to change code**: `edit_file` (a quoted stretch replaced, with
  the nearest lines shown when it is not found), `apply_patch` (unified
  diffs and `*** Begin Patch`, all or nothing), `find_files`,
  `search_files`, `read_file` by line ranges; with a shell, `diagnostics`
  and `run_tests` run the project's checker and tests in the sandbox.
- **The browser** keeps tabs, downloads into the workspace and chooses
  files from it for a page's file field.
- **MCP**: servers' resources and prompts (`sion agent mcp resources`,
  `prompts`; `/prompts` and `/prompt` in chats and at the terminal), a
  person's input asked in the conversation (forms and addresses; a form
  asking for a secret is declined), and sampling within limits the
  operator sets (`sion agent mcp sampling`), in both protocol eras.
- **sion's own MCP servers** for mail (IMAP and SMTP), calendars
  (CalDAV) and a folder of documents (Word, OpenDocument, PowerPoint, PDF,
  HTML, text): `sion mcp mail|calendar|documents`, added with `sion agent
  mcp add <name> --first-party …`.
- **Pull requests followed**: review comments on their code reach the
  work item and are answered in their threads; failed checks reach it from
  the check's app; tools read a pull request, its review comments, its
  checks and a job's log; `sion agent work run --listen` takes GitHub's
  webhooks, signed with `GITHUB_WEBHOOK_SECRET`.
- **Skills from anywhere**: `sion skill import` reads an Agent Skill from a
  folder or a git repository, reports what it holds, and installs it only
  on your review of its bundle root or an audit of it on a bond; `sion
  skill starter` installs seven starter skills.
- **On a server**: `sion agent up` runs the chain, the bridges, the work
  runner and the scheduler in one process and starts each again when it
  stops; `sion deploy user@host` puts sion and your agents on a server
  over SSH and runs them as a service there; `deploy/compose.yaml` and a
  container image built by the release workflow.
- **Live voice**: `sion agent talk --voice --live` listens all the time,
  speaks the reply a sentence at a time as it is written, and stops when
  spoken over; `--wake "hey sion"` takes only what begins with the phrase,
  heard first on this machine. Simplified: speech is found by loudness,
  and there is no echo cancellation.

### Work it takes on by itself

- **`sion agent work`**: a profile's agent watches GitHub repositories
  and chats (`watch <profile> github:<owner>/<repo> --allow <login>`, or
  `<messenger>:<chat>`) and takes work on there. What someone on the
  list asks it is taken on at once. What nobody asked for (a bug report
  not addressed to it, a listed bot's message) passes a fixed gate, is
  weighed by a model against the operator's rules (`rules <profile>
  --rule <id>=<instruction>`), and in `observe` mode, the default, is
  only recorded (`decisions <profile> --would`) until `--mode auto` lets
  it act. `run <profile>` is the runner; `list`, `show`, `answer`,
  `approve`, `stop` and `retry` work on its items.
- **A mandate for every piece of work**: who asked, under which version
  of the rules, by which path and under which rules. The agent's wallet
  publishes its digest as a claim (`work.mandate/v1`); the item's
  payments name that claim as their evidence; before every run it is
  checked against the rules as they are. A mandate the rules no longer
  allow is revoked, and one a person stops (`stop` on the thread, or
  `sion agent work stop`) is withdrawn, each on the ledger.
- **Attention spent as a cost**: the agent says once that it took the
  work on, says nothing while it works, and writes again with its
  result, with one question at a time (`work_ask`, with choices), or
  with why it cannot go on (`work_report`), in the conversation the work
  came from: at most 100 words in a chat and 400 on an issue, every
  message with a stated purpose, the limits held by the runtime.
- **`sion agent waiting`**: one list of everything that waits for a
  person, with the command that answers each: work items' questions,
  approvals and failures, messages that could not be sent, sessions
  waiting for approval, chats waiting to be let in.
- **A slot kept for people**: items run in slots (`--slots`, two by
  default), people's first, and one more is kept for a person's request
  or answer, so work nobody asked for never keeps anyone waiting.
- The design of the gate, observe mode, mandates checked again and the
  kept slot follows DeepSeek's dsh-superagent prototype; here fixed
  rules route the work, and every item runs as an ordinary session.

### The website and the book

- **How it compares**: a table on the landing page sets Siontology beside
  Hermes Agent, OpenClaw, Grok Bot and ChatGPT dots, from each one's own
  documentation and announcements as of 2 November 2026, with sources. Four
  rows are Siontology's alone (spending limits a ledger enforces, memory a
  rewrite cannot hide, hiring paid only for a result that passes a rule
  set in advance, a bond behind every skill from someone else); the rest
  include where the others are ahead, its status among them.
- **The website leads with your agent, and a team of them**: its first
  figure, an illustration of a graph of agents, is a lead agent handing the
  parts of a job to three others, which work side by side and pass their
  answers on. Six features follow, each with a headline, a line of plain
  words and a short run of its own: where it talks, what it remembers,
  what it does on a schedule, the helpers it brings in, its browser and
  its sandbox. A grid of example roles (a CMO, a CTO, a CFO, a head of
  sales, an artist manager, a biology researcher, a mental health
  companion, no substitute for a clinician) shows each as an agent with a
  face that blinks, looks about and follows the pointer, with one button
  to pause them all; with reduced motion they start paused. The brand
  guidelines record these faces as the one kind of motion that explains
  no mechanism. The payments figure moves to a section on the limits, and
  the transcripts to where each path starts.
- **The website shows the agent at work**: its first figure is a chat in
  which the agent asks before it pays and the ledger refuses a payment a
  page slipped in, whatever you approved; a figure in six parts follows,
  one button for each (its apps, its sandbox, its memory, its schedule,
  its helpers, its skills), every part a run of one mechanism with the
  numbers of a recording in the book. The messaging services and models
  it works with are listed by name.
- **The website leads with two doors**: run your own agent, or build on
  the ledger. A page of its own for the agent (`site/agents.html`): where
  it talks, what it asks, its tools, memory, skills, models, teams, the
  attacks tried and what it does not do yet, with an animated figure of a
  payment it is asked to make and one an injected page asks for. The
  developers page adds the agent's HTTP, OpenAI-compatible and A2A
  endpoints, every demo and the main commands; the security page, what an agent's
  limits rest on; the technology page, agent identities, memory logs, Task
  Charters and storage deals.
- **The book is restructured** around who reads it: getting started, your
  agent, build on the ledger, run a network, concepts, reference. The two
  long agent guides are split into focused pages under `agent/`, and the
  old addresses redirect to them.

### One person, one machine

- **`sion setup`**: a chain of one's own (one validator as a systemd user
  service or launchd agent, or a printed command where there is no service
  manager), the person's wallet and the agent's with test dollars, the
  agent registered with a budget and a billion-Pulse lifetime, its memory
  log, a model checked to answer, a profile (`assistant`: workspace
  `~/sion-workspace`, the web, L5), the scheduler as a service (so that
  scheduled tasks run), and Telegram if asked. Every step is
  skipped when done; `--yes` asks nothing. What it made is in
  `$SION_HOME/setup.json`.
- **Bare `sion`** talks with that profile in a new conversation; **`sion
  -c`** continues the last one.
- **Kept credentials**: models' and speech services' keys and addresses
  sealed in `$SION_HOME/agents/credentials.bin` (`sion agent credential
  add|list|remove`), read when the environment has none; never given to
  skills' scripts. Trust atom `agent:kept-credentials`.
- **`local:<model>`**: a model server on this machine or its network that
  speaks OpenAI's API (Ollama, LM Studio, llama.cpp), at
  `SION_LOCAL_MODEL_URL`. A cloud provider's base URL may now be on this
  machine (a loopback address).
- **`sion update`**: the latest release installed in place of the running
  program once `SHA256SUMS` is signed by the release key it was built with
  (`scripts/release/allowed_signers`) and the archive matches it; a build
  with no key refuses. The release workflow (`.github/workflows/release.yml`)
  builds Linux and macOS archives on `v*` tags, signs the sums and
  publishes; the installer now takes the latest release when it is not in
  a checkout, through the GitHub CLI for a private repository. Trust atom
  `release:signing-key`. No release key or release exists yet.
- **An idle chain rests**: `sion devnet --idle-round-ms` makes a validator
  with nothing to order wait before its next round, and keep full pace
  after ordering a transaction until its Pulse closes. One validator at
  rest, measured on 2 cores: 98.8% of a core unpaced, 5.7% at 10 s (the
  chain `sion setup` makes); a transfer took 2.3 s.

### Reads side by side, what a session last saw kept, A2A pushes

- **Reads side by side**: calls that only read, one after another in a
  turn and each free to run without anyone, run at once; the results
  answer the calls in order.
- **The latest of what a session read or changed** (`Tool::resource`:
  `file:<path>` for the workspace's tools) is kept apart from the
  conversation, the 32 most recent, and a compaction's summary carries it
  after what lasts.
- **A2A push notifications**: webhooks per task (0.3's
  `tasks/pushNotificationConfig/set|get|list|delete`, 1.0's
  `CreateTaskPushNotificationConfig` and the rest, or a message's own
  config), each POSTed the task as its state changes, with the client's
  token and Bearer credentials; HTTPS and public addresses only unless
  `sion agent serve --a2a-push-private`. The card now says
  `pushNotifications: true`.
- **`tasks/resubscribe`** (`SubscribeToTask`) streams a task's state as it
  changes, until it is final.
- Trust atom `agent:a2a-webhooks`.

### Confidential content, signed decisions, imitated notes, attacks tried

- **Confidentiality label**: a session holds confidential content once
  it is told so (`--confidential`), once `read_file` reads a path named
  with `--confidential-file` (profiles take both), or once a subagent's
  report or a graph node's input comes from a session that held some;
  from then on its external calls and spending wait for a person, at any
  autonomy level, and allowances and skills' pre-approvals do not hold.
- **Signed decisions**: `sion agent continue --approve|--decline --sign
  <wallet>` signs the decision with a key made on this machine, over the
  session, the call's key and input digest and the decision as recorded;
  `sion agent decisions <session>` checks each signature.
- **Imitated notes**: the runtime's own bracketed notes in a tool's
  result or a graph node's input are marked as not the runtime's before
  the model sees them.
- **Attacks tried**: a test runs nine prompt-injection attacks on a
  scripted model that obeys everything it reads and checks what holds
  and what gets through (two do, as built); the runtime guide shows the
  table.
- Trust atoms `agent:confidential-label`, `agent:signed-decisions`,
  `agent:imitated-notes` and `agent:attack-tests`.

### Graphs: votes, quorums, replays, and runs that go on after a person

- **`copies`** runs a node side by side, each copy told which attempt it
  is; **`"kind": "vote"`** answers with the answer given most often,
  without a model; the **self-consistency** template puts three copies to
  a vote.
- **Joins wait** for all (as before), the first answer (`"wait":
  "first"`) or a quorum (`{"quorum": n}`), and then close.
- **`read_only`** gives a node only the tools that read; the
  **orchestrator** template's researchers have it, and a verification
  checks their findings before the answer is written, with one round back
  to the plan.
- **Pinned and replayed**: a run keeps its graph's digest and is refused
  if the graph changes under it; **`sion agent graph replay <run>
  [--graph <file>]`** runs the graph again on what the run recorded and
  says whether it routes it the same way.
- **`sion agent graph continue <run>`** goes on after nodes whose sessions
  waited for a person, once they were approved; a wave's answers are all
  kept when one of its nodes fails.
- Trust atoms `agent:graph-replay` and `agent:read-only-node`.

### Dreams tested before they are kept; a curated log only dreams write

- **`sion memory dream --eval <set>`** (`--eval-model`, `--eval-runs`):
  held-out tasks run with the core memory as it is and as it would be
  with the dream's notes in mind; the notes are recorded only if no case
  passes fewer times with them, and a case whose task the dream read is
  refused as not held out.
- **`--into <log> --curator <wallet>`**: the notes go into a curated log
  that only dreams write, its marker per source log; **`--shared-memory
  <log>`** on `sion agent run` and profiles keeps it in mind, read-only,
  after the agent's own core memory.
- **Confidence from provenance**: consolidated and dreamt notes were
  recorded at full confidence; they now get 700‰ from a trusted session
  (300‰ from content from outside), 50‰ for each further source up to
  three, 100‰ for a dream's quote, never above 950‰, shown as
  `confidence_permille`.
- **Skill drafts**: a failure seen in two or more trusted sessions a dream
  reads becomes a draft skill under `$SION_HOME/skills/drafts`, with a
  note of where it came from; nothing installs it.
- Trust atoms `agent:dream-test` and `agent:curated-log`.

### Graphs measured against one agent; runs judged step by step

- **`sion agent graph eval <profile> --set <file>`** (`sion_agent::compare`):
  one agent, one agent several times with its answers put to a vote
  (`--samples`), and each template or graph given, on a set of tasks with
  stand-in tools and checks: the same token budget for a case
  (`--budget-tokens`; a vote's samples split it, a graph's nodes share
  what is left wave by wave), each case run several times (`--runs`).
  Pass rates with 95% Wilson intervals, the passes of each repeat, tokens,
  and each graph's difference from one agent with Newcombe's interval.
  The comparison is kept per profile and set.
- **The router is held to what earned it**: `run --route` offers only the
  templates that beat both baselines in one of the profile's comparisons,
  and keeps the task to one agent without asking when none has;
  `--untested` lets it choose among all.
- **`sion agent judge <session> --model <model>`** (`sion_agent::judge`): a
  model reads a session's steps (each call, the start of what came back,
  the answer) after labelled examples, and says of each whether it moved
  the task on, stalled or did harm; `--calibrate` measures it on labelled
  steps it was not shown first. `graph eval --judge` judges every run.
- Trust atoms `agent:comparison` and `agent:judge`. The graphs guide is
  recorded again with the router's gate, a comparison and a judge.

### Checks fixed when a task is posted

- **`sion task post --check <dir>`** (and `task_post`'s `check`): a
  command and its files, committed with the description in the task's
  spec, so that they cannot change once the task is posted; kept on the
  poster's machine for the task's verifiers, and not handed to the
  worker, which sees the check's commitment (`task_status`). A task with
  a check must ask for an attestation.
- **`sion task verify <verifier> <task> <output>`**: runs the check the
  task fixed, refusing any other, in a sandbox with no network, in a
  fresh directory holding its files, the output read-only at `$OUTPUT`,
  and attests the output on the verifier's bond only if it passes.
- **`task_submit`** takes the attestations a result rests on, and for a
  task that needs them keeps the output for its verifiers rather than
  submit it without; **`task_release`** lets a worker give up a task it
  cannot complete, saying why: the escrow goes back, its bond untouched.
- **`sion agent run --verify-files <dir>`**: a check's own files, bound
  into the sandbox read-only at `/check` only while the check runs, so
  that the agent's commands neither see nor change them.
- Trust atom `agent:task-check`; `agent:verifier` names the check's own
  files.

### Dreaming, and cases an evolution never saw

- **`sion memory dream`**: the sessions of the episodes recorded since
  the last dream, each read whole by a reader of its own, four at a
  time. A note a reader proposes is kept only if its session says the
  words it quotes (12 characters at least), a session that read content
  from outside yields facts only, and what is left goes through
  consolidation's gates and is recorded as consolidation records it,
  citing the session's episode; `memory dreamt-through` marks how far it
  went (`agent:dream-quote`).
- **`sion skill evolve --held-out <file>`**: the version given and the
  best are scored on cases the evolution never saw, and the best is kept
  as a draft only if it passes every one that the version given passes.

### Graphs of agents

- **`sion agent graph`**: a profile's agent run as the nodes of a small
  graph, each node a session of its own, the nodes of a wave side by
  side: `run <profile> --task … --template fan-out|work-check|draft-review`,
  or `--graph <file>` for one of your own (nodes with instructions, and
  optionally tools, a model and a schema for their answer; edges taken
  always, when a check passes or fails, on a field's value, or once for
  each item of a list); `show <run>`; `templates`.
- **`--route`**: the profile's model is asked first whether the task
  needs one agent (the default, and what an answer it may not give is
  taken as) or one of the templates.
- A graph is checked before it runs (every node reachable, at most 16
  nodes, 12 parts to a fan-out, 24 node runs unless it says more, 100 at
  most), and its run kept a wave at a time under
  `$SION_HOME/agents/graphs`. A node whose session read content from
  outside taints the nodes its answer reaches.

### What a model is sent, and checked before it is done

- **Prompt caches counted.** The input tokens a provider read from its
  cache or wrote to it are read from every provider (Anthropic, OpenAI's
  two APIs, Gemini) and count as the input they are, in a run's budget
  and in `/usage` (Anthropic's had gone uncounted); `/usage` and traces
  tell them apart (`gen_ai.usage.cache_read.input_tokens`,
  `gen_ai.usage.cache_write.input_tokens`).
- **A system prompt that stays the same.** A session's system prompt
  keeps the agent's core memory and skills as they were when the session
  began, or at the first run after its conversation was compacted, so
  that a provider's cache of it holds; what a chat asks (`/system`) comes
  after them.
- **Old results cut before a summary.** Past 120,000 characters, the
  results longer than 400 characters of the calls before the last 12
  messages become a line each, the calls kept; only if that is not
  enough, or past 36 messages, is the middle summarised.
- **What lasts survives compaction.** A summary begins with what the
  session did that lasts, kept apart from the conversation: its last 40
  calls that changed something or reached outside, earlier ones counted
  by tool.
- **`--verify <command>`** (on `sion agent run`, with `--shell`): each
  time the agent says it is done, the command runs in its sandbox, in its
  workspace, and exit status 0 passes. A failed check is shown to the
  agent, which goes on; after three (`verify_attempts`) the run ends
  unverified with its best answer (by the `score: <0 to 1>` the check
  printed last, if any). With a check, the agent is given
  `cannot_complete`, which ends the run as not done, saying why
  (`agent:verifier`).

### Content from outside, read in quarantine

- **`--quarantine`** (on `sion agent run` and on profiles): what tools
  return from outside is held out of the conversation, under a handle. The
  agent reads it with `quarantine_read`, giving a question and a JSON
  schema for the answer; a separate model call, given no tools and told
  that the content is data, answers, and the runtime checks the answer
  against the schema before the agent sees it.
- An answer of booleans, numbers, strings from an enum and strings of a
  strict format (a date, a time, an email address, a URL, a UUID, hex
  digits, a version) does not taint the session; one with free text does.
- The strings an answer gives are labelled as from outside: an external
  call or a payment whose input carries one waits for a person, whatever
  the level, a skill or an allowance says, and is approved on its own
  (`--always` is refused) (`agent:quarantine`).
- A browser's pages, a desktop's screens and the shell's output are not
  held, since the agent acts on what they show; they taint as before.

### Decisions that name what they decide; runs held to a token budget and out of circles

- **A waiting call names its request**: the first eight hex digits of its
  key, the same for as long as it waits. A decision may give it
  (`/approve 3f2a9c1e`, `sion agent continue … --approve --request
  3f2a9c1e`), and is refused if another call waits by then, so that a
  late answer cannot decide a call the person never saw.
- **Decisions kept with who made them** (`State::decisions`): as the
  channel names them (`slack:U07`, `http:token 3046d92e`, `cli:<user>`),
  when, and what else they allowed; a run shows `(approved by …)`.
- **Approvals for the rest of a session.** `/approve always`
  (`--always`): the same call again; `/approve always <rule>` (`--always
  <rule>`), a rule as skills' `allowed-tools` write them
  (`write_file(notes:*)`, `shell(git status:*)`): the calls of its tool
  that the rule matches run without asking, marked as allowed and by whom.
  Never a payment, which is approved on its own; never while the session
  has read content from outside; never past what the autonomy level
  refuses. `/allowed` lists them, `/allowed off` withdraws them
  (`agent:session-allowance`).
- **Going round in circles.** A call that gives the same result three
  times in a row, or two that take turns with the same results for three
  rounds: the model is told so, with the last result; two more, and the
  run stops (`Ended::Stuck`). `Limits::stuck_after` sets the count (0
  turns it off).
- **Token budgets** (`--budget-tokens`, on `sion agent run` and on
  profiles): the tokens a run may use, its subagents' included. Before
  each model call, what it sends (estimated: a token for every three
  bytes) and the most it may answer must fit in what is left, besides a
  reserve of 2,000 tokens; when they do not, the model is asked, within
  what is left, to say what it did and what remains, and a call it makes
  then is not made (`Ended::OutOfBudget`). The subagents a turn spawns
  share what is left, the reserve kept back, and say what they used
  (`agent:token-estimate`). `/usage` shows subagents' tokens and the
  budget.
- **More is content from outside.** A message from another agent over
  A2A taints its session; so does a task's output that `task_status`
  reads (another agent's work); and the subagents of a tainted session
  start tainted, since the tasks it writes them may carry what it read.

### Effects that cannot happen twice by accident; traces for OpenTelemetry

- **Nothing lost in a crash.** A call that changes something is written
  down before it runs, and the session kept on disk then, after each note
  the call takes and after it ends. A session resumed after a crash finds
  the call unknown, not forgotten, and the turn it was in is answered from
  the journal.
- **Each tool says how it keeps its effect from happening twice**
  (`sion_agent::Dedup`): a repeat changes nothing, its service drops a
  repeated key, what it did is reconciled from what it noted, or nothing
  does (a person settles it). A completed call is never repeated; a call
  whose outcome is unknown is repeated only by a tool that keeps it from
  happening twice.
- **Payments reconciled from the ledger.** `ledger_pay` notes its
  transaction's digest and last Pulse before sending it; if its outcome
  is lost, the ledger says what it did: executed (not paid again), past
  its last Pulse (paid again), or pending (it waits).
- **`sion agent settle <session>`** lists the calls whose outcome is
  unknown and settles one, done or not done.
- **`sion agent trace --otel`**: a session's steps (each model asked, each
  call, their times, tokens and outcomes) as OpenTelemetry spans, named
  after its generative-AI conventions (still at their Development stage).
  The batched path (subagents) now runs tainted sessions' calls as
  tainted.

### More channels: WeCom, QQ, WeChat, Zalo, X, Synology Chat, SimpleX, Nostr, ntfy and phone calls

- **WeCom** (`--via wecom`): a self-built app's callbacks, encrypted and
  signed as WeCom's `WXBizMsgCrypt` has it (AES-256-CBC under the
  `EncodingAESKey`, a SHA-1 signature over the token, time, nonce and
  ciphertext), checked and decrypted before anything reaches the profile;
  replies through the app's API, pictures, voice messages and files both
  ways through its media API.
- **Feishu's Encrypt Key.** With `FEISHU_ENCRYPT_KEY`, encrypted events are
  read (AES-256-CBC under the key's SHA-256) and their signatures
  (`X-Lark-Signature`) checked; pictures and files people send are
  fetched.
- **QQ** (`--via qq`): the bot platform's webhooks, signed with Ed25519
  under the key both sides derive from the bot's secret; direct messages,
  groups that mention the bot and guild channels; replies as QQ's passive
  ones, numbered.
- **WeChat** (`--via weixin`): Tencent's iLink bot protocol, the one its
  ClawBot plugin speaks. `sion agent weixin login` draws a QR code in the
  terminal to scan with WeChat; the sign-in is kept sealed beside the
  secrets. Long polled; each reply carries the context of what it answers.
- **Zalo** (`--via zalo`): the Zalo Bot Platform's API, long polled, typing
  shown, pictures and voice messages fetched.
- **X** (`--via x`): an account's direct messages through the X API v2,
  polled once a minute; the token renewed from its refresh token when X
  refuses it.
- **Synology Chat** (`--via synology-chat`): a chatbot's webhooks, carrying
  its token, answered through its incoming URL.
- **SimpleX** (`--via simplex`): through the WebSocket API of SimpleX's
  terminal client; contacts and groups that mention the bot; a reply shown
  as it is written, as a SimpleX live message.
- **Nostr** (`--via nostr`): NIP-17's private messages (NIP-44, sealed and
  gift-wrapped as NIP-59 has it) and NIP-04's encrypted ones, each answered
  the way it came. Every event's signature is checked, and a sealed
  message's sender; relays that ask are signed in to (NIP-42); a private
  reply goes to the relays its reader names (kind 10050), an older one to
  the relays they read (kind 10002). The cryptography is checked against
  NIP-44's and BIP-340's test vectors.
- **ntfy** (`--via ntfy`): each topic a chat, polled and published to,
  the bridge's own answers tagged and left out.
- **Phone calls** (`--via phone`): calls to a Twilio number, heard with
  `<Gather input="speech">` and answered with `<Say>`, each step a webhook
  signed with the account's auth token; only the numbers allowed are
  answered.
- These have been run only against stand-ins for their services. Discord's
  voice channels are not joined: Discord admits only clients that speak
  DAVE, its end-to-end encryption, which the bridge does not.

### Agents that talk A2A, and Home Assistant's devices

- **A2A.** `sion agent serve --a2a <profile>` gives other agents an
  Agent2Agent card for the profile and answers A2A's JSON-RPC at
  `/v1/profiles/<profile>/a2a`, both wire versions (0.3 and 1.0), streamed
  or whole. A message's `contextId` is a conversation; a task waiting for
  an approval is `input-required`. The card names no instructions of the
  operator's. Push notifications are not offered.
- **Home Assistant.** A profile made with `--home-assistant` has its home's
  entities and services as tools (`ha_states`, `ha_state`, `ha_services`,
  `ha_call`), through Home Assistant's REST API; calling a service is an
  external call (`agent:home-assistant`).

### Voice in the terminal, and more speech services

- **`sion agent talk <profile>`**: a conversation in the terminal, the reply
  as the model writes it and each tool on a line of its own, in a session
  like any channel's. With `--voice`, you are recorded (`--record`, sox's
  `rec` by default), heard (`--hear`), and answered aloud (`--speak`,
  played by `--play`).
- **Deepgram and ElevenLabs.** `--hear deepgram:<model>`, `--hear
  elevenlabs:<model>` (Scribe) and `--speak deepgram:<voice model>` (Aura),
  wherever voice is heard or spoken.

### Pictures and files, both ways

- **What a person sends reaches the agent.** A picture (PNG, JPEG, GIF or
  WebP, known by its bytes, 5 MB at most) is shown to the model with the
  words it came with; with a workspace, every file sent is put in its
  `inbox/` under a name not taken yet, and the agent told where (a file
  of more than 25 MB is not taken). Telegram's photos and documents,
  Slack's shared files, Discord's and Signal's attachments, Matrix's
  `m.image`, `m.video` and `m.file`, Mattermost's files, WhatsApp's
  pictures, videos, documents and stickers, LINE's pictures, videos and
  files, an MMS's pictures, email attachments (ten a message) and
  iMessage attachments, captions kept as the text.
- **What the agent shares reaches the person.** `share_file` sends a file
  of the workspace after the reply, as each messenger takes files: a
  photo or a document on Telegram, an upload in the thread on Slack and
  Mattermost, an attachment on Discord and Signal, an `m.image` or
  `m.file` on Matrix, a picture or a document on WhatsApp, an email of
  its own in the thread, a file through imsg. Where a messenger takes no
  files, the person is told.
- **Pictures made.** A profile made with `--image-model openai:<model>`
  (and a workspace) gives its agent `image_generate`: a picture made
  through OpenAI's Images API, shown to it and put in its workspace's
  `images/`, named as it asks.
- **An album seen whole.** The model is shown the pictures of the
  person's last message that has any, ten at most, besides the last three
  of the conversation, and a session keeps their bytes with the last
  six's (`sion_agent::model::person_pictures`).
- **Over HTTP.** A message to the HTTP channel takes `"files"` (base64,
  32 MB at most with them) and its answer gives what the agent shared;
  the OpenAI-compatible endpoint shows the model pictures sent inline
  (`data:` URLs), taking requests of 32 MB.
- An email's voice notes and files are held until they are answered,
  rather than until the next batch is read, now that a bridge reads on
  while it answers.

### Models: Gemini natively, keys in turn, the Responses API, Sign in with ChatGPT

- **Gemini.** `--model gemini:<model>` asks Google's Gemini API natively
  (`streamGenerateContent`), tools as JSON Schema, the thought signature
  of each call sent back with it.
- **Keys in turn.** A provider's key variable may hold several keys,
  separated by commas: each call starts with the next, and a key that is
  limited rests a minute, one refused an hour.
- **The Responses API.** `--model openai-responses:<model>`, streamed and
  not stored.
- **Sign in with ChatGPT.** `sion agent chatgpt login` signs in with
  OpenAI's sign-in for open-source apps (PKCE, the app registered on the
  machine's first sign-in, the ID token checked against OpenAI's keys and
  for its nonce), and `--model chatgpt:<model>` runs on the plan, its
  tokens kept encrypted and renewed; `status`, `models` and `logout`.
  Claude.ai plans are not offered: Anthropic does not let other apps sign
  in to them.

### Each chat its own; many profiles behind one bridge; streams through a gateway

- **Per chat.** `/system <text>` adds to what the agent is asked in this
  chat, `/tools <names>` keeps it to some of its profile's tools, and
  `/model` chooses among the profile's models: for the chat's sessions,
  set by those who may decide there.
- **Routes.** `--route <chat>=<profile>` has a chat answered by another
  profile than the bridge's, in sessions of its own.
- **Through a gateway, as it is written.** A message with `"stream":
  true` through an agent gateway is answered with the same events as the
  HTTP channel's, the host telling the gateway what the agent did every
  quarter of a second (`POST /v1/hosts/progress`).

### Bridges answer while the agent works, and keep their replies

- **Chats side by side.** A bridge answers each chat on a thread of its
  own, `--parallel` at once (four by default), one turn at a time in
  each; messages sent within `--debounce` milliseconds of each other
  (500) are one turn, a group's lines marked with who wrote them.
- **What is said while the agent works** is told to it before its next
  step (`--busy steer`, the default), answered after one at a time
  (`queue`) or together (`collect`), or stops it to be taken up
  (`interrupt`). `/stop` stops the agent at once on every channel: before
  its next model or tool call, and a model that is writing is cut off
  between two words, its connection closed and what it wrote kept
  (`Stop::Cut`; a stream's watcher returns whether to go on).
- **Chat commands.** `/retry`, `/undo`, `/compact`, `/usage`,
  `/sessions` and `/resume <id>` (this conversation's own sessions only),
  `/title`, `/model` (the profile's own models, for those who may decide),
  `/whoami` and `/help`, besides `/new`, `/status`, `/approve` and
  `/decline`. A second run in a conversation where one goes on is refused,
  not run beside it; each Slack or Mattermost thread and Telegram topic is
  a conversation of its own.
- **Marks.** Each message is marked taken, then answered or failed:
  reactions on Telegram, Slack, Discord and Mattermost, read receipts on
  Matrix, Signal and WhatsApp (`--no-reactions` turns them off).
- **Delivery.** A reply that fails in a way that may pass is tried three
  times; one the service refuses is not tried again. After five failures
  in a row a messenger's breaker opens for a minute, twice as long each
  time it opens again (ten minutes at most). What is not sent waits in an
  outbox kept with the bridge's state, and is sent when the messenger
  takes it again, after a restart too. A receive that fails is tried
  again with a growing pause; a chat whose answer a restart cut off is
  told so before it is answered again.
- JSON state files are written through a temporary file of their own, so
  that two bridges never write into one.

### Typing indicators, and replies as they are written

- **Models stream.** Anthropic's Messages API and OpenAI-compatible chat
  completions are asked for Server-Sent Events, and the text is told as
  each event brings it, while the answer is built from the same events
  into what a whole answer would have been. A stream that stops before
  its end, or reports an error partway, is a failure as its status would
  be (an overloaded provider is transient, and the next model is asked); a
  server that answers whole, or refuses `stream_options`, is read whole
  or asked again without it. A run can be watched as it goes
  (`sion_agent::Seen`): a session's run begins, a model is asked, the text
  it writes, the tools it calls and runs, a call that waits, an answer
  taken or dropped.
- **Typing, on every messenger that has it.** From the moment a bridge
  takes a message until it answers: Telegram's chat action, Discord's
  and Mattermost's typing, Matrix's (turned off at the end), Signal's,
  Teams' `typing` activities, WhatsApp's typing indicator (which marks
  the message read), LINE's loading animation, Twilio's for WhatsApp
  senders, and Slack's status line in a thread; where a platform has no
  typing for bots, a reaction stands for it and is taken off at the end
  (Slack outside a thread, Feishu's `Typing`, Nextcloud Talk). Each is
  shown again before it lapses, on the platform's own clock.
- **Replies as they are written.** Telegram's drafts in a private chat
  (`sendMessageDraft`, kept up past their 30 seconds), Slack's own stream
  in a thread with each tool a task on its timeline, Teams' streaming in a
  chat with one person, Feishu's streaming cards, DingTalk's AI cards
  given a template; elsewhere a message edited as it grows (Telegram
  groups, Slack, Discord, Matrix marked live, Mattermost, Google Chat),
  first sent with something worth a notification, and updated no oftener
  than each platform takes. `--stream progress|partial|off` and
  `--no-typing` choose.
- **Long replies are split, not cut,** where they read best, a code
  block closed and opened again across parts; replies go in the
  message's thread on Slack, Mattermost and Telegram's topics.
- **A reply is everything the agent said in its run,** each answer a
  paragraph, an answer cut off at its token limit joined to the next; on
  every channel and in `/status`.
- **The HTTP channel streams.** `POST /v1/profiles/<name>/messages` with
  `"stream": true` answers with `session`, `delta`, `tool` and `done`
  events; the OpenAI-compatible endpoint streams real deltas, its chunks'
  id naming the session.

### Bridges keep what they took; jobs reach every messenger

- **Nothing taken is lost.** A bridge keeps the messages it has taken
  from a messenger until each is answered, so those taken when it
  stopped are answered when it starts again (one being answered then may
  be answered twice).
- **Jobs' answers on any messenger.** `sion agent schedule add --deliver
  <messenger>:<chat>` sends a job's answer through Slack, Discord,
  Matrix, Signal, Mattermost, WhatsApp, LINE, SMS, email, iMessage,
  Feishu, Lark, Nextcloud Talk or Google Chat (with a service account),
  with the bridge's credentials; an email of the agent's own is titled
  as such.

### DingTalk, Feishu and Lark, Nextcloud Talk

- **DingTalk.** `sion agent bridge --via dingtalk` connects in Stream
  mode: a WebSocket of its own to DingTalk, opened with the app's
  credentials, every frame acknowledged, so there is no address to
  expose. Replies go through the message's session webhook, or after it
  expires through the robot's API; DingTalk's own transcription of a
  voice message is what the agent hears.
- **Feishu and Lark.** `--via feishu` (or `lark`) takes the app's events
  as webhooks with the subscription's verification token, answers the
  address's check, refuses encrypted events, and replies to each message
  through the Open Platform's API with the tenant token; voice messages
  are fetched to be heard.
- **Nextcloud Talk.** `--via nextcloud-talk` takes a bot's webhooks,
  signed with its secret by the server named, and replies through Talk's
  bot API, signed the same way.
- Trust atoms `agent:dingtalk-stream`, `agent:feishu-token` and
  `agent:talk-signatures`.

### iMessage

- **Through imsg.** `sion agent bridge --via imessage` runs `imsg rpc` on
  the Mac that runs Messages (`IMSG_COMMAND`, which can reach it through
  SSH) and subscribes to the messages after the bridge's place, so what
  came while it was stopped is answered; replies go to the chat, through
  iMessage or SMS (`IMSG_SERVICE`). Voice notes are heard as imsg
  converts them from Core Audio, and spoken answers sent as audio
  messages. Trust atom `agent:imessage-handles`.

### Microsoft Teams and Google Chat

- **Teams.** `sion agent bridge --via teams` takes the Bot Connector's
  webhooks, each with a token checked against the keys
  `login.botframework.com` publishes: issued for the bot's app ID, within
  its times, endorsed for the channel, and naming the service URL the
  reply goes to. Replies go through the Connector API with a token from
  Microsoft Entra ID (client credentials; a single-tenant bot's tenant
  with `TEAMS_TENANT_ID`), in the conversation and as a reply to the
  message.
- **Google Chat.** `--via google-chat` takes a Chat app's webhooks, in
  either shape Chat sends (an interaction event, a Workspace add-on's),
  with Chat's own token for the project number or a Google ID token for
  the endpoint's URL. The answer goes back in the webhook's answer, held
  open for it within Chat's thirty seconds, or, with a service account's
  key, through the Chat API in the message's thread, voice notes fetched.
- **RS256.** `sion-http` checks and makes RSA signatures with the TLS
  stack (`rsa_sha256_verify`, `rsa_sha256_sign`); `sion_cli::jwt` reads
  key sets, checks tokens and makes them. Trust atoms
  `agent:teams-tokens` and `agent:google-chat-tokens`.

### Email

- **A mailbox of its own.** `sion agent bridge --via email` reads the
  agent's mailbox over IMAP (waiting with IDLE where the server has it,
  marking nothing read) and answers over SMTP, each reply in the thread
  of what it answers, with a password or an OAuth token from a command
  (XOAUTH2). TLS, or STARTTLS, is required but to a loopback address,
  and what a server sends between agreeing to TLS and the handshake is
  refused; `sion-http` connects to IMAP and SMTP servers, and turns a
  connection to TLS when the protocol says (`Client::start_tls`).
- **Who wrote it.** A message is answered only when the provider's
  `Authentication-Results` show it is from the address its `From` names
  (DMARC, or DKIM or SPF aligned with the domain); machines' mail
  (out-of-office replies, bounces, lists, bulk) is not, and the replies
  say `Auto-Submitted: auto-replied`. Trust atom `agent:email-from`.
- **What it reads.** Plain text or HTML, quoted replies and signatures
  left out, the subject where a conversation starts; attachments are
  named, audio heard as voice notes, and a spoken answer sent as an
  attachment.

### Installing and running: the installer, an image, Homebrew, Nix, services, doctor

- **The installer.** `scripts/install.sh` installs `sion` from a
  checkout, from the repository (`--git`), or from a release archive
  checked against its `SHA256SUMS` and, with `--allowed-signers`, their
  SSH signature; `--dry-run` says what it would do. It never uses root.
- **Packages.** A `Dockerfile` (a small Debian image, a user of its own,
  the home a volume, the browser and desktop as a build argument), a
  Homebrew formula building the head (`packaging/homebrew/sion.rb`), and
  a Nix flake (`nix build`, `nix develop`).
- **Services.** `sion agent service install <name> -- <arguments>` keeps
  a bridge, the HTTP channel, the scheduler or a host running as a
  systemd user unit or a launchd agent, its tokens (`--env`) in a file
  only the user reads; `list`, `remove`, `--start`.
- **`sion doctor`.** What the machine has, and what is missing: the home,
  the node, bubblewrap and Docker, the browser and the desktop, and which
  credentials are set (by name).
- **No node needed** for the agent commands that keep only local state:
  pairing, secrets, logins, MCP servers' settings, schedules and
  services.

### Mattermost, IRC and Twitch

- **Mattermost.** `--via mattermost` receives over the server's WebSocket
  and posts through its API, with a bot's token; voice messages (audio
  files attached to posts) are heard, and spoken replies posted with the
  audio attached.
- **IRC and Twitch.** `--via irc` keeps a connection of its own (TLS for
  `ircs://`, `sion-http` now connects to IRC servers as it does to
  WebSockets), logs in with SASL when given a password, joins the
  `#channels` it answers, and answers pings; `--via twitch` is Twitch's
  chat, with its OAuth token. Replies go out as lines, a few at most, no
  faster than the networks allow; what was queued when the bridge stops
  is sent before it quits.

### OpenAI's chat API on the HTTP channel

- **For apps made for OpenAI.** `sion agent serve` also answers
  `POST /v1/chat/completions` (a profile as the model, `stream` as
  Server-Sent Events) and `GET /v1/models` (the profiles the token
  reaches), with the channel's tokens. A conversation the app sends
  whole goes on in its session, found by a digest of what was said
  before its last message; one not seen before starts a new session,
  told what was said.

### WhatsApp, LINE and SMS

- **Webhook messengers.** `sion agent bridge --via whatsapp|line|sms`
  listens for the service's webhooks on `--listen` (127.0.0.1:8797 by
  default) and checks each against the service's signature: WhatsApp's
  `X-Hub-Signature-256` with the app secret (and Meta's check of the
  address with the verify token), LINE's `X-Line-Signature` with the
  channel secret, Twilio's `X-Twilio-Signature` over the public address
  Twilio is given. Unsigned or wrongly signed webhooks are refused;
  messages sent again are answered once.
- **Answers.** WhatsApp through the Cloud API (`/messages`), LINE with the
  message's reply token while it lasts and a push after, SMS through
  Twilio's `Messages` API. Voice notes are heard on all three and spoken
  as an audio message on WhatsApp.

### Voice notes on every messenger

- **Heard everywhere.** Besides Telegram's, voice notes are heard from
  Discord's and Signal's audio attachments, Slack's audio clips (fetched
  with the bot's token, and only from Slack's own addresses) and
  Matrix's `m.audio` messages (authenticated media, or the older
  download path).
- **Spoken everywhere.** A voice note is answered aloud on Matrix as a
  voice message (MSC3245), and on Discord, Slack (its upload flow:
  `files.getUploadURLExternal`, the bytes, `files.completeUploadExternal`)
  and Signal as the reply with the audio attached.

### Live connections: Discord's gateway, Slack's Socket Mode, mentions

- **WebSocket.** `sion_http::ws`: RFC 6455 connections over the guarded
  client's own (`Client::connect`: the same address rules, roots and
  proxy, through a `CONNECT` tunnel), with the handshake checked, frames
  masked, pings answered, fragments joined and bounded, and waits that
  lose nothing of a frame half read; a server side for stand-ins. SHA-1,
  HMAC-SHA1 and HMAC-SHA256 for the protocols that name them, checked
  against their RFCs' vectors.
- **Discord through its gateway.** The bridge receives Discord's messages
  over the gateway (heartbeats, resuming, reconnecting with backoff; a
  refused token or a missing message-content intent ends it with why),
  in servers' channels and direct messages, instead of polling each
  channel. What is said while the bridge is not listening is not
  answered later.
- **Slack through Socket Mode.** With an app-level token in
  `SLACK_APP_TOKEN`, Slack's events come over Socket Mode, each envelope
  acknowledged at once and a message Slack sends twice (as a message and
  as a mention) answered once; without one, the bridge polls as before.
- **Pairing in direct chats, and mentions.** Pairing now applies to a
  person writing directly to the bot (a private Telegram chat, a Discord
  or Slack direct message, a Matrix room of two, Signal); a group chat
  not answered stays ignored, said once per chat. Discord, and Slack in
  Socket Mode, now pair too. `--require-mention` answers only what
  mentions the bot, or answers one of its messages, in the group chats a
  bridge answers.

### Pairing and voice notes on channels

- **Pairing.** `--pairing` on `sion agent telegram` and `sion agent bridge`
  gives someone a channel does not know a code instead of an answer; the
  operator lets their chat in with `sion agent pairing approve <code>`, on
  that channel and profile only (`sion agent pairing list`, `revoke`). A
  code is 8 consonants and digits, with no vowels (so it spells no word)
  and no look-alikes, from the operating system's randomness, sent only to
  the sender and never printed by the bridge. It
  lasts an hour; a sender is given one every ten minutes at most, a
  channel has at most three waiting, and five wrong approvals in a row
  stop approvals for an hour. The store is readable only by the
  operator's user and changed by one process at a time. `--allow` is no
  longer required with `--pairing`. On Matrix, pairing joins the rooms
  the account is invited to; Slack polled channel by channel (without
  Socket Mode) refuses it.
- **Voice notes in.** `--hear openai:<model>`, `groq:<model>` or
  `command:<program>` transcribes voice notes (Telegram's voice notes and
  audio files, Discord's audio attachments), which reach the profile as
  `(voice note) …`. A note that cannot be heard is answered with why.
- **Voice notes out.** `--speak openai:<model>:<voice>`,
  `elevenlabs:<voice id>[:<model>]` or `command:<program>` answers a voice
  note aloud on Telegram (`sendVoice`, the reply as its caption), and in
  text where the messenger sends no voice notes or speaking fails.

### A browser and a desktop of the agent's own

- **A browser.** `sion agent run --browser` (and profiles) gives a session
  Chromium or Chrome, with a profile made for the session, driven over
  the DevTools protocol on a pipe: `browser_open`, `browser_snapshot`,
  `browser_click`, `browser_type`, `browser_press`, `browser_scroll`,
  `browser_back` and `browser_screenshot`. A page is its accessibility
  tree, each element it can act on with a reference; frames of the page's
  own site are shown in it. On Linux it runs in bubblewrap, with the
  machine's managed browser policies; it reaches the network only through
  the egress proxy, to public addresses, or to the hosts `--browser-host`
  names. One page: what would open a window opens in it; a JavaScript
  dialog is dismissed and reported.
- **Where it acts on its own.** Pages taint the session, so by default its
  actions after a page wait for a person, as fetching does. Named hosts
  (`--browser-host docs.rs`, or `*`) hold it to those sites and let it act
  on their pages as the autonomy level allows (the new
  `Policy::contained`); everything else still waits.
- **Logins.** `sion agent login add|list|remove` keeps logins encrypted
  beside the secrets, each bound to an origin. `--login NAME` gives a
  session one, filled by `browser_fill_login` only into a field whose own
  frame is of that origin, and cut out of all the browser returns.
- **A desktop.** `--computer` gives a session an X server of its own
  (Xvfb, xdotool, ImageMagick) in bubblewrap or a Docker image, the
  programs `--computer-program` allows, and the `computer` tool with the
  actions computer-use models are trained on, each answered with a
  screenshot.
- **Pictures.** Tool results carry pictures: shown to Anthropic's models
  in the result and to OpenAI-compatible ones in a message after it, the
  last three at a time. A session keeps the bytes of the last six, and
  the conversation every picture's digest, so that letting one go changes
  nothing its trajectory digest covers.
- **Between runs.** A server keeps a waiting session's browser and
  desktop open; a session continued by a process of its own gets a
  browser that goes back to its page, keeping only the references that
  still name the same elements.
- **The egress proxy** sends each plain HTTP request on a connection of
  its own (`Connection: close`), so that a client's next request on the
  same connection is never acted on unchecked; its refusal names the
  hosts allowed, not a skill's.
- Trust atoms `agent:browser`, `agent:browser-hosts`, `agent:browser-logins`
  and `agent:desktop`.

### MCP servers over HTTP, with OAuth

- **Every transport.** `sion agent mcp add NAME --url https://…` reaches
  a server over Streamable HTTP as of 2026-07-28 (each request its own
  `POST`, its version, method and tool mirrored in headers, and the
  arguments a tool marks with `x-mcp-header` in `Mcp-Param-*`; a tool
  whose marks break the rules is left out), over Streamable HTTP of
  2025-03-26 to 2025-11-25 (the `initialize` handshake, a session in
  `Mcp-Session-Id` opened again when the server forgets it and closed with
  `DELETE`, requests of the server's own answered on the stream), and
  over the deprecated HTTP+SSE transport of 2024-11-05, whose stream is
  closed when its link is let go. Which one a server speaks is found as
  the specification says, what it spoke at its review first. Answers come
  as JSON or as events. `--header 'Name: value'` adds headers, `${VAR}`
  filled in where the agent runs.
- **OAuth as MCP's authorization says.** A server that answers `401` is
  logged in to at `add`, or with `sion agent mcp login`: its protected
  resource metadata (RFC 9728, from the challenge or the well-known URIs),
  which must name the server; its authorization server's metadata (RFC
  8414, then OpenID Connect discovery), which must name itself as issuer
  and offer PKCE with S256; a client registered beforehand
  (`--client-id`), named by a client ID metadata document
  (`--client-metadata-url`), or registered dynamically (RFC 7591); the
  person's consent in a browser that comes back to a loopback port, or,
  from a browser on another machine, the address it was sent to pasted at
  the terminal; the `state` and the issuer (RFC 9207) checked in the
  answer, PKCE and `resource` (RFC 8707) at the token endpoint. Discovery
  and tokens go over HTTPS at public addresses, unless the MCP server is
  on this machine (plain HTTP there) or at a private address itself (that
  network's too).
- **Tokens kept, renewed and revoked.** Tokens are kept encrypted beside
  the secrets, under their key. An access token is renewed with its
  refresh token (kept rotated) without the person; a `403` asking for
  more scopes is met where no person is needed, and otherwise says how to
  log in with them. `--client-credentials` authorizes a client of its own
  (the client credentials grant), for agents that run unattended, its
  secret left in its environment variable. `sion agent mcp logout`, and
  `remove`, revoke the tokens where the authorization server takes
  revocations (RFC 7009), then forget them.
- Checked against the official MCP Python SDK (2.3) and its OAuth
  authorization server, and against a stand-in for each era.
- `sion-http` sends any method and reads any status; `sha256` and
  `random` come from the TLS provider already compiled in, so no crate is
  added.
- Trust atoms `agent:mcp-remote` and `agent:mcp-oauth`.

### What a skill may do, enforced

- **Pre-approvals.** A skill's `allowed-tools` (Agent Skills) names the
  calls it pre-approves, in this runtime's tool names or other runtimes'
  (`Bash(git log:*)`, `Write(notes/*)`, `WebFetch(domain:docs.rs)`).
  Once a session has read the skill, a call one of its rules matches runs
  where the autonomy level would ask, and the model is told it ran
  without asking. A refusal stays a refusal, spending is always asked, a
  session that has read content from outside asks again, and a shell
  command with `;`, `|`, `&`, redirections or substitutions is never
  covered by a prefix. `--no-skill-approvals` (on `sion agent run` and
  profiles) turns them off.
- **Where a skill's scripts connect.** `sion-network` in a skill's
  metadata names hosts (`api.github.com *.example.org:8443`), and the
  scripts reach those alone: the sandbox gets no network of its own, a
  bridge inside it passes connections over a Unix socket to an egress
  proxy outside, and the proxy takes `CONNECT` and plain HTTP only to a
  declared host and port, at a public address, chaining through the
  runtime's own proxy when it has one. A plain request whose `Host`
  header names another site is refused. The connections asked for are
  shown with the script's output. The new `--sandbox-network declared`
  gives commands no network and skills' scripts only what their skills
  declare; `true` there means any public host.
- **Secrets.** `sion agent secret add|list|remove` keeps secrets in a
  store encrypted with ChaCha20-Poly1305 under a key only the operator's
  user can read; a value is typed without echo, piped, or taken with
  `--from-env`. A skill names the ones its scripts receive in
  `sion-secrets`; they arrive as environment variables, and their values
  are cut out of what the scripts print before the model sees it. Names
  the sandbox needs (`PATH`, the proxy variables, `LD_…`) are refused.
- **`sion-timeout`** shortens how long a skill's script may run.
- **Consent at install.** `sion skill install` shows what a release
  declares before anything is signed and installs it only with
  `--accept-permissions`; `.source` records what was granted. A release
  and its audits commit to the permissions through the bundle root.
- **The agent cannot grant itself any of it.** A skill it writes keeps
  only a name, a description and instructions (rewriting one the
  operator placed drops what it declared, and says so), evolution keeps a
  skill's front matter as it was, and a workspace or MCP directory that
  overlaps the skills directory is refused, shell or not.
- **Paths that hold on every file system.** A bundle's paths must be
  portable ASCII and distinct when case is ignored, and evolution
  compares paths the same way, so that on macOS or Windows no file
  (`skill.md`, `scripts/Run.sh`) can take the place of another.
- Trust atoms `agent:skill-pre-approval`, `agent:declared-hosts` and
  `agent:secret-store`; `agent:sandbox-network` names the declared
  network.

### An agent's standing, checked as a light client

- **`sion agent status <agent>`** checks an agent's standing as a light
  client: its charter and the overlays of every charter of its lineage
  are proven against one certified header (`--chain` names the chain to
  verify against, `--min-pulse` refuses stale proofs), and its
  allotments are proven too, for its budget; a stopped agent's says
  nothing can be spent. A node that leaves out an ancestor's overlay,
  which could hide that ancestor's suspension, is caught. The status is
  worked out from the proven objects by one rule,
  `sion_exec::charter::agent_status`, which the node's agent query and
  `sion agent show` now use as well.
- **The app's Agents tab** lists the agents the device watches: those it
  backs, answers for or is, found among the Pulses the node keeps, and
  any added by id. Each one's standing is checked that way against the
  pinned chain, with its budget, and the device suspends, resumes or
  revokes (asked twice) the agents it oversees.

### Evolution beyond a skill's instructions

- **A skill's other files.** In `sion skill evolve`, a variant may add,
  replace or remove the skill's references, examples and templates as
  well as rewrite its instructions: at most 8 files a variant, each text
  of at most 32 KiB inside the skill. A script is never written,
  rewritten or removed: evaluation runs read a skill's files but never
  run its scripts, so a changed script could not be scored. A version's
  size, for the Pareto front and the best, is the bytes of all its
  files, and a version is new by the digest of all of them.
- **A profile's instructions.** `sion agent profile add --instructions`
  gives a profile its operator's instructions, which follow the fixed
  system prompt. `sion agent evolve <profile> --evals <set> --mutator
  <model>` evolves them on an evaluation set under the same gates (at
  most 4 KiB, new, no case lost that the parent passed), and `--apply`
  makes the best the profile's.

### Channels across machines

- **A profile's own tokens.** `sion agent profile token add <profile>`
  makes a token that reaches only that profile, kept only as its hash and
  naming the profile's agent (`--agent`, for a profile served elsewhere);
  with `--approve` it also decides what the profile's sessions wait on.
  `sion agent serve` takes them, with or without the machine's token.
- **Slack, Discord, Matrix and Signal.** `sion agent bridge <profile>
  --via slack|discord|matrix|signal|telegram --allow <chats>` answers a
  messenger's chats with a profile, all through one interface; with
  `--approvers`, only the people named may approve or decline. Telegram
  goes through the same bridge, and `sion agent telegram` takes
  `--approvers` too.
- **An agent gateway.** `sion agent gateway` answers people's messages for
  profiles served from other machines, with the profiles' tokens. A host,
  `sion agent host <gateway> --profile <name>`, connects out to it and
  proves for each profile that it holds the key of the agent the
  gateway's tokens name, signing the gateway's challenge under the
  account's policy on the ledger. The
  gateway passes an agent's messages only while the ledger has it in good
  standing, checked at most five seconds before.

### An agent's past: search, consolidation and archives

- **Searching past sessions.** An agent with a memory log has a
  `session_search` tool, and `sion agent sessions search <agent> <words>`
  does the same from the command line: BM25 over the messages of the
  agent's sessions on this machine. Each hit says how the session as stored
  stands against the trajectory digest its episode recorded in the memory
  log: `checked` once the episode is committed to the ledger, `changed` if
  the stored session was edited since, `compacted` if it was compacted
  since. Episodes now record how many messages their digest covers and the
  session's compactions.
- **Consolidation.** `sion memory consolidate <wallet> <log> --model …`: a
  model reads the episodes recorded since the last consolidation and the
  notes the log keeps, and proposes facts, procedures and preferences, each
  citing its episodes. Gates keep only grounded, new and small notes; each
  is a derived atom linked to its episodes, naming the model and the
  consolidator in its extraction trace, and a note it replaces is
  superseded by a revision, never deleted.
- **Outside content stays out of prompts.** What an agent records while
  its session is tainted, the episode of a session that was ever tainted,
  and a note consolidated from one are marked as prompt-injection risks:
  kept and recalled, never put into the agent's prompt as core memory. A
  search that finds a tainted session taints the session that searched.
  Tools see the taint through `Tool::call_tainted`.
- **Archives with storage providers.** `sion memory archive` encrypts every
  committed atom of a log for its writer (ChaCha20-Poly1305, under a key
  derived from the wallet's seed and the log), names the archive's storage
  root as the log's manifest on the ledger, and posts a storage deal for
  it; later commits carry the manifest forward. `sion memory restore` takes
  the archive from a file or from the provider that keeps it, and checks
  it against the ledger before it replaces the local store. An archive
  never holds the store's epoch keys, so forgetting holds for archives too.
- **Retrieval from providers.** `sion storage serve --listen` serves the
  data of the deals a provider keeps to their clients (`POST
  /v1/deals/<deal>/retrieve`, signed by the deal's client under its account
  policy on the ledger); `sion storage retrieve` fetches it and checks it
  against the deal's root.

### Attested tasks and an index of claims (specification v1.15)

- **Required attestations.** A task's acceptance rule can name verifiers
  who must each attest every result (`must_attest`) and how many
  attestations a result needs (`min_attestations`). Until v1.14 a worker
  could settle on its own bond alone if the bond was large enough, and a
  bounty could not ask for a particular verifier, such as an overseer.
- **Nobody attests their own result.** The ledger refuses an attestation
  by the result's author, and a task whose named worker is one of its
  verifiers. Until v1.14 only the commands checked this: in an open bounty,
  a named verifier could submit a result of its own with its own
  attestation.
- **An index of claims.** `IndexClaim` names an owner's latest claim under
  a key, such as a publisher's latest release of a skill, at an id anyone
  can compute, and moves only to later claims of the owner's own. A reader
  checks the entry with a state proof against a certified header, so no
  server can make one up; the `sion` commands refuse a proof from more
  than 16 Pulses before the latest certified Pulse they know of (one they
  verified earlier, the node's own report, or `--min-pulse`) as stale.
- `sion task post` takes `--must-attest` and `--min-attestations`, and
  `sion skill bounty` too (one attestation by default: a better version is
  verified by others); `sion skill publish` indexes the release under
  `skill:<name>`; `sion skill latest publisher/name` reads the index with
  its proof, and `sion skill install` takes `publisher/name`. The `tasks`
  demo's bounty requires reviewer-2's attestation.
- Both proof stacks implement v1.15, and the differential tests cover the
  new rules and every way they fail.

### Lineage ids, token buckets, overlays and cohorts (specification v1.14)

- **Lineage ids.** A spawned agent's id is `lineage_id(parent, index)`,
  the index from a counter in the parent's allotment that only the parent's
  agent writes, and every charter keeps its lineage: its tree's root and
  the path of indices down to it. Anyone holding those recomputes every
  ancestor's id by hashing. Depths run from 0 to 8, down from chains of up
  to 16 charters.
- **Token buckets.** Caveats carry a `burst` (what the agent may spend at
  once) and a `rate` (what refills it each Pulse) instead of a window
  limit and length. An agent starts with a full bucket, and spawning
  carves the children's burst, rate and starting tokens out of the
  parent's bucket, so a subtree whose root has `(σ, ρ)` pays at most `σ +
  ρ·τ` over any `τ + 1` consecutive Pulses: a property test checks it on
  random trees. With fixed windows, a subtree could pay twice its limit
  across a window's boundary.
- **Overlays.** What happens to an agent after it is created (revoked,
  suspended, narrowed) is kept in its overlay, an object at a hash of its
  id. A check reads the overlays of the agent and its ancestors, at most
  9, most of them proofs that none exists, instead of loading up to 16
  charters. `NarrowAgent` adds limits to a whole subtree with one write:
  allowed payees, a smaller largest payment, an earlier end, no spawning;
  only the backer may widen or lift them.
- **Cohorts.** `SpawnCohort` commits to up to 1,048,576 subagents in one
  transaction, as the root of a Merkle sum tree over their bursts and
  rates, carved from the parent's bucket; `MaterializeAgents` creates up
  to 128 of them at their lineage ids, each with a proof of its leaf, its
  budget paid from the parent's allotment then. A member costs nothing on
  the ledger until then.
- **Containment, from a review of v1.14.** An overseer acts only in good
  standing: one whose own charter, or a charter above it, is revoked or
  suspended can no longer suspend, narrow or revoke the agents below it.
  The highest suspender is recorded, so an overseer lower in the tree can
  no longer lift a suspension from higher up by having suspended first.
  An expired charter, or one under a narrowing that has ended, spawns and
  materializes nothing, and nothing is spawned already expired; before,
  an expired agent could move its budget into new subagents while its
  backer waited to reclaim it. A subagent that must cite evidence judges
  it by its parent's policy; before, a laxer policy passed delegation.
- `sion charter create`, `sion agent register` and `spawn` take `--burst`
  and `--rate` instead of `--max-window` and `--window`; new `sion agent
  cohort`, `materialize` and `narrow`; `sion agent show` reads the
  overlays and gives amounts in USD. The node answers `GET
  /v1/agents/{id}`: an agent's charter and the overlays of its lineage,
  each with its state proof, so a resolver can check its status against a
  certified header; its status and what the agent can spend are as of the
  next Pulse, expiry included.
- `sion bench --only lineage` measures cohorts, materialization and a
  payment at the deepest level.
- Both proof stacks implement v1.14, and the differential tests cover the
  new operations and the containment rules; the bucket property test
  covers delegation and checks every subtree, not only the whole tree.

### Commands, scripts and MCP servers for agents

- A sandbox for commands (`sion_agent::sandbox`): bubblewrap (new user,
  PID, IPC, UTS, cgroup and network namespaces, no capabilities, the
  system's programs read-only, the workspace the only place it can write)
  or a Docker image pinned by digest and never pulled during a call
  (read-only root, every capability dropped, no new privileges, bounded
  processes, memory and CPUs, run as the workspace's owner). No network
  unless the operator gives one: bubblewrap's `host` (unfiltered), or a
  Docker network the operator set up (Docker's `host` and `bridge` are
  refused). CPU, file-size, wall-clock and output limits.
- `sion agent run --shell [bwrap|docker:<image>@sha256:<digest>]` gives an
  agent a `shell` tool that runs `/bin/sh` commands in its workspace inside
  the sandbox, and `skill_run`, which runs a script that comes with one of
  its skills (the skill read-only at `/skill`); a skill's scripts get the
  network only if its front matter's `metadata` says `sion-network: "true"`
  and the sandbox has one. `--sandbox-network`, `--sandbox-read`,
  `--sandbox-timeout`. Profiles take the same flags.
- Tools from MCP servers (`sion_agent::mcp`): `sion agent mcp add <name>
  -- <command>` starts a server, lists its tools with what their calls are
  taken to do and pins each by a digest of its definition; `sion agent run
  --mcp <name>` offers only tools whose definitions still match, so a
  server cannot change what a tool tells the model between a review and a
  run (`sion agent mcp review` shows what changed). Effects come from the
  reviewed annotations (open-world tools are external and taint the
  session) or `--effect`. Servers get only the environment variables they
  are given, may run in the sandbox, and are spoken to in both eras of the
  protocol: 2026-07-28 (probed with `server/discover`) and the `initialize`
  handshake of 2025-11-25 and before. `sion mcp`, the ledger's own server,
  now speaks both too.
- The journal no longer answers repeats of calls meant to run again: shell
  commands, skills' scripts and file writes run every time
  (`Tool::journaled`). A repeat it does answer says so. A tool can declare
  a smaller effect for a given input (`Tool::effect_of`).
- A workspace may not hold `$SION_HOME` or lie inside it, and a shell's
  workspace may not hold the skills it runs. `read_file` refuses pipes and
  devices.
- New trust atoms `agent:sandbox`, `agent:sandbox-network`,
  `agent:shell-repeats`, `agent:workspace-content`, `agent:mcp-unsandboxed`,
  `agent:mcp-effects` and `agent:storage-from-account`. Docs: [Commands, scripts and MCP
  servers](docs/book/src/agent/tools.md).

### Storage deals and batched signing for agents

- `sion agent run --storage` gives an agent `storage_keep` (post a deal
  to have a workspace file kept and proven every round, from an https
  locator or for a provider on this machine), `storage_deals` and
  `storage_close`. The escrow comes from the agent's account, not its
  charter's budget (`agent:storage-from-account`).
- Tools may run the consecutive calls of one turn together
  (`Tool::batches`, `Tool::call_batch`), each after passing the policy by
  itself; an identical call still waits for the journal. `agent_spawn`
  does: the subagents a turn asks for are spawned in one `SpawnAgents`
  transaction, funded in one transfer and run at the same time.
- Keyless subagents' payments go through a batcher per signing wallet
  (`sion_cli::batcher`): those made within moments of each other (100 ms
  after the last, 500 ms at most) are signed as one batch, so three
  subagents paying at once need one signature, not three.

### Fixes

- The gateway answers submissions while its faucet waits. The faucet read
  its account and checked its last transaction with queries that wait on
  a validator (up to 15 seconds each), and signed on the gateway's own
  thread, so a busy validator held every submission behind it and clients
  saw "the gateway did not answer". The faucet now runs on a thread of its
  own; the gateway only hands it grants and submits what it signed.
- The app core's devnet tests take turns: three devnets at once on a
  2-core machine starved each other past the tests' deadlines.

### Batched authorization and keyless subagents (specification v1.13)

- One authorization can sign a batch of up to 256 transactions: the RFC 9162
  Merkle root over their digests, with its signer and size, signed in a
  context of its own (`sion/tx/v1/batch/<configuration digest>`). Each
  member carries the authorization, its position and its inclusion path.
  Its fee floor is the usual one less 80 % of the base fee (the part that
  pays for checking signatures), plus its share of that part, rounded up:
  0.000304 USD instead of 0.0011 USD on the devnet in a batch of 256. New
  types
  `sion_exec::tx::{TxBatch, BatchMember, BatchEnvelope}`, field
  `SignedTransaction::batch` (the encoding of every signed transaction
  changes, so data from earlier devnets does not load), module
  `sion_exec::auth`, and `Wallet::{draft, sign_batch}`.
- Keyless subagents: `Spawn::agent` becomes optional, and a subagent
  spawned without an account gets `H("sion/agent/keyless", id)`, which no
  key controls. It acts only as a member of batches its nearest keyed
  ancestor signs. `sion agent spawn --keyless [--gas <USD>]` spawns them
  and writes a `.keyless` wallet file per subagent that holds no secret,
  only its charter and the wallet that signs for it: every command that
  takes a wallet takes its name. `sion agent pay-batch` pays from many
  agents under one signature, or prints the batch for `POST /v1/batches`;
  like any command, it first prints what is signed, member by member.
  The runtime's `agent_spawn` now makes keyless subagents, so no seed is
  made or kept for a subagent.
- Forced intents may be batch members; an obligation records its
  `signer`, and a rotation of the signer's policy resolves it.
- Proof stack B implements v1.13 independently (`sion_spec::authorize`);
  the differential tests cover batches a sender signs for itself, keyless
  subagents at two depths and below a keyed subagent, every way a member
  is refused, forced intents and settlements on the member's floor.
- The node checks a batch's signatures once: validators remember a batch
  by its message with the policy and signatures compared byte for byte,
  the executor and both proof stacks too, so a batch of 256 costs one
  signature check, not 256. The gateway sends each shard committee a
  batch's members in one `LaneBatch` message, consensus groups carry each
  batch's authorization once, and `POST /v1/batches` takes a whole batch
  with its signatures once.
- `sion demo batch`: 64 keyless subagents pay in one batch. `sion bench
  --only authorization` measures checks and bytes signed alone and in
  batches, and a Pulse of agent payments both ways. New trust atoms
  `agent:keyless-signer` and `impl:batch-size-fee`. Docs: [Batched
  authorization](docs/book/src/concepts/batched-authorization.md).

### An agent runtime

- New crate `sion-agent`: runs a language model as an agent. Models: the
  Anthropic Messages API (version 2026-10-21, automatic prompt caching),
  any OpenAI-compatible API, and a scripted model; a busy model cools
  down for 30 seconds while the next is asked. Tools declare an effect
  (read, write, external, spend); autonomy levels L0 to L6 decide what
  runs, what waits for a person's approval (valid 15 minutes) and what is
  refused. Content from outside taints a session for 24 hours, during
  which external calls and spending need approval. A journal returns the
  recorded outcome of a repeated call and never repeats one whose outcome
  is unknown. Runs are bounded (12 turns, 4 more per turn of progress, at
  most 32), compacted when long, stopped by a kill switch, and pause and
  resume from disk. Sessions export as chat-format trajectories.
- `sion agent run | continue | sessions | trace | kill-switch`: an agent
  identity at work, with the ledger as tools (`ledger_balance`,
  `ledger_pay` under its charter, `ledger_transaction`), files in a
  workspace and public web pages. The ledger refuses a payment outside
  the charter whatever the runtime's level. New trust atoms
  `agent:local-policy`, `agent:provider-sees-context` and
  `agent:plain-sessions`. Guide: [Agents at
  work](docs/book/src/agent/index.md).
- Memory: `sion agent run --memory <log>|new` gives the agent its memory
  log and the tools `memory_remember`, `memory_recall` (proved complete
  against the ledger for a subject and a relation), `memory_search`,
  `memory_forget` (a retraction; nothing is deleted) and `memory_commit`.
  Its preferences, goals and procedures are in every prompt, and each
  finished session is recorded as an episode with its trajectory's digest.
- Skills in the Agent Skills format (`sion_agent::skills`): the agent's
  skills are listed in every prompt, read on use (`skill_read`,
  `skill_file`), and written or improved by the agent (`skill_write`),
  which keeps the versions it replaces; a skill's bundle is a canonical
  encoding of its files.
- Skills from others: `sion skill pack | audit | publish | show |
  install`. A release (`skill/v1`) is a claim on the publisher's bond or
  on audits of the same bundle (`skill.audit/v1`, each on its auditor's
  bond), which add up. Installing checks the bundle against the release's
  root and accepts the release with the agent's remaining budget at risk;
  a release whose backing does not cover it is refused before anything is
  signed. An installed skill cannot be rewritten by the agent, and one
  whose release is falsified is taken out of use before the next run;
  the agents that relied on it are compensated from the slashed bonds.
  New trust atom `agent:local-skill-checks`. Guide: [Skills from
  others](docs/book/src/agent/skills.md).
- Skills that get better: evaluation sets (`sion_agent::evals`: cases
  with stand-in tools and checks, fixed by the digest of their bytes) and
  evolution (`sion_agent::evolve`: a mutator model proposes variants for
  the cases a version fails; gates for purpose, size, novelty and
  regressions; a Pareto front). `sion skill eval` (with trajectories
  labelled by result), `evolve` (the best version kept as a draft),
  `vouch` (an evaluation set's author stands behind it on a bond,
  `skill.evals/v1`), `bounty` (a Task Charter for a better version, its
  spec, verifiers and required claims fixed first), `verify` (a named
  verifier runs the set and attests), `submit` (result and settlement),
  `publish --evidence` (the release rests on the settled result and
  records it) and `lineage`. New trust atom `agent:eval-runs`. Guide:
  [Skills that get better](docs/book/src/agent/skill-improvement.md).
- After a finished task with at least five successful tool calls and no
  skill written, `sion agent run` asks the agent whether to write down how
  it did it, with only its skill tools (`--no-reflect` to skip).
- Scripted models: a step can play only `when` the conversation contains
  a text, or `unless` it does; a resumed session's script goes on from the
  step it reached.
- Teams, schedules and channels. `sion agent run --delegate` adds
  `agent_spawn` (a subagent with a budget carved from the agent's, fee
  money from its account, and a session of its own with the models of
  `--subagent-model`, two levels deep at most), `task_post` (a Task
  Charter from the agent's budget), `task_status` and `task_submit` (a
  worker's result on its task bond, and the settlement). `sion agent
  profile` keeps how to run an agent under a name; `sion agent schedule`
  and `sion agent scheduler` run a profile's task once, every interval or
  on cron (`sion_agent::schedule`, UTC), delivering its answer to a file, a
  webhook or Telegram; `sion agent serve` (HTTP, bearer token) and
  `sion agent telegram` (long polling, allowed chats) carry conversations,
  each a session, with `/approve`, `/decline`, `/new` and `/status`. New
  trust atoms `agent:channel-token`, `agent:telegram-chats` and
  `agent:local-scheduler`. Guide: [Teams, schedules and
  channels](docs/book/src/agent/index.md).

### Fixes from the review of storage deals, pooled stake and the app

An independent review of storage deals and the app, and a review of the
staking incentives, found defects that were fixed before release ([Reviews and
fixes](docs/book/src/security/reviews.md)):

- **Storage challenges** (spec v1.12): a round's challenge now comes from
  the beacon of its first Pulse, fixed at acceptance, instead of the beacon
  of the previous proof's Pulse, which a provider working with a validator
  could time. The deferred phase records the beacon of every eighth Pulse
  in one of 12,500 beacon records (`sion_exec::beacons`); rounds start and
  last multiples of 8 Pulses; the longest round is 100,000 Pulses. Missed
  rounds forfeit `⌊bond × missed / rounds⌋`; open deals must ask for
  collateral.
- **The reporter's share** (spec v1.11) is of the offender's own stake and
  rewards only: a share of the pool's slashed tenth made reporting
  yourself profitable once your pool was about 190 times your stake.
- **Proof stack B** implements storage deals and beacon records; the
  storage differential tests run again.
- **The provider** proves first and fetches in the background with
  deadlines; refuses `file://` locators unless allowed and private
  addresses always; batches proofs by size; takes modest deals by default;
  writes copies only after acceptance and deletes them at the close. A
  refused submission releases its coins. New crate `sion-http`: bounded,
  guarded outbound HTTP with X25519MLKEM768 first.
- **The app** shows outcomes from proven receipts only, waits for the
  digest it computed, pays 64-digit accounts only, writes settings
  atomically and pins nothing on first use after a reset; test money is
  labelled test USD, and the wording is neutral.
- `GET /v1/deals?open=1`, from an index; `sion storage post` needs
  `--locator` or `--local`.

### Storage deals (specification v1.12)

- `PostDeal` escrows `price × rounds` for keeping data whose Merkle root
  the deal fixes (leaves of at most 4 KiB, RFC 9162 tree), for a named
  provider or for any (then with collateral). `AcceptDeal` posts the
  provider's collateral and creates a custody record the provider owns;
  rounds start on the next multiple of 8 Pulses. `ProveStorage` proves the
  round under way for many deals at once, owned-linear: for each, the
  leaves the beacon of the round's first Pulse picks, with their Merkle
  paths. `SettleDeal` pays the provider for proven rounds; `CloseDeal`
  refunds unproven rounds to the client, returns the collateral less
  `⌊bond × missed / rounds⌋`, or forfeits all of it to the treasury when
  the client closes a deal its provider abandoned for three rounds. The
  supply counts deal escrow and collateral. Both proof stacks implement it.
- `sion storage prepare | post | deals | show | accept | prove | settle |
  close | serve`; `serve` runs a provider that proves every round, collects
  and closes, and accepts deals whose data it fetched (`https://`, or
  `file://` when allowed) and checked against the root. MCP tools
  `sion_storage_post`, `sion_storage_deals` and `sion_storage_close`.
  `GET /v1/deals`; account views list custody records.
- The `storage` demo: a robot commits its memory log with its archive's
  root as the manifest and pays a laptop to keep the archive; a phone that
  kept only half of the camera footage it accepted cannot answer the
  challenges and forfeits its collateral.
- The app's Storage tab turns a device into a provider: it takes open deals
  on modest terms whose data it can fetch, keeps the data, proves every
  round (each proof expiring at the end of its round, so a late one costs
  nothing), collects and closes; turned off, it keeps proving the deals it
  holds.

### The app: a light node on every device

- New crate `sion-app-core`, shared by the app's desktop and mobile
  shells: the device's key, a light client that verifies the latest
  certified Pulse every eight seconds (chain identity pinned on first use
  or by its user, committee certificate, receipts, a state proof, 16 random
  data shares), receipts proven for every transaction the device signs,
  payments and pooled stake, over any node's HTTP API. `Send + Sync`, and
  no lock held while it talks to a node.
- A Tauri 2 shell in `apps/sion-app` (its own Cargo workspace) with four
  tabs (Node, Wallet, Stake, Settings) in the brand's type and colours.
  Built and run on Linux against a devnet; Android and iOS are configured
  but not built yet. See the [app page](docs/book/src/getting-started/app.md).
- `sion_cli::verify::verify_pulse_report` returns the chain identity and
  Pulse a verification checked, so a client pinning on first use pins the
  identity it verified against.

### Networking between machines

- Validators run one per process and talk QUIC: `sion keys validator`
  generates a validator's keys and a network record signed with both of its
  consensus keys, `sion genesis` collects records into a genesis file and a
  peer file, `sion node` runs a validator (with an optional embedded
  gateway) and `sion gateway` a gateway on its own. TLS 1.3 with
  X25519MLKEM768 only and ML-DSA-44 raw public keys only, pinned at both
  ends; one stream per message class, bounded queues, quotas (new crate
  `sion-net`, on quinn 0.11.9 and rustls with aws-lc-rs). Tested with four
  node processes and a gateway on one machine. A stopped validator cannot
  rejoin yet (`net:no-rejoin`); see the
  [guide](docs/book/src/guides/network.md).
- Every message between nodes now goes through a `Transport` seam that
  stamps the sender: the devnet's in-memory transport carries exactly what
  the old bus did, a seeded fault injector adds loss, delay, duplication,
  reordering and partitions per message class, and status and `sion bench`
  report traffic per class, kind and peer.
- Lost messages are asked for again: gateways retry late lane approvals,
  validators pull missing Pulse signatures, and the buffers peers can fill
  are bounded in bytes or count.

### Memory for agents and robots: the library, commands and demo

- New crate `sion-memory`, the off-ledger half of memory logs: integer-only
  memory atoms with space and two kinds of time, single-writer logs with an
  on-disk store, a canonical 4D index (hourly Merkle R-trees per frame, a
  long-lived tree, subject and link maps) specified for independent
  rebuilding, complete range proofs including empty answers, subject and
  link proofs, omission proofs that convict an index leaving an atom out,
  belief as a fold with proofs checked from heads, sealed payloads with
  forgetting by key destruction, and the Reality Memory Protocol. 84
  tests, including a second implementation of the index written from its
  specification. Over 10⁶ atoms, a 50 × 50 × 10 m, one-hour query returns
  9 atoms with a 36 KiB proof, verified in 0.2 ms.
- `sion memory open`, `remember`, `commit`, `recall` and `show`, and the
  MCP tools `sion_memory_open`, `sion_memory_remember`,
  `sion_memory_commit` and `sion_memory_recall`: recall answers are
  checked against the index root the ledger holds.
- The `robots` demo: two robots with their own agent identities and memory
  logs share a warehouse; a rewrite is refused; a curator's refutation
  gives two readers two provable beliefs; an omitting index is convicted; a
  camera frame is forgotten; a suspended robot's memory stops growing.

### Pooled stake (specification v1.11)

- `DelegateStake` adds stake to a validator's pool without handing it
  over: the delegator keeps a delegation object holding pool shares. It
  joins at the epoch's end, then counts towards the validator's weight and
  earns a share of its rewards. `Undelegate` pays what is owed and starts
  unbonding (the stake stays slashable), `WithdrawDelegation` returns the
  stake, and `ClaimDelegatorRewards` pays the rewards credited so far.
- At each epoch's end a validator's reward is split between its own stake
  and its pool's earning stake; the operator keeps a commission on the
  pool's part, set with `SetCommission`: at most 25 %, effective at the
  epoch's end, and, once anyone has delegated, raised by at most 2 points
  at a time. The rest is credited per share with an exact accumulator.
- A delegation is refused if it would give its validator more than twice
  an equal share of the bonded stake (never less than a tenth).
- An equivocation also slashes a tenth of the offender's pool, unbonding
  delegations included; the reporter's share is of the offender's own
  stake only (as first written, of both). Validators rank
  for the active set by their own plus their pool's earning stake.
- Both proof stacks implement v1.11. The differential tests run every rule
  and failure through both, check every epoch end's split against a third,
  plain-arithmetic reading, and run randomized workloads of delegations,
  claims, commission changes, fees and slashings, checking the pools
  against the delegations they hold after every Pulse.
- New `sion stake` commands (`pools`, `delegate`, `show`, `claim`,
  `undelegate`, `withdraw`, `commission`), MCP tools `sion_stake_pools`,
  `sion_stake_delegate`, `sion_stake_show`, `sion_stake_claim`,
  `sion_stake_undelegate` and `sion_stake_withdraw`, and the guide
  *Delegating stake*. The node's account view lists an account's
  delegations. The supply invariant counts pooled stake and unclaimed
  delegator rewards.

### Task Charters (specification v1.10)

- `PostTask` escrows money for a task and fixes, at posting, the rule a
  result must pass: the poster's policy, the claims every result must rest
  on (an environment's bonded "no known exploit" claim, say), the
  verifiers whose attestations count, the adjudicator, the largest
  detection factor and how long the poster relies on a paid result. An
  agent can post from its own budget, under its caveats.
- `SettleTask` pays the worker once, when a result claim that commits to
  the task, with exactly the formula the task requires (the worker's bond
  or a named verifier's attestation of the same output, and every required
  claim), passes the decision rule. It reserves κ times the payment of the
  result's backing for whoever paid, so if a required claim is later
  falsified (an exploit in the environment), that party is compensated
  from the slashed bonds and the challenger is paid a share.
- `CancelTask` refunds a task its worker releases, a bounty its poster
  withdraws, or one past its deadline. A named worker can count on its
  task until the deadline.
- Both proof stacks implement v1.10; the differential tests run every rule
  and failure, posting from an agent's budget, bounties, cancellation and
  an exploit's compensation. New `sion task` commands (`post`, `attest`,
  `submit`, `settle`, `cancel`, `show`), MCP tools `sion_task_post`,
  `sion_task_attest`, `sion_task_submit`, `sion_task_settle` and
  `sion_task_show`, and the `tasks` demo.
- The supply invariant counts open task escrow.

### Memory logs (specification v1.9)

- A memory log is a log of memory atoms that only its writer appends to,
  kept off the ledger. `OpenMemory` creates one, owned by the writer,
  optionally for an agent identity; `CommitMemory` moves its head to a new
  size and root along an RFC 9162 consistency proof that the ledger checks,
  so a log only grows and cannot fork. The Pulse of a commit is the
  transaction time of the atoms it adds.
- An agent's log stops growing while the agent, or any agent above it, is
  suspended or revoked.
- Both proof stacks implement v1.9; the differential tests run every rule
  and its failure cases, and a property test runs random commits, honest
  and forged, by two writers through both.
- The design note "Billions of agents" covers the memory layer's atoms,
  4D index, completeness proofs, belief and privacy.

### Agent identities (specification v1.8)

- An agent identity is a charter with an identity record: a commitment to
  the agent's card and its depth in the tree. The charter's id is the
  agent's id (`did:sion:<chain>:<id>`).
- `RegisterAgent` registers one. `SpawnAgents` launches up to 256
  subagents in one transaction, each with its own account, card, caveats
  at least as strict as its parent's, and a budget carved from the parent's
  allotment. `SetAgentCard` replaces a card.
- `SuspendAgent` and `ResumeAgent` pause and resume a charter and
  everything below it; any overseer (the backer, the principal or an
  ancestor's agent) may suspend, the suspender or the backer may resume.
  `RevokeCharter` may now be sent by the agent itself or any overseer.
- Window limits are carved like budgets, for `Delegate` too: allotments
  gain `carved_window`, and an agent's own limit is what it has not handed
  down. Until v1.7 each child could spend its parent's whole window limit,
  so many children multiplied the parent's rate. An independent design
  review found this.
- Both proof stacks implement v1.8; the differential tests run every
  operation and its failure cases through both. New `sion agent` commands
  (`register`, `spawn`, `show`, `card`, `card-template`, `suspend`,
  `resume`, `revoke`, `reclaim`) and the `swarm` demo.
- The wallet now puts an operation's extra inputs (an allotment, the
  account object) right after the gas coin, so `inputs[1]` holds them
  however many coins pay the fee.

### Continuous integration

- Pull requests and pushes run the fast suite: formatting, lints, the tests
  not marked `#[ignore]`, the independence check, and now also a check that
  the generated website pages match their sources and a build of the book.
- The demo runs (about three minutes on two cores) are `#[ignore]`d and run
  every night with every other test (`cargo test --workspace --
  --include-ignored`), next to the quick benchmarks, whose JSON is kept for
  90 days.
- The website deploys from `master` as well as `main`.

### Node fixes found by the networking review

- The gateway handed consensus every certificate of a 20 ms flush as one
  group, capped at 512 transactions but not in bytes. Consensus drops an
  item above `max_payload_bytes` (512 KiB) without a reply, and about 20
  dual-signed transfers fill it, so a burst never executed: the gateway
  re-sent the same oversized group every 4 s until it expired. Groups are
  now split by encoded size, at most half the payload limit
  (`PayloadItem::groups`), in both the flush and the re-send path. A
  transaction whose certificate cannot fit even alone is refused with a
  reason instead of retried forever. A devnet test sends 40 dual-signed
  transfers at once under a 128 KiB limit; it fails without the fix.
- Beacon reveals committed ahead of their Pulse lived only in the Pulse
  builder, so a validator restarting alone would compute a different beacon
  for up to 8 Pulses. Log records and snapshots now carry them with the
  beacon (storage format 4, `SIONWAL4` and `SIONSNP4`), taken as of each
  Pulse's own close: one commit can close two Pulses, and the first one's
  record used to hold the beacon after the second. A directory in the older
  format is refused with a clear error instead of being truncated as a torn
  tail. Pulse headers do not yet commit to the pending reveals; that needs a
  specification version.

### Payment streams (specification v1.6 and v1.7)

- Payment streams: a payer escrows funds for one payee and pays it off the
  ledger by revealing the elements of a SHA3-256 hash chain, one per
  payment. The payee checks a payment with one hash and settles many in one
  transaction (`OpenStream`, `SettleStream`, `TopUpStream`,
  `CloseStream`, §5.31 to §5.34). A settlement walks from the head, one
  hash per step, at most 65,536 steps, and pays 2 micro-USD per hash.
- Specification v1.7, checkpoints: a payer may commit to a Merkle tree over
  every `spacing`-th chain element, and the payee may settle by a jump to
  the checkpoint below its position, for at most `spacing + 21` hashes
  however many payments the settlement covers.
- Both proof stacks implement the rules; differential tests, a randomized
  property test and executor scenarios cover them.
- x402 gains a `stream` scheme: a request is paid with the next chain
  element, checked by the server with one hash, and settled in batches by
  the server's stream desk. A refused payment names the server's position
  so an honest client resynchronizes. The x402 client reuses HTTP
  connections.
- New commands `sion stream open|pay|jump|check|settle|top-up|close|show`;
  `sion x402 serve --streams` and `sion x402 fetch --stream --count`; the
  `streams` demo; the `streams` benchmark group.
- Measured on one core of a 2-core sandbox: a payee checks a payment in
  0.42 µs; settlements by checkpoint covering about 9 × 10⁸ payments a
  second executed with every signature checked. See the research note
  [Billions of payments a second](docs/book/src/research/billion-scale.md).
- New research notes in the book: the billion-payment note, a plan for
  succinct validity proofs and a plan for networking between machines.

### Review of the validator machinery (specification v1.5)

- An independent review of staking, rewards, slashing and committees
  found that an honest validator could be slashed: the node released a
  rejected transaction's lane locks at once, although its inputs kept
  their versions, and its next approval of those inputs was equivocation
  evidence until the first transaction expired. A rejected transaction now
  keeps its locks until it expires.
- Specification v1.5: slashed validators lose their seats at the epoch
  boundary even if nobody else is eligible. A chain whose every eligible
  validator was slashed halts instead of running on them.
- The simulator's default validator parameters match the devnet: 70 % of
  fees to validators, 5 % of a slashed stake to the reporter.
- New trust atom `impl:active-floor`: the active set can shrink to a single
  validator.

### Validators, staking and epochs (specification v1.3 and v1.4)

- The validator set is part of the state. A validator joins by staking,
  with a proof that it holds its keys; it leaves, and withdraws its stake
  after an unbonding period.
- Epochs. At each epoch's end, joining validators become active, leaving
  validators start unbonding, and the next active set is chosen by stake.
  Consensus is weighted by stake, and lane committees are sampled from the
  active set with the beacon. The World Proof covers every committee
  change.
- Light clients follow the committee from the genesis they pinned, with
  one boundary proof per epoch (`GET /v1/epochs/{e}`, `sion verify`).
- Equivocation, two consensus headers for one round or two live approvals
  of one object version, is slashed on signed evidence. The validator
  forfeits its stake and unclaimed rewards; the reporter gets a share (5 %
  on the devnet).
- Rewards come from fees only (v1.4). Fees are credited to the treasury,
  which keeps a running total of them. At each epoch's end, a share of the
  epoch's fees (70 % on the devnet) is paid to the epoch's validators by
  weight. Slashed stake and released bond pools go to the treasury and are
  never paid out as rewards.
- Forced inclusion answers censorship. A forced intent becomes an
  obligation of its sender's shard; if it could still execute at its
  deadline, it is overdue and the shard's committee is replaced by
  validators from outside it.
- Devnet: `--standby`, `--epoch-length` and `--unbonding`;
  `GET /v1/validators`; `sion demo staking`.

### Renamed to Siontology

- Crates are `sion-*`, the binary is `sion`, and the environment variables
  are `SION_NODE`, `SION_HOME` (default `~/.sion`) and `SION_CHAIN`.
- Hash domain tags start with `sion/`, so digests, identifiers and roots
  differ from earlier builds. The default devnet chain id is
  `sion-devnet-1`, the x402 network is `sion:<chain>`, and the MCP tools
  are named `sion_*`.
- "The Fold" remains the name of the proof-folding mechanism.

### Licence, documentation and community

- Licensed under Apache-2.0 or MIT, at your option (`LICENSE-APACHE`,
  `LICENSE-MIT`).
- The documentation book (`docs/book`), published at
  [siontology.org/docs](https://siontology.org/docs/).
- Contributing guide, security policy, code of conduct, governance,
  trademark policy, citation file, and issue and pull request templates.

## 2026-10-28

- **Validator messages are bound to the chain** (`de39a21`). Lane batches
  and consensus headers name the chain identity; certificates, approvals
  and equivocation evidence from another chain never count, even when its
  validators reuse the same keys.
- **x402** (`de39a21`). An HTTP API can charge per request with HTTP 402
  and settle on the chain (x402 version 2, `exact` scheme). `sion x402
  serve|fetch`, the MCP tool `sion_fetch_paid` and `sion demo x402`.
- **Forced inclusion** (`b0f5c55`). A user whose transactions are ignored
  posts them straight to consensus (`POST /v1/forced`, `sion send
  --forced`). Pulse headers commit to the pending and overdue counts, so
  censorship by more than a third of a committee is visible to light
  clients. `sion demo censorship`.
- **Specification v1.2** (`b0f5c55`), from two independent reviews.
  Transaction signatures are bound to the chain configuration; inputs,
  allotments, orders per batch, formula depth and certificate values are
  bounded; reservation splits are exact. The node and the light client
  received fixes from the same reviews.
- **Proof stack B and specification v1.1** (`0bbf3ef`). `sion-spec`
  implements the state transition from the specification alone. Every
  World Proof carries leaf proofs from both stacks and verifies only if
  both accept every chunk; `scripts/check-independence.sh` checks that
  stack B never calls the reference executor. The differential tests found
  defects in the reference executor, fixed by v1.1. Transactions and
  receipts read as sentences in the CLI.

## 2026-10-27

- **Chain identity** (`4bc64bb`). Pulse headers carry a digest of the
  genesis specification, the validators and the shard layout. A light
  client pins it and rebuilds every committee itself. Fixed: approvals
  from the second shard were attached to the first shard's batches.
- **Certified inputs and a binding World Proof** (`b4c17cf`). Every
  transaction, contended ones included, locks its owned inputs with a
  lane certificate. The World Proof carries the Pulse's published data and
  its lane certificates and checks both. `sion bench`; CI.
- **The state transition specification, v1.0** (`c12fc18`). Receipts
  commit to an outcome code; a dead charter's allotments can be reclaimed.
- **The CLI** (`0af09fe`): a devnet, wallets, every transaction kind over
  HTTP, light-client verification, the MCP server for agents, and the
  first scripted demos.
- **The devnet node, light client and prover** (`1431cd3`). Validators on
  their own threads with their own state; lane committees, certified-DAG
  consensus, Pulses, data availability, a write-ahead log with snapshots,
  and the HTTP API. The prover folds per-chunk proofs from each proof stack
  into a World Proof per Pulse.
- **Execution** (`9046b14`): the two-sorted state machine, coordination
  classes, the Pulse's phases, and stateless re-execution of every chunk
  from its witness.
- **Consensus** (`398cafb`): Linear Lanes, certified-DAG BFT, committees,
  the beacon, forced inclusion and a simulator.
- **Assurance, clearing and data availability** (`7fe243b`): the Assurance
  Algebra with cost-of-corruption intervals, an exact linear-programming
  solver, Verifiable Clearing, and 2D Reed-Solomon data availability with
  sampling.
- **Foundations** (`58fe022`): canonical hashing, Merkle structures and
  post-quantum identities.
