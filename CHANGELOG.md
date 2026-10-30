# Changelog

Notable changes to Siontology, newest first. There are no releases yet:
everything below is on the default branch, unreleased. Once releases begin,
they will follow [Semantic Versioning](https://semver.org/). The state
transition specification (`docs/spec/state-transition.md`) has its own
version, given below where it changed.

Commit messages before the rename use the working name, Fold.

## Unreleased

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
  servers](docs/book/src/guides/agent-tools.md).

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
  work](docs/book/src/guides/agent-runtime.md).
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
  others](docs/book/src/guides/skills.md).
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
  [Skills that get better](docs/book/src/guides/skill-improvement.md).
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
  channels](docs/book/src/guides/agent-teams.md).

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
