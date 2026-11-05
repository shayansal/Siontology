# Implementation status

What this repository implements of the architecture ("Neo-Blockchains: The
Fold Architecture"), what it simulates, and what it leaves out. Rule 5 of
`AGENTS.md` asks that every shortcut be named; this file is where the names
live.

**Status words**

- **Built**: implemented as the architecture describes, at devnet scale, with
  tests.
- **Simplified**: implemented, with a named simplification that changes a
  guarantee or a cost.
- **Simulated**: something stands in for a component the architecture
  requires, and is labeled as a stand-in in the types and in the output.
- **Not built**: absent.

## At a glance

| Architecture section | Status |
|---|---|
| Foundations: two-sorted state, derived coordination class | Built |
| Identity: post-quantum accounts | Built |
| Identity: personhood | Not built |
| Identity: Agent Charters | Built |
| Identity: agent identities and subagents | Built (devnet scale) |
| Identity: batched authorization and keyless subagents (beyond the paper) | Built (devnet scale) |
| Agents: a runtime that lets a model work as an agent identity, within its charter (beyond the paper) | Partly built (runtime with providers and a scripted model, memory, the agent's own skills, skills from others installed through bonds and audits, self-improvement with verified promotion, subagents, hiring, schedules and channels, a sandboxed shell, skills' scripts with the permissions their skills declare, MCP servers over standard input and output or HTTP with OAuth, a browser and a desktop with tabs, downloads and files to upload, storage deals, work it takes on by itself from GitHub and chats under mandates, its pull requests' reviews and checks followed, web search, tools to change code and check it, MCP resources, prompts, input and sampling, sion's own mail, calendar and documents servers, skills imported from folders, repositories and registries (ClawHub, skills.sh, a site's well-known index) through the audit, a model chosen for each task, OpenClaw and Hermes agents imported, tasks taught by doing them once in a browser, X's mentions answered with their sources and its posts searched, everything kept running on a server, live voice with a wake word, a model of speech, echo cancellation and a realtime model that asks the agent, meeting notes by phone or on the machine) |
| Data availability: 2D Reed–Solomon, sampling, reconstruction, fraud proofs | Built |
| Data availability: retention and archival | Simplified |
| Ordering: Linear Lanes (owned objects) | Built |
| Ordering: Escrow Lanes (bounded shared totals) | Built |
| Ordering: Contention Consensus | Simplified (certified DAG, not uncertified) |
| Ordering: censorship resistance (forced inclusion) | Simplified (obligations and committee rotation; the shard is not frozen) |
| Ordering: committees, epochs and staking | Built (devnet scale) |
| Ordering: two levels of finality | Built |
| Execution: prove heavy, execute light | Simulated (replay proofs) |
| Execution: N-version proving | Built (two independent replay stacks) |
| Execution: proving market | Not built |
| The Fold: World Proof per Pulse | Simulated (replay proofs, real bindings) |
| Knowledge layer: claims and the Assurance Algebra | Built |
| Verifiable Clearing | Built |
| Privacy: encrypted payloads, viewing keys, sealed orders | Built |
| Privacy: confidential amounts, zero-knowledge compliance | Not built |
| Interfaces: CLI, HTTP API, MCP server for agents | Built |
| Interfaces: x402, AP2, ERC-8004, C2PA adapters | Partly built (x402; AP2 and ACP checkout) |
| Interfaces: the app, a light node on every device (beyond the paper) | Partly built (desktop shell built and run; mobile configured, not built) |
| Payments: streams with checkpoint settlement (beyond the paper) | Built (devnet scale) |
| Memory: logs for agents and robots, kept append-only by the ledger (beyond the paper) | Built (devnet scale) |
| Orchestration: Task Charters, escrowed work with acceptance fixed at posting (beyond the paper) | Built (devnet scale) |
| Economics: fees, treasury, slashing with compensation | Built |
| Economics: staking, rewards, equivocation slashing | Built |
| Economics: pooled stake, delegation without custody (beyond the paper) | Built (devnet scale) |
| Economics: storage deals, devices paid to keep data and prove it every round (beyond the paper) | Built (devnet scale) |
| Economics: payments to provers, value-in-flight cap | Not built |
| Networking between machines | Simplified (QUIC between processes, peers from files; a stopped validator cannot rejoin) |

## By section

### Foundations

**Built.** State is two-sorted (`sion-exec`): linear objects with owners
and versions, tombstoned when consumed and never deleted, and a grow-only
knowledge log (an RFC 9162 Merkle log in `sion-core`). A transaction's
coordination class (knowledge, owned-linear, bounded, contended) is derived
from what it touches (`Transaction::class`), never declared by the sender.
Every consensus-critical value has one Borsh encoding and is hashed with a
domain tag. The state root is a sparse Merkle tree with single-leaf
subtrees collapsed (`sion_core::smt`).

### Identity

**Accounts: built.** Accounts are identified by the hash of their genesis
authorization policy, so keys rotate (`RotatePolicy`) without changing the
identifier. The default policy requires two signatures from unrelated
families over the same digest: ML-DSA-44 (FIPS 204, lattice) and
SLH-DSA-SHA2-128f (FIPS 205, hash-based); `SLH-DSA-SHA2-128s` is available
for accounts that sign rarely. ML-KEM-768 (FIPS 203) with
ChaCha20-Poly1305 encrypts payloads to viewing keys. The implementations
are the pure-Rust `fips203`, `fips204` and `fips205` crates; they have not
been audited for side channels.

**Personhood: not built.** The architecture's issuer-signed credentials with
per-application nullifiers can be expressed today as claims, but there is no
nullifier scheme and no unlinkability (that needs zero-knowledge proofs).

**Agent Charters: built.** A charter binds an agent to a principal with
caveats the protocol checks on every payment (`sion_exec::charter`): an
allow-list of payees, a per-payment limit, a budget per window, an expiry,
an evidence requirement above a threshold (a claim that passes the decision
rule under a stated policy), receipts as claims, and whether the agent may
delegate. Delegation must attenuate (every caveat strictly narrower).
Revocation by the principal stops the agent and every sub-agent at once;
`ReclaimCharter` returns unspent allotments. Payments come from an
allotment only the agent can write, so they need no consensus.
Not built: the architecture's model terms (which models and providers may
act) and a bond behind each agent's receipts beyond the agent atom.

**Agent identities and subagents: built (devnet scale, specification
v1.8).** An agent identity is a charter with an identity record (a card
commitment and a depth); its id is the agent's id. `SpawnAgents` launches
up to 256 subagents per transaction, each with its own account, card,
attenuated caveats, a bucket carved from its parent's and a budget carved
from its parent's allotment. Any overseer (the backer, the principal, an
ancestor's agent) can suspend a subtree, the suspender or the backer can
resume it, and the agent or any overseer can revoke it. Both proof stacks
implement it, and the differential tests cover every operation and its
failure cases. The `swarm` demo runs it on a devnet. Specification v1.13
adds **keyless subagents**: a subagent spawned without an account gets one
derived from its id, which no key controls, and acts only as a member of
batches its nearest keyed ancestor signs (see [Batched
authorization](book/src/concepts/batched-authorization.md)); the runtime's
`agent_spawn` makes keyless subagents, and `sion agent spawn --keyless`
does from the command line.

**Lineage ids, token buckets, overlays and cohorts: built (devnet scale,
specification v1.14).** A spawned charter's id is `lineage_id(parent,
index)`, the index from a counter in the parent's allotment, and every
charter keeps its lineage, so its ancestors' ids are computed; depths run
from 0 to 8. Rate limits are token buckets (`burst`, `rate`)
carved down the tree with the children's starting tokens, so a subtree
pays at most `σ + ρ·τ` in any `τ + 1` Pulses: a property test checks it on
random trees and cohorts (`sion-exec/tests/buckets.rs`), and fails when a
spawn is made to give tokens it does not take. What happens to an agent
afterwards (revoked, suspended, narrowed) is in its overlay; a check reads
the overlays of the agent and its ancestors, at most 9, most of them proofs
of absence. `NarrowAgent` narrows a subtree with one write. An overseer
acts only in good standing (its own charter and those above it neither
revoked nor suspended), the highest suspender is recorded, an expired
charter spawns nothing, and a subagent that must cite evidence judges it
by its parent's policy (fixes from a review of v1.14). `SpawnCohort`
commits to up to 2²⁰ subagents in one transaction by a Merkle sum tree over
their bursts and rates; `MaterializeAgents` creates up to 128 of them with
proofs of their leaves. The node answers `GET /v1/agents/{id}` with the
charter and its lineage's overlays, each with its state proof. Both proof
stacks implement it, with differential tests for every new operation and
its failures. Not built, and named in the [design
note](book/src/research/agent-scale.md): a rule that makes a certified
agent payment final before it executes (today a suspension, narrowing or
revocation that executes first applies to it), agent keys of ML-DSA-44
alone, and lane keys. A narrowing cannot narrow a bucket (it would need
tokens of its own), and a cohort's unmaterialized members cannot be
revoked one by one: revoking or narrowing their parent stops them all.

**Batched authorization: built (devnet scale, specification v1.13).** One
authorization signs a batch of up to 256 transactions (`TxBatch { signer,
size, root }`, the root an RFC 9162 tree over their digests); each member
carries the authorization, its position and its path, and its fee floor is
the usual one less 80 % of the base fee (the part that pays for checking
signatures), plus its share of that part, rounded up. Both proof
stacks implement it (`sion_exec::auth`, `sion_spec::authorize`), with the
differential tests covering batches a sender signs for itself, keyless
subagents at two depths and below a keyed subagent, every way a member can
be refused, forced intents whose signer rotates, and settlements that pay
per hash on the member's floor. Validators, the executor and both proof
stacks check a batch's signatures once and each member's path every time;
the gateway sends each shard committee a batch's members in one message
and consensus groups carry its authorization once; `POST /v1/batches`
takes a whole batch. Not built: a sequence number per signer that would
make two conflicting batches slashable, so that a payee could rely on a
member before it is certified; today conflicting batches are settled by
lane locks, as any two transactions are. Trust atoms:
`agent:keyless-signer`, `impl:batch-size-fee`.

**Agent runtime: partly built (beyond the paper).** `sion-agent` runs a
language model as an agent: the Anthropic Messages API (version
2026-10-21), any OpenAI-compatible API (with `local:`, a model server on
the machine or its network), Google's Gemini API natively, OpenAI's
Responses API, the Responses API on a ChatGPT plan signed in on the
machine (`sion agent chatgpt login`), or a scripted model; fallback with
a 30-second cooldown; a profile's routes choosing its models for each
task (pictures, work items and long tasks by rule, other tasks sorted by
a model shown the task as data and allowed only to name a route, a
model moved first in a route only by beating its first on a set of tasks
by Newcombe's 95% interval), tested with scripted models; what each model
thought kept and given back to it alone (Anthropic's signed thinking, the
Responses API's encrypted reasoning items, Gemini's signatures on its text
as on its calls, a chat completions server's reasoning text within the
task), let go when a provider refuses it, and how hard each model thinks
written after its name (an effort level, or a budget for the models that
take one) with room to think counted in a run's budget, tested against
local stand-ins for each API (trust atom `agent:kept-reasoning`); a
large toolbox whose own tools stay in view, the rest found by a ranked
search (BM25) and loaded to be called by name; programs in Python that call
the session's tools that only read, run in the sandbox with nothing of the
machine in view, each call checked and made as the model's own would be
(trust atom `agent:code-mode`); the operator's hooks before and after
tool calls and runs, a hook before able to block a call, one that fails
blocking it (trust atom `agent:hooks`); subagents that change code in
git worktrees of their parent's repository, side by side, their work
committed on branches of their own by git run so that nothing the
subagent wrote steers it (trust atom `agent:worktrees`); tools
that declare an effect (read, write, external,
spend); autonomy levels L0 to L6 with approvals that expire after 15
minutes, each waiting call named by a request id that a decision may give
and is checked against, each decision kept with who made it, and
approvals that allow the same call again, or the calls a rule matches, for
the rest of a session (never a payment, never while tainted, never for an
argument read in quarantine); taint for 24 hours after content from
outside (a page, another agent's message or task output, a tainted
parent's task), during which external calls and spending need approval;
quarantine, if asked for, which holds content from outside out of the
conversation for a reader without tools to answer to a schema, answers of
values leaving the session untainted and the strings they give watched
for in external calls and payments; a journal that returns the recorded outcome of a
repeated call and never repeats one whose outcome is unknown; a turn
budget of 12, plus 4 per turn of progress, at most 32; a token budget per
run, if given, held before each model call to an estimate of what it sends
and the most it may answer, ending with a last answer within a reserve;
a stop for a model that goes round in circles (told at the third same
result in a row, stopped at the fifth); compaction that cuts the results
of calls long past first and begins each summary with what the session
did that lasts; a system prompt that stays the same through a session,
for providers' caches, and the tokens read from and written to those
caches counted; a check of the task, if given, run each time the model
says it is done, and an end for a task it cannot complete; a kill switch;
sessions that serialise, pause for approval and resume; trajectories
in chat format; and graphs of a profile's agent (fan-out, work-check,
draft-review, self-consistency, orchestrator or one's own, with copies,
votes, joins that wait for all, the first or a quorum, read-only nodes,
runs pinned to their graph, replayed and continued after a person), each node a session, a router that keeps a
task to one agent unless it divides, and taint passed along the edges
([Graphs of agents](book/src/agent/graphs.md)). A comparison
(`sion agent graph eval`) runs one agent, one agent voting over several
tries and each graph on a set of tasks with stand-in tools, on the same
token budget, each case several times, and reports pass rates with
Wilson intervals; a graph whose rate beats both baselines by Newcombe's
interval earns the profile's traffic, and the router offers only graphs
that have. A judge (`sion agent judge`) reads a session's steps after
labelled examples, and is measured on labelled steps it was not shown. A
session that holds confidential content (told so, read from a path named
confidential, or reached from a subagent or graph node that held some) has
its external calls and spending wait for a person; decisions can be signed
with a wallet's keys and checked (`sion agent decisions`); the runtime's
own notes are marked where a tool's result imitates them; and a test of
nine attacks on a model that obeys everything it reads shows what holds,
and that two get through, as built. Calls that only read run side by side
in a turn, the latest of what a session read or changed outlives
compaction, and A2A tasks push their updates to their clients' webhooks
and can be subscribed to again. `sion agent run` gives it the
ledger as tools (balance, payments under the agent's charter, transaction
status), files in a workspace and public web pages
([Your agent](book/src/agent/index.md)). With a memory log,
the agent records facts, episodes, procedures, preferences, goals, plans
and reflections as atoms; its preferences, goals and procedures are in
every prompt; recall for a subject and a relation is proved complete
against the index root on the ledger; retraction leaves atoms in the log;
each finished session is recorded as an episode with its trajectory's
digest and the head is committed. Its own skills, in the Agent Skills
format, are listed in every prompt, read on use, and written and improved
by the agent with their history kept; a skill's bundle is a canonical
encoding of its files, and its root is the Merkle root a storage deal over
the bundle commits to. Skills from others
([Skills from others](book/src/agent/skills.md)): a release is a claim
(`skill/v1`) naming a skill, its version and its bundle root, resting on
the publisher's bond or audits of the same root (`skill.audit/v1`, each on
its auditor's bond); `sion skill install` checks the bundle against the
root, accepts the release with the agent's remaining budget at risk
(reserving κ times that of its backing), and only then writes the skill,
which the agent cannot rewrite. Before each run, a skill whose release is
no longer active is taken out of use. Since specification v1.15 a
publisher's index on the ledger names its latest release of each skill
(`IndexClaim`, moving only forward), and `sion skill install
<publisher>/<name>` installs it after checking the index entry's state
proof against a certified header (a proof from more than 16 Pulses before
the latest certified Pulse the command knows of is refused as stale; a
node can still answer from an older state within that window, or when the
command knows of no later Pulse). Self-improvement
([Skills that get better](book/src/agent/skill-improvement.md)): an
evaluation set is cases with stand-in tools and checks, fixed by the
digest of its bytes; `sion skill evolve` shows a model the cases a
version fails and scores the variants it proposes (new instructions, and
the skill's other files but never its scripts), under gates (purpose,
files, size, novelty, no regression), keeping a Pareto front, and keeps
the best as a draft, with `--held-out` only if it passes every case the
evolution never saw that the version given passes; `sion agent evolve` does the same for a profile's
own instructions, which follow the fixed system prompt; a bounty is a Task Charter whose spec names the release, the
set and the cases to pass, with its verifiers, adjudicator and required
claims (such as the set's author vouching for it, `skill.evals/v1`) fixed
before any version exists; verifiers run the set themselves and attest;
the next release rests on the settled result and records it, and
`sion skill lineage` follows the evidence back. After a finished task
with at least five successful tool calls and no skill written, the
runtime asks the agent whether to write one, with only its skill tools.
Scripted models take conditions on the conversation, to stand in for a
model that follows a skill. Teams, schedules and channels
([Subagents and hired help](book/src/agent/teams.md),
[Profiles and schedules](book/src/agent/schedules.md),
[Channels](book/src/agent/channels.md)): with
`--delegate`, an agent spawns subagents (a budget carved from its
allotment under attenuated caveats, fee money from its account, a session
of their own, two levels deep at most), and posts, follows and delivers
Task Charters; profiles keep how to run an agent under a name;
`sion agent schedule` and `sion agent scheduler` run a profile's task
once, every interval or on cron (UTC), and deliver its answer to a file,
a webhook, Telegram, or a chat on any messenger that can write to one
unasked; `sion agent serve` (HTTP with the machine's token or a
profile's own) and `sion agent bridge` (every messenger below, chats
allowed by id; what a bridge has taken kept until answered, across a
restart) carry conversations, each a session, with approvals answered in
the channel, by named approvers if
given; with `--pairing`, someone who writes to a bridge directly and
whom it does not know is given a code that the operator approves
(`sion agent pairing`); with `--require-mention`, a group is answered
only where it mentions the bot; with `--hear` and
`--speak`, voice notes are transcribed and answered aloud;
`sion agent serve` also answers OpenAI's `/v1/chat/completions` and
`/v1/models`, each conversation an app sends whole going on in its
session; `sion agent gateway` routes a profile's messages to the machine
that proved the agent's key with `sion agent host`.
Commands, scripts and MCP servers
([Commands, scripts and MCP servers](book/src/agent/tools.md)):
`--shell` runs `/bin/sh` commands in a sandbox (bubblewrap, or a Docker
image pinned by digest and never pulled) that writes only the workspace
and reaches no network unless the operator gives one (refusing Docker's
`host` and `bridge`); `skill_run` runs a skill's own scripts there, with
the network only for skills that declare it; `sion agent mcp add` reviews
an MCP server's tools and pins each by a digest of its definition, and a
session offers only tools that still match, with effects from the reviewed
annotations or the operator, in both eras of the protocol (2026-07-28 and
the handshake of 2025-11-25 and before), over standard input and output or
over HTTP (Streamable HTTP in both eras, and the deprecated HTTP+SSE);
a server over HTTP that asks for authorization is logged in to with OAuth
as the MCP specification says (protected resource and authorization
server metadata, a client registered beforehand, named by a client ID
metadata document or registered dynamically, PKCE, `resource`, the
issuer checked), its tokens kept encrypted, renewed without a person and
revoked at logout, or with the client credentials grant for agents that
run unattended; `--storage` lets an agent keep a
workspace file with storage providers and follow and close its deals.
Calls meant to run again (commands, scripts, file writes) are not
journaled, and a replayed outcome says so. The spawns of one turn go
together, in one `SpawnAgents` transaction and one transfer of fee money,
and their sessions run at the same time; their keyless payments are signed
in batches as they arrive. Tested with the scripted model and with
stand-ins for the providers' and the messengers' APIs: this environment
reaches none of them. The sandbox tests run bubblewrap for real, and
Docker with `SION_TEST_DOCKER_IMAGE`. An agent with a memory log searches
its past sessions (`session_search`, `sion agent sessions search`:
BM25), each hit checked against the trajectory digest its episode
committed to the ledger; `sion memory consolidate` turns episodes into
derived notes under gates, and `sion memory dream` reads the sessions
themselves, a reader each, keeping a note only if its session says the
words it quotes, tested first on held-out tasks with the notes in mind
and without (`--eval`), into the agent's log or a curated log that only
dreams write (`--into`, read by sessions given `--shared-memory`), with
recurring failures turned into skill drafts and each note's confidence
from its provenance,
superseding and never deleting; what a tainted session records, its
episode and the notes consolidated from it are marked as prompt-injection
risks and kept out of prompts; `sion memory archive` and `restore` keep a
log with a storage provider as an archive encrypted for its writer, named
as the head's manifest, and bring it back checked against the ledger,
providers serving retrievals to their deals' clients. Channels reach a
profile through its own tokens, kept as hashes (approvals only with
tokens made for them); Slack, Discord, Matrix, Signal and Telegram behind
one bridge, with approvers; and an agent gateway that passes a profile's
messages to the host that proved the agent's key, only while the agent is
in good standing. Discord is reached through its gateway and Slack,
given an app-level token, through Socket Mode, over the WebSocket client
of `sion-http` (which also reached Discord's real gateway and a public
echo server through this environment's proxy, in an ignored test), so
direct messages arrive too; `--require-mention` answers in a group only
what mentions the bot or answers it. Pairing codes (8 consonants and
digits, an hour each, three waiting per channel, a lockout after five
wrong approvals) let in one direct chat on one channel and profile, from
a store only the operator's user reads, changed by one process at a
time. Mattermost (its WebSocket), IRC and Twitch (connections of their
own, over TLS) are answered too; trust atom `agent:irc-nicks`. WhatsApp
(the Cloud API), LINE and SMS (Twilio) reach a bridge as
webhooks on an address of its own, each checked against the service's
signature (HMAC-SHA256, or Twilio's HMAC-SHA1) before anything reaches
the profile; trust atom `agent:webhooks`. Email is read over IMAP (IDLE
where the server has it, without marking anything read) and answered over
SMTP in the writer's thread, TLS or STARTTLS required but to a loopback
address, and only what the provider's `Authentication-Results` show to be
from the address it names, and not written by a machine, is answered;
trust atom `agent:email-from`. Microsoft Teams (the Bot Framework) and
Google Chat (an HTTP endpoint app) reach a bridge as webhooks whose
tokens are checked (RS256, against the keys the services publish, with
the TLS stack's RSA) before anything reaches the profile; Teams is
answered through the Connector API with the app's token, Google Chat in
the webhook's own answer or, with a service account's key, through the
Chat API; trust atoms `agent:teams-tokens`, `agent:google-chat-tokens`.
iMessage is answered through `imsg` on the Mac that runs Messages (its
JSON-RPC over stdio, there or through SSH), from the bridge's place in
the Messages database; trust atom `agent:imessage-handles`. DingTalk is
reached in Stream mode (a WebSocket the bridge opens, each frame
acknowledged), Feishu and Lark as webhooks with the event subscription's
verification token (and, with an Encrypt Key, encrypted and signed),
Nextcloud Talk as webhooks signed with the bot's secret; trust atoms
`agent:dingtalk-stream`, `agent:feishu-token`, `agent:talk-signatures`.
WeCom's callbacks are decrypted and checked as its `WXBizMsgCrypt` has
them (AES-256-CBC, a SHA-1 signature), QQ's webhooks against Ed25519
signatures under the key derived from the bot's secret, Synology Chat's
against the bot's token; WeChat is reached through Tencent's iLink bot
protocol (signed in with a QR code), Zalo and X's direct messages by
polling their APIs, SimpleX through its terminal client's WebSocket API,
ntfy's topics by polling and publishing, phone calls through Twilio
Voice's signed webhooks, and Nostr on its relays: NIP-17's private
messages (NIP-44, NIP-59) and NIP-04's, every event's signature and a
seal's sender checked; trust atoms `agent:wecom-callbacks`,
`agent:qq-signatures`, `agent:synology-chat-token`,
`agent:simplex-client`, `agent:nostr-relays`, `agent:ntfy-topics`,
`agent:phone-callers`. The HTTP channel answers other agents in A2A
(0.3 and 1.0), with an agent card for each profile it publishes, and a
profile may have Home Assistant's entities and services as tools
(`agent:home-assistant`). `sion agent talk` is a conversation in the
terminal, by voice with `--voice`; with `--live` it listens all the time,
speaks the reply a sentence at a time as it is written, and stops when
spoken over, and `--wake` takes only what begins with a wake phrase,
heard first by a transcriber on the machine. Speech is told from other
sound by RNNoise's network (`nnnoiseless`) with a loudness floor, or by
loudness alone (`--detector loudness`); the reply's echo from
loudspeakers is cancelled: raw samples played at their pace and kept on
the microphone's count, the delay found by correlation and the room's
echo learned by a 64 ms NLMS filter, which stops learning in double talk,
with what is left quietened while only the loudspeaker sounds.
`--realtime openai:<model>` hears and speaks through OpenAI's Realtime
API, whose one tool, `ask_agent`, goes to the profile's agent; utterances
are found and sent from the machine, its own turn detection off.
`sion agent meeting` takes a meeting's notes, dialled in through Twilio
(its keys pressed, its transcription posted back, signed), heard on the
machine (the microphone, and what it plays as the echo reference), or
joined by its link as a guest that listens (Chromium with a silent
microphone and a black camera, which names itself, asks to join and
waits to be let in, the meeting's sound heard from a PulseAudio sink of
its own), the notes written by the profile's agent in a tainted session;
with `--diarize`, what was heard is recorded and heard again by a
transcriber that tells speakers apart (OpenAI's
`gpt-4o-transcribe-diarize`, Deepgram's or ElevenLabs'), and the notes are
written from the transcript by speaker. By default a live conversation's
turn ends after half a second of quiet unless what was said sounds
unfinished (by its punctuation and its last English word), which is held
a second and a half for the rest (`--turns semantic`). Hearing and
speaking can stay on the machine: whisper.cpp (`whisper:`) and Piper
(`piper:`), or a server that speaks OpenAI's API (`local:`), any of which
can hear the wake word. Tested end to end on a devnet with stand-in
microphones (silence and a tone, paced as spoken), stand-in transcribers,
a scripted model, a slow player, and stand-ins for the Realtime API and
for Twilio; the meeting guest with Chromium against stand-in meeting
pages served on the machine (one that lets it in, one that refuses it);
telling speakers apart with a stand-in for its transcriber; whisper.cpp
and Piper with stand-ins for their programs; the model of speech on a
sentence spoken by espeak-ng and on noise; the echo canceller on a room
made up (at least 20 dB quieter once learned, a voice over it within 3
dB). No microphone or loudspeaker was used, and neither the Realtime API,
Twilio, Google Meet, Zoom, Teams nor a service that tells speakers apart
was reached. Trust atoms `agent:live-voice`, `agent:realtime-voice`,
`agent:meeting-notes`, `agent:meeting-guest`.
Voice notes are heard on every messenger (OpenAI-compatible
transcription, Groq, Deepgram, ElevenLabs, or a command) and answered
aloud (OpenAI, ElevenLabs, Deepgram, or a command): a voice note on Telegram, a voice message on
Matrix, audio attached to the reply on Discord, Slack and Signal; trust
atoms `agent:pairing`, `agent:voice`. While the agent works, a chat sees
it typing (each platform's own indicator, refreshed on its clock, or a
reaction where a bot has none) and its reply as the model writes it: the
providers stream over Server-Sent Events, a watched run tells what it
does (`sion_agent::Seen`), and the reply shows on Telegram's drafts,
Slack's and Teams' streams, Feishu's streaming cards and DingTalk's AI
cards, or as a message edited where those are not; long replies are split
where they read best. The HTTP channel and its OpenAI-compatible endpoint
stream the same text as Server-Sent Events. A bridge answers chats side
by side, one turn at a time in each, a burst of messages one turn; what
a chat says meanwhile steers the agent, is queued, is collected or
interrupts it, and `/stop` stops it at once, a model that is writing cut
off and what it wrote kept. Messages are marked taken, answered or failed
(reactions, or read receipts), and replies are tried again, held back
while a messenger's breaker is open and kept in an outbox across
restarts. The chat commands (`/retry`, `/undo`, `/compact`, `/usage`,
`/sessions`, `/resume`, `/title`, `/model`, `/whoami`, `/help`) work on
every channel. Pictures people send are shown to the model and every
file is put in the workspace's `inbox/`, the agent told where; what it
shares (`share_file`) is sent after the reply as each messenger takes
files, and a profile with an image model makes pictures
(`image_generate`). The messengers and the speech services are tested against
stand-ins for their APIs, and the bridge against a messenger kept in
memory. What a skill may do
([What a skill may do](book/src/agent/skill-permissions.md)) is
declared in its front matter and enforced: `allowed-tools` pre-approves
calls once a session has read the skill (never past a refusal, spending
or a taint, and never from a skill the agent wrote, whose front matter
keeps only a name and a description); `sion-network` names the hosts its
scripts reach through an egress proxy outside the sandbox, which refuses
private, loopback and link-local addresses and reaches the inside through
a bridge on a Unix socket (the `declared` network gives commands none);
`sion-secrets` names secrets from the operator's store, encrypted at rest
with ChaCha20-Poly1305, passed as environment variables and cut out of
what scripts print; `sion skill install` shows what a release declares
before anything is signed and installs it only with
`--accept-permissions`. A bundle's paths must be portable and distinct
when case is ignored, so that on macOS or Windows no file takes another's
place. The filtering runs in bubblewrap for real in the tests. Trust
atoms: `agent:skill-pre-approval`, `agent:declared-hosts`,
`agent:secret-store`. Skills are imported
([Importing a skill](book/src/agent/skills.md#importing-a-skill)) from a
folder, a git repository, ClawHub (`clawhub:owner/slug`), a site's
`/.well-known/agent-skills/` index (v0.2) or skills.sh's index of GitHub
repositories, and searched for on ClawHub and skills.sh: every file
checked against the SHA-256 the registry lists for it, ClawHub's scanners
reported one by one and a version any of them calls malicious refused, an
index without digests refused, archives unpacked with no path out of the
skill and no links; the files checked for what the 2026 campaigns against
ClawHub did (downloads run by a shell, paste sites, encoded payloads,
credential, keychain and wallet paths, persistence, edits to an agent's
memory files, words aimed at the model, characters a reader cannot see,
programs and archives), the worst holding an install back until the
operator accepts the risk (`--accept-risks`); installed only on the
operator's review of its bundle root or an audit on a bond, with hosts
and secrets the operator grants written into its metadata. Tested against
stand-ins for ClawHub and a site, and run once against ClawHub itself (a
search, and a skill read, checked against its listed digests and
reported on, not installed); trust atom `agent:skill-import-review`.
An OpenClaw or Hermes Agent is brought over to a profile
([Coming from OpenClaw or Hermes](book/src/agent/import.md); `sion agent
import`, a plan first): its persona and memory files into one block of the
profile's instructions, marked as another agent's notes and checked as a
skill's files are, the worst findings holding it back until the operator
accepts them; its jobs as schedules, cron moved to UTC with the zone's
offset today; with `--models`, `--keys` and `--bridges`, its models, its
model keys (sealed) and a bridge service for each chat it was in
(Telegram, Discord, Slack, Matrix, Mattermost), installed and not started;
its skills listed for the audit. OpenClaw's JSON5 config, Hermes's YAML
and both `.env` files are read by readers of sion's own, tested on homes
laid out as OpenClaw 2026.10 and Hermes Agent 0.22 lay them out; neither
program was run. Not imported: conversations, OAuth logins, OpenClaw's
pairing lists and jobs kept only in its database (it exports the jobs),
WhatsApp, Signal and iMessage logins, per-chat settings. Trust atom
`agent:imported-agent`.
A task is taught by doing it once in a browser window (`sion skill
teach`, [Teaching a task](book/src/agent/skills.md#teaching-a-task)):
Chromium with a profile of its own, driven over its DevTools pipe, a
recorder in an isolated world of every frame saying what the person does
(trusted events only), and the recording, at most ten minutes, written
down as a draft `SKILL.md` for the browser tools, installed through the
import's audit. Passwords, fields that look secret and a login's user
name are never sent from the page; addresses lose their secret-looking
parameters. A field counts as secret by its type (as it
was when the page loaded, so a password shown as text stays one), its
autocomplete, its masking by the page's style, and what its name, label
and placeholder say; the user name typed before a password, on its page or
the one before, is not kept; addresses lose long tokens in their paths
and parameters that name the person; titles and names lose email
addresses; a prompt's answer is not kept. Tested with Chromium for real,
headless and in a window on Xvfb, played by a program through the
protocol's input events on a site served on the machine
(`crates/sion-agent/tests/teach.rs`); and `sion
skill teach` run once end to end in a window on Xvfb as an ordinary user,
typed into with xdotool and ended by closing the window (not part of the
test suite). A person did not use it. Not recorded: drags, hovers, scrolling, keyboard shortcuts
other than Enter and Escape, canvas and plugin content; frames of other
sites are recorded as their own targets, untested. Trust atom
`agent:taught-skills`.
Mentions on X are answered in their threads (`sion agent x-mentions`,
[Mentions on X](book/src/agent/messengers.md#mentions-on-x)): the
sources gathered by the runner before the session (the post replied to
or quoted, and the web search's results for the mention's words), the
session tainted and without memory, skills, workspace or any tool, and an
answer posted only if it cites what it was given and names no address it
was not (anything X would link, scheme or none); names unlinked, the
reply addressed to the author alone; split into chained posts within X's
weighted count, the addresses in the last; mentions counted as they are
taken on and kept before replies are posted. A tainted session's skill
writes wait for a person, in every channel. `x_search` searches recent
posts. Tested against stand-ins for X's API and a SearXNG instance with
the scripted model on a devnet (`crates/sion-cli/tests/x_mentions.rs`);
X's API itself was not reached. Not built: answering quotes that do not
mention the account, pages of more than 100 new mentions between polls,
media in replies. Trust atom `agent:x-mentions`.
MCP over HTTP is tested against the official MCP
Python SDK (2.3) with its OAuth authorization server, and against a
stand-in for each era of the transport; trust atoms `agent:mcp-remote`
and `agent:mcp-oauth`. MCP resources, prompts, a person's input
(elicitation: forms and addresses to open) and sampling are answered in
both protocol eras (2026-07-28's input-required results, and an earlier
server's own requests during a call), tested against a stand-in server in
each era and end to end at the terminal on a devnet
(`crates/sion-cli/tests/agent_reach.rs`); sampling is off for every
server until the operator allows it, within limits counted in the run's
budget. Trust atoms `agent:mcp-input`, `agent:mcp-sampling`. sion's own
MCP servers (`sion mcp mail`, `calendar`, `documents`) are tested against
stand-ins for IMAP, SMTP and CalDAV, and, for documents, as the real
binary mounted in a session (`pdftotext` reading a PDF made for the
test); trust atom `agent:own-mcp-servers`. `web_search` asks Brave, Tavily
or a SearXNG instance, tested against a stand-in for each and end to end
through the SearXNG one; trust atom `agent:web-search`. The code tools
(`edit_file`, `apply_patch`, `find_files`, `search_files`, `read_file`'s
ranges) are tested in their crate and end to end; `diagnostics` and
`run_tests` are tested on samples of each checker's and test runner's
output, and were not run in a sandbox in the tests. Not built: resource
subscriptions and notices that a server's lists changed; client
certificates (mutual TLS) for servers over HTTP; the device authorization
grant
([Reliable, self-improving agents](book/src/research/agents.md)).
A browser and a desktop
([A browser and a desktop](book/src/agent/browser.md)):
`--browser` drives Chromium or Chrome over the DevTools protocol on a
pipe, a page shown to the model as its accessibility tree with references
(frames of the page's own site inside it), in bubblewrap on Linux with the
machine's managed browser policies, reaching public addresses only, or the
hosts `--browser-host` names, through the egress proxy; held to named
hosts, it acts on their pages as the autonomy level allows
(`Policy::contained`), and otherwise its actions after a page wait for a
person; the operator's logins (`sion agent login`) are filled only into a
field whose own frame is of the login's origin, and cut out of all it
returns. `--computer` gives an X server of the session's own (Xvfb,
xdotool, ImageMagick) in bubblewrap or a Docker image, with the programs
the operator allows, as the `computer` tool. Pictures reach Anthropic's
models in the tool result and OpenAI-compatible ones after it, the last
three shown, the last six kept, every one named by its digest in the
conversation. The browser and the desktop run for real in the tests
(Chromium in bubblewrap against two sites on this machine; Xvfb in
bubblewrap), and the guide's transcripts are of docs.rs. Trust atoms
`agent:browser`, `agent:browser-hosts`, `agent:browser-logins`,
`agent:desktop`. Tabs (up to eight), downloads into the workspace and
files chosen for a page's file field from it run for real in the tests
too (Chromium in bubblewrap against a site on this machine); trust atom
`agent:browser-files`. A browser profile the operator signs in with
(`sion agent browser login`, a headed Chromium) is kept and given to a
session's browser by `--browser-profile`, one browser at a time, closed
gracefully so that its cookies are written; tested in bubblewrap with a
cookie set on a page on this machine and found by the next session's
browser (trust atom `agent:signed-in-browser`). Where the tree says too
little, a screenshot marks the last tree's references on the page, and
`browser_click_at` and `browser_type_focused` act on a point and on the
focus. `browser_ask_person` hands the page to the person the session can
ask: a page served on 127.0.0.1, by a token of 128 bits and answering only
to its own host name, a few requests at a time and each bounded, shows the
browser's page and passes the person's clicks, text, eight keys and
scrolling to it until they reply (in a chat, within ten minutes); a page
that shows a frame of reCAPTCHA's, hCaptcha's or Turnstile's says so, and
captchas are handed to the person, never solved by the agent or a
service. Tested in
bubblewrap with a stand-in person who opens the page, clicks and types a
code (trust atom `agent:browser-handover`). Not built: a desktop that
outlives its process; a hand-over reachable from another machine but by a
tunnel.

**Renting an agent out, and agents' records: built (beyond the paper).**
`sion agent rent serve` lists a profile's agent for rent by the token: a
claim naming the agent, the USD per thousand input and output tokens, the
payment stream's step, the least to escrow, a line of what it is for, and
an attestation of the enclave it runs in, named the owner's latest under
`rental:<name>` in its index; the attestation's binding commits to a
digest of the terms, so a listing changed afterwards no longer matches it.
A renter (`sion agent rent use`) checks the attestation before anything is
sent or paid, escrows in a payment stream, seals each request to the
enclave's ML-KEM key, and pays what the enclave signed for; the enclave
counts the tokens and signs the running total with the key its attestation
binds, payment running one request behind, so the owner is ever owed at
most one request's tokens. A rented session runs the owner's profile
stripped of its memory, files, skills, web, shell, MCP servers, screens and
subagents, tainted from the start. Attestation is `sion-enclave`: an AMD
SEV-SNP report checked against AMD's chain for Milan, Genoa and Turin (its
root and signing keys built in) with aws-lc-rs, the chip and firmware
versions its key names matched, debuggable guests refused, the binding and
its expiry checked; reports asked for through configfs-tsm, the chip's key
from the host's table or AMD's key service. Tested on a real Milan report
and chip keys from `virtee/sev`'s tests (changed bits, a forged key and
another chip's key refused), and end to end on a devnet, over a loopback
address, with a simulated enclave and a stand-in agent: the ledger shows
the stream's position is the signed total in steps and its escrow falls by
exactly that. No AMD hardware, and no model provider, was reached. An
agent's record is read from the ledger's own events (tasks delivered and
what they earned, tasks released, delivered results later proven false)
and turned into one number to trust it by; finding an agent for a task
weighs how well each candidate's offer fits the task (BM25) against that
record, held to what is needed (an enclave, a price). Trust atoms
`agent:rented-enclave`, `agent:metered-tokens`, `agent:agent-record`. Not
built: checking Intel TDX quotes (collected, then refused); AMD's
revocation lists; Turin's layout of firmware versions in a report; a
listing served on the renter's own model key; building and launching the
confidential VM itself; a quote before a request, or a spending cap inside
a rented session; enumerating every agent or listing on the chain to search
through.

**Improving the whole agent, shared memory, projects, a speaking guest
and the trust layer: built (beyond the paper).** Changes to the agent
itself — the words its operator gives it, the models a kind of task
reaches for, how it works a task — are scored against the agent as it is
and kept only when better beyond chance on the tasks tried (Newcombe's 95%
interval above zero) and regressing on no held-out task it already
passes; each kept change is recorded with its evidence, and one that only
looked better is turned down and said so (trust atom
`agent:measured-improvement`). Memory shared between owners carries who
vouched for a fact (a claim on the voucher's bond, under an adjudicator)
and whether it was revoked (a claim named under the atom in the index); a
revoked fact and everything derived from or assuming it, however deep, are
left out, the walk ending because content addresses admit no circle
(`agent:vouched-memory`). A project gives an agent a goal over weeks, a
budget it may spend, milestones and a check-in that reports what it did
since the last one; a project paused, ended or out of budget takes no more
work, and the budget holds whatever it tries to spend
(`agent:project-budget`). A meeting's guest can be given a voice: a
PulseAudio sink whose monitor is remapped into a source the browser takes
as its microphone, and it answers only lines that name it, found anywhere
in the line (`agent:meeting-voice`). The HTTP channel serves three public
reads for agents on other frameworks, without a token: an agent's record,
a transaction's outcome with how to verify its receipt, and a rental
listing with whether its enclave checks out (`GET /v1/trust` says what it
serves). All unit-tested; the guest's microphone needs PulseAudio and a
meeting and was not run, and no change was measured against a real model.
Not built: choosing the tasks an improvement is measured on (a set written
to flatter a change would pass it); a project that spends
from its own allotment on the ledger rather than being counted against a
budget kept here.

**Work the agent takes on by itself: built (beyond the paper).** `sion
agent work` gives a profile's agent watches: GitHub repositories (new
issues and pull requests, and the comments on them, read through the
REST API) and chats on the messengers a bridge answers
([Work it takes on by itself](book/src/agent/work.md)). A fixed gate
decides who may start work: someone on a watch's list who says
something to the agent (a mention, an assignment, a direct message, a
reply to it) makes a request; a bot on the watch's bot list, and the
kinds of message its admission table names, make candidates to weigh;
anything else starts nothing. A thread (an issue, a chat's thread, a
direct chat, a group chat outside its threads) has one item open at a
time, and what is said on it goes to that item (in a group chat outside
its threads, only what is said to the agent): a listed person's message
as theirs, anyone else's marked as data. A candidate is weighed by a
model with no tools against the operator's rules, and taken on only in
`auto` mode when the model names a rule that covers it; in `observe`
mode, the default, the runtime records that it would have been. Each
item has a mandate (who asked, under which version of the rules, by
which path, under which rules, what it covers) whose digest the agent's
wallet publishes as a claim (`work.mandate/v1`, a commitment), and which
the item's payments name as their evidence. Before every run the mandate
is checked against the rules as they are: one they no longer allow is
revoked (`work.mandate.revoked/v1`), one a person stops is withdrawn
(`work.mandate.withdrawn/v1`). The agent reports with `work_report` and
asks with `work_ask`: the runtime says once that it took the work on,
sends nothing the agent only notes, and sends its results, one question
at a time and why it cannot go on, to the conversation the work came
from, each message with a purpose and within 100 words in a chat or 400
on an issue. What each item waits for is worked out from its facts, and
`sion agent waiting` lists everything that waits for a person: work
items' questions, approvals, failures and messages that could not be
sent, sessions waiting for approval, chats waiting to be let in. Items
run in slots (two by default), people's first, and one more is kept for
a person's request or answer. A message that could not be sent is tried
eight times, then waits for a person, while the work goes on. Every step
is stored before it is done; a run a restart interrupted goes on in the
same session, told so. Tested on a devnet against stand-ins for GitHub's
API and Telegram's Bot API.
A pull request's item follows its review comments (answered in their
threads) and the checks that fail on its head (from the check's app, when
the watch lists that bot), with tools to read the pull request, its
review comments, its checks and a job's log; with `--listen`, the runner
takes GitHub's webhooks, checked against their signatures, as well as
polling. Tested on a devnet against a stand-in for GitHub's API (a pull
request's review thread and a failed check, end to end); the webhook
listener is tested on its own (signatures, events, deliveries taken
once). Trust atom `agent:github-webhooks`.
**Simplified:** the runner polls (every 30 seconds by default) unless it
takes webhooks too; a change of the rules reaches open items when they
next run, not at once; a
mandate is evidence the ledger records, not evidence that passes a
charter's evidence policy (it is not bonded). **Not built:** other
threads followed for context, a rule's own way of reporting, requests
beyond a mandate handed
back to be weighed again. Trust atoms `agent:work-gate`,
`agent:work-assessment`, `agent:work-messages`, `agent:work-runner`.

### Data availability

**Built.** Each Pulse's data (transaction bodies in execution order) is laid
out as a `k × k` square of Goldilocks field elements, extended with a 2D
Reed–Solomon code to `2k × 2k`, and committed with row and column Merkle
roots (`sion-da`). Light clients sample random shares; withholding enough to
prevent reconstruction is detected with probability `1 − (3/4)^s` for `s`
samples. Iterative row/column decoding rebuilds the square from any
sufficient subset, and bad-encoding fraud proofs exist as a library. The
World Proof additionally checks that the published data encodes to the
certified data root, so the fraud-proof path is a fallback.

**Retention: simplified.** Validators keep a write-ahead log of every
executed Pulse and periodic snapshots, and serve the last 256 Pulses' data
from memory. There is no archival tier and no incentive for anyone to keep
shares.

### Ordering

**Linear Lanes: built.** An owned object version can be locked to one
transaction digest per validator. A transaction is final when validators
holding more than two thirds of its shard committee's stake have approved
it. Approvals are signed per batch (one Merkle root per batch, one dual
signature per batch), so a certificate is a few references into signed
batches. Every transaction, contended ones included, locks its owned inputs
this way before consensus sees it.

**Escrow Lanes: built.** `CreateBounded` splits a shared total (seats, a
budget, a rate limit) into single-writer allotments; spending an allotment
is an owned-path operation; only `Rebalance` goes through consensus.

**Contention Consensus: simplified.** A certified-DAG BFT protocol in the
Bullshark family, partially synchronous, with leader timeouts and
equivocation evidence (`sion_consensus::core`), tested in a deterministic
network simulator with an adversarial pre-GST phase. The architecture's
production target is an uncertified DAG (Mysticeti class), which commits
in fewer message delays.

**Censorship resistance: simplified.** A user posts a transaction
straight to consensus as a forced intent (`POST /v1/forced`, `sion send
--forced`); every validator proposes every round, so one honest proposer
gets it ordered. The Pulse that commits it **posts** it in the state as an
obligation of the sender's shard (specification §6.3), after checking that
it could execute at all (signature, fee, inputs owned at their versions).
The shard then has a window (8 Pulses by default) to execute it or for it
to become unexecutable. Honest validators of the shard approve forced
intents themselves and exchange the approvals, so a certificate forms
without the gateway. At the deadline, the state checks whether the intent
could still execute; if it could, the shard censored it: the intent is
**overdue** and the shard's lane committee is replaced by validators drawn
from outside it first (at most once per shard per window), and the
obligation stands for another window. Every Pulse header commits to the
number of pending and overdue intents and a root over the overdue ones
(`sion_light::ForcedSummary`), so censorship by more than a third of a
committee appears in certified headers that light clients check (`sion
demo censorship`). Simplified: the architecture also freezes the shard
until the obligation is met; here the shard keeps running under its new
committee. With no more validators than seats (the default devnet: 4 and
4), a rotation can only reshuffle the same members. The header's
forced-inclusion summary is certified by the committee and can be checked
against the obligations in the state, but the World Proof does not
recompute it.

**Committees, epochs and staking: built (devnet scale).** The validator
set is an object in the state (specification §2, §5.26–5.30, §6.3). A
validator joins by staking with a proof that it holds its keys, takes its
seat at the next epoch boundary, leaves at a boundary and withdraws after
unbonding. At each epoch's last Pulse the state pays a share of the fees
collected during the epoch (and nothing else the treasury received) to the
epoch's validators by weight, admits and retires
validators, picks the next active set by stake (at most `max_active`) and
reseeds lane-committee sampling with the beacon. Every validator derives
the consensus committee (the active set, weighted) and the lane committees
(`sample_committee` per shard, with rotations) from its state after every
Pulse; nothing about committees is configured. Consensus runs one DAG per
session: a new session begins at every epoch boundary (and every restart);
what the old session committed after the epoch's last Pulse, or never
committed, is proposed again in the new one. Standby validators run as
observers and start proposing once their stake makes them active. `sion
demo staking` runs the whole cycle: a standby validator stakes and joins,
a light client follows the committee change, rewards are claimed, and the
newcomer is slashed for signing two headers for one round and loses its
seat. Simplified: the consensus committee is the whole active set rather
than a sample; a slashed validator keeps its seat until the epoch ends;
validators log equivocation they observe but do not report it themselves
(any account can submit the evidence); and certificates are not drained at
a committee change. A lane certificate is checked against the committee in
force when its transaction executes, so one formed just before a change is
certified again by the new committee. Members who stay keep their locks;
new members start without them, so an owner who signs two spends of one
object version could get the second certified by a fully replaced
committee, defeating the first certificate's finality (it never executes).
Proof finality is unaffected. The architecture drains certificates at
every change; until then certificate finality holds within one
committee's term (`impl:lock-handover`).

**Two levels of finality: built.** Certificate finality comes from the lane
certificate (owned paths) and the consensus commit (contended paths); every
Pulse header is then signed by the committee, and proof finality comes when
the World Proof for that Pulse verifies.

### Execution and proving

**Prove heavy, execute light: simulated.** Execution is cut into chunks;
each chunk carries a statement (world commitments before and after,
accumulators of effects and deferred items, the steps) and a witness (each
touched object with a sparse-Merkle proof, plus the knowledge frontier).
Proof stack A (`sion_prover::ReplayStack`) "proves" a chunk by including
everything needed to re-execute it statelessly; verification re-executes
it and rejects any read outside the witness. This has the interface and
the binding properties of a succinct proof (stateless, chunk-local,
foldable) but not its cost: verifying is as expensive as executing. The
type is labeled `simulated: true`, and every report that verifies a World
Proof says so. A STARK stack would plug into the same `ProofStack` trait.

**N-version proving: built.** The state transition is specified in
`docs/spec/state-transition.md`. A second implementation written only from
that document (`crates/sion-spec`, proof stack B) checks every chunk
alongside the reference executor (stack A), and a World Proof verifies only
if both accept every chunk. `scripts/check-independence.sh` (run in CI)
checks that stack B never calls the reference executor's logic. Its
differential tests run thousands of chunks, random workloads, boundary
probes and tampering through both stacks. They found two defects in the
reference executor and several gaps in the specification, all fixed in
specification v1.1: a market clearing deadline near `u64::MAX` overflowed
when its batch sealed (any account could halt the chain), and a charter
window near `u64::MAX` made the reference executor disagree with the
specification. Both stacks share the hashing, Merkle, signature, Assurance
Algebra and clearing-certificate code (`impl:shared-primitives`).

**Proving market: not built.** The devnet runs one prover thread.

**Verifiable AI inference: built as claims.** The three tiers (proof,
attestation, bond) are expressed as claims whose formulas name their atoms
(`receipts` demo): a TEE quote is an unbonded atom with a known attack cost,
an operator bond is slashable. Zero-knowledge inference proofs are not
built.

### The Fold

**Simulated, with real bindings.** For every Pulse the devnet's prover folds
all chunk proofs into a World Proof (`sion_prover::world`) that checks the
chain of chunk statements from the Pulse's first commitment to its last,
the receipts root over every effect, the published data (it is exactly what
executed, and it encodes to the certified data-availability root), and every
lane certificate against its shard committee. Folding is a Merkle tree over
leaf statements, not recursive proof composition; the World Proof's size
grows with the Pulse's witnesses. A light client verifies it today by
re-executing every chunk (or a random sample of chunks).

### Knowledge layer

**Built.** Claims carry formulas over atoms and other claims (`sion-assurance`).
Cost of corruption is an interval computed per consumer policy: exact for
formulas whose disjuncts share no atoms, with a lower bound by an exact
solver when they do. The decision rule, reservations of `κ·V` against a
claim's backing, bonds with lock periods, challenges, adjudication,
slashing with compensation to the parties who relied on the claim, and
failure probabilities for insurance are implemented and property-tested.
Policies can trust or distrust atoms and classes, or assume an atom broken.

The assurance registry is a single contended object. Every claim, bond and
reservation operation touches it, so its witness grows with the registry.
That is fine at devnet scale and is the first thing to split (per atom and
per claim objects) for production.

### Verifiable Clearing

**Built.** Markets collect sealed orders during a batch window, seal with
the Pulse's beacon, reveal sealed orders with committee key shares, and
settle the first submitted clearing whose optimality certificate verifies
(`sion-lp`, `sion-clearing`): exact rational arithmetic, a linear-time
certificate check, beacon-derived priorities for canonical tie-breaking,
and a canonical price band. Sealed double auctions are implemented on the
ledger, and the market demo clears one at one price; assignment markets
(jobs to slots) exist in `sion-clearing` as a library, not on the ledger.

### Privacy and compliance

**Built:** claim payloads can be public, encrypted to an ML-KEM viewing key,
or replaced by a commitment; orders can be sealed to a market committee
(ML-KEM with Shamir t-of-n sharing, practical for small committees).

**Not built:** transfer amounts and balances are public. Confidential
transfers, private computation and compliance-by-proof (KYC, Travel Rule
and sanctions checks proven without revealing data) need a succinct
zero-knowledge stack. The claim model can carry such attestations today,
but only as issuer-signed claims.

### Interfaces

**Built:** the `sion` CLI (devnet, wallets, transfers, claims, bonds,
challenges, charters, markets, light-client verification, demos,
benchmarks), a JSON HTTP API (`sion_node::api`) and a Model Context
Protocol server (`sion mcp`) with tools to check balances, pay, issue
claims, price a claim with the Assurance Algebra before relying on it, pay
under an Agent Charter, verify a Pulse and read events.

**Built as well:** every transaction kind and every event reads as a
sentence (`sion_cli::describe`); the CLI prints what each command signs and
what happened; the MCP server has `sion_submit_intent` (a transaction the
agent signed itself) and `sion_fetch_receipt` (a receipt verified as a
light client would).

**x402: built.** `sion_cli::x402` implements x402 version 2 (the
`PAYMENT-REQUIRED`, `PAYMENT-SIGNATURE` and `PAYMENT-RESPONSE` headers)
with a Siontology `exact` scheme on network `sion:<chain>`: a resource server
quotes a price with a fresh nonce, the client signs a transfer or a
charter payment whose purpose names the quote, and the server's
facilitator submits it and serves only once the receipt verifies as a
light client would (committee certificate, certified order, inclusion
proof). Each quote and each transaction pays for one request. `sion x402
serve` and `sion x402 fetch` use it from the command line, and the MCP
tool `sion_fetch_paid` lets an agent buy HTTP resources within a cap it
sets and within its charter's limits. The adapter speaks the protocol's
headers and JSON; it has not been tested against third-party x402
clients, and EVM schemes are out of scope.

**x402 `stream` scheme: built.** A client that opened a payment stream to a
server pays each request with the next element of its hash chain instead
of a transaction; the server's `StreamDesk` checks it with one hash per
step, serves, and settles in batches (by checkpoint when it can), and
settles everything before it stops. A refused payment's response names the
server's position, so an honest client resynchronizes. Tested end to end
on a devnet (`crates/sion-cli/tests/streams.rs`): lost payments, escrow
refusals and top-ups, forged and replayed elements, misdirected
requirements, and the close-out. The desk keeps its tabs in memory: a
server that crashes loses what it had not settled. MCP's
`sion_fetch_paid` pays with `exact` only.

**Agentic commerce: ACP checkout and AP2 mandates: built (devnet
scale).** `sion agent sell serve` serves the Agentic Commerce Protocol's
checkout API at API version 2026-04-17 (`crate::acp`): the discovery
document, create, update, retrieve, complete and cancel, with ACP's
required headers, its idempotency rules (replay, conflict, in-flight, 24
hours, no 5xx kept) and its flat errors. It declares one payment handler of
its own, `org.siontology.ledger`, whose config is an x402 quote for the
session's total; completing the session settles the signed transaction the
instrument carries through the x402 facilitator, and the order is made
only after its receipt verifies against the pinned chain
(`agent:acp-checkout`). `sion agent buy` is the buyer: it checks the
session's total against its cap and the chain against the pinned one, and
pays from a wallet or under an Agent Charter. AP2 v0.2 (of 2026-04-28) is
in `crate::ap2`: SD-JWT (RFC 9901) with ES256 and the Delegate SD-JWT
chain, the merchant's and the payment side's verification, every
constraint AP2 defines (an unknown one fails), signed receipts, and the
shopping agent's issuing and closing of mandates. The merchant takes AP2
through an ACP extension of its own, `org.siontology.ap2`: each session
carries a checkout JWT the merchant signed, a mandate is checked before any
money moves, an open checkout mandate makes one order, and receipts are
returned (`agent:ap2-mandate`). Tested end to end on a devnet
(`crates/sion-cli/tests/commerce.rs`), and against the encoded tokens of
AP2's own specification, whose signatures and hash bindings check out
here (`crates/sion-cli/tests/ap2_vectors.rs`); not tested against another
implementation of either. Not built: card and wallet handlers (an agent
that knows only those cannot pay this merchant), ACP's request signing,
webhooks, delegated payment and authentication, 3-D Secure, the feed and
cart APIs; AP2's x5c roots, OpenID4VP delegation from a user credential,
and `payment.agent_recurrence`, which fails as unresolved. Sessions are
kept in memory.

**Not built:** adapters for ERC-8004 and C2PA.

**The app: partly built.** `sion-app-core` turns a device into a light
node: it keeps the device's key (`key.seed`, mode 0600), verifies the
latest certified Pulse every eight seconds the way `sion verify --no-world`
does (chain identity pinned on first use, committee certificate followed
from the pin, receipts, a state proof when the node is at that Pulse, 16
random data shares), proves the receipt of every transaction it signs
(waiting for the digest it computed, and showing only what the proven
receipt says), and pays (to 64-digit accounts only) and delegates stake
(v1.11) through `sion_cli`. It is `Send + Sync`, takes `&self` and holds no
lock while it talks to a node. It can also run a storage provider (v1.12)
that takes open deals on modest terms, fetches their data in the
background from the internet only (not from the device or its network,
unless told), keeps the data and proves every round, and that keeps
proving deals it holds when turned off. Settings are written atomically;
unreadable ones are kept aside and nothing is pinned on first use without
the person's say. It watches agents: those it backs, answers for or is,
found among the registrations and spawns the node keeps, and any added
by id, each one's charter and the overlays of its whole lineage proven
against one certified header and its status worked out from them by the
rule the node uses; it suspends, resumes or revokes those it oversees,
with proven receipts. Tested against a devnet over HTTP
(`crates/sion-app-core/tests/devnet.rs`). The Tauri 2 shell in
`apps/sion-app` (its own Cargo workspace) builds and runs on Linux; it was
run under a virtual display against a devnet and its tabs captured. The
Agents tab came later: the shell with it was type-checked
(`cargo check`), and the core it calls is tested against a devnet, but
it has not been run in a window.
**Not built:** the Android and iOS builds (configured, not attempted: no
SDK here), a key in the platform's keystore (`app:plain-key`), proofs of
the balances it shows (`app:unproven-balances`), bandwidth served by
devices, and installers or code signing.

### Installing and running

**Built.** `scripts/install.sh` installs `sion` from a checkout, from the
repository with cargo, or from a release archive checked against its
`SHA256SUMS` and, given an allowed-signers file, their SSH signature (run
here against a release made for the purpose: a signed release installed,
a wrong signer and a changed archive refused). `sion agent service` keeps
a long-running command as a systemd user unit or a launchd agent, its
tokens in a file only the user reads (`agent:service-tokens`; the files
are tested, systemd and launchd were not run here). `sion doctor` reports
what the machine has. The agent commands that keep only local state run
without a node. `sion setup` makes a chain of one's own (as a service, or
by a printed command), the wallets, the agent, its memory log, its model
(checked to answer; keys kept sealed, `agent:kept-credentials`) and a
profile, skipping what is done; bare `sion` talks with it and `sion -c`
continues the last conversation (an end-to-end test runs setup against a
chain and talks with a scripted model; the service path was not run, there
being no service manager here). `sion update` installs a release once its
signed sums and archive are checked (`release:signing-key`; tested against
a release served on this machine), and the release workflow builds Linux
and macOS archives on tags and signs them; it has not run, and the release
key does not exist yet. A devnet validator with nothing to order can wait
between rounds (`--idle-round-ms`): one at rest used 98.8% of a core
unpaced and 5.7% at 10-second rounds, measured on 2 cores. `sion agent up` runs a list of commands
and starts each again when it stops, its pause doubling to a minute, and
what it runs stops however it stops (tested for real); `sion deploy`
puts sion and an agent (or this machine's home, `--copy-home`) on a
server over SSH and runs `sion agent up` there as a user service (tested
against a stand-in for SSH on this machine: no server, and no service
manager, was reached). Trust atom `deploy:copied-keys`. The compose file
(`deploy/compose.yaml`) is checked for its syntax only, and the release
workflow's image job (`ghcr.io/<owner>/sion`, amd64 and arm64) has not
run. **Not built:**
the release key and a release, and code signing (macOS binaries are not
notarized). The `Dockerfile`, the Homebrew formula
(`packaging/homebrew/sion.rb`, building the head) and the Nix flake are
checked only for their syntax: this environment has no Docker daemon,
and building them would rebuild the workspace.

### Payments

**Payment streams: built (specification v1.6 and v1.7).** Not in the
architecture paper; added to test whether a ledger can settle payments at
billions a second without checking each one
([research note](book/src/research/billion-scale.md)). A payer escrows funds for one
payee and pays by revealing a SHA3-256 hash chain's elements off the
ledger; the payee settles with a walk (one hash per step, at most 65,536)
or, with checkpoints, a jump (at most `spacing + 21` hashes, however many
payments). Both proof stacks implement it, with differential tests for
every rule, a randomized property test and executor scenarios; the CLI
has `sion stream …`, the demo `streams`, and the bench group `streams`.
Measured on one core of the 2-core sandbox: about 2.2 million payee
checks a second, and settlements covering about 9 × 10⁸ payments a second
by checkpoint. **Simplified:** the fee of 2 micro-USD per hash prices
execution, not proving; payers build their chain up front (one hash per
step); unsettled payments are the payee's risk if it does not settle by the
stream's last Pulse; a stream pays one payee in one direction.

### Memory

**Memory logs: built on the ledger (specification v1.9).** Not in the
architecture paper; added so that agents' and robots' memories can be
verified ([concept](book/src/concepts/memory.md), [design
note](book/src/research/agent-scale.md)). A memory log is owned by its
writer; `CommitMemory` moves its head to a new size and root only along an
RFC 9162 consistency proof, which the ledger checks, so a log only grows
and cannot fork, and the commit's Pulse is the transaction time of the
atoms it adds. A log can belong to an agent identity, and then stops
growing while the agent, or an agent above it, is suspended or revoked.
Both proof stacks implement it, with differential tests for every rule and
a randomized property test of honest and forged commits.

**The off-ledger library, `sion-memory`: built.** Integer-only memory
atoms with space and two kinds of time; single-writer logs with an
on-disk store; a canonical 4D index, specified so that another
implementation can rebuild it (a second one, written from the
specification alone, agrees on random logs), with complete range proofs,
subject and link proofs, and omission proofs that convict an index that
leaves an atom out; belief as a fold with proofs checked from heads;
sealed payloads with forgetting by key destruction; the Reality Memory
Protocol. Measured over 10⁶ atoms on one core: a 50 × 50 × 10 m, one-hour
query returns 9 atoms with a 36 KiB proof verified in 0.2 ms; the index
builds in 0.78 s. Commands `sion memory …`, MCP tools `sion_memory_*`, and
the `robots` demo. **Simplified:** the index and manifest roots are stored
unchecked, and the ledger does not act on omission proofs (atom
`impl:memory-index`); the index is rebuilt for every head; logs are held
in memory. **Not built:** compact fraud proofs for other malformed
indexes, selective disclosure of an atom's fields, aggregators that commit
many logs in one claim, curated world memory beyond cell digests, and
storage with proofs.

### Orchestration

**Task Charters: built (specification v1.10).** Not in the architecture
paper; added for work bought across trust boundaries
([concept](book/src/concepts/task-charters.md), [design
note](book/src/research/agent-scale.md)). `PostTask` escrows money for a
task and fixes its acceptance rule: the poster's policy, the claims every
result must rest on, the verifiers whose attestations count, the
adjudicator, the largest detection factor and the reliance period. An
agent posts from its allotment under its caveats. `SettleTask` pays once
a result claim that commits to the task, and whose formula is exactly the
worker's bond or a named verifier's attestation of the same output and
every required claim, passes the decision rule; it reserves κ times the
payment of the result's backing for the payer, so a falsified result
compensates it. `CancelTask` refunds tasks released by their worker, open
ones and expired ones. Since specification v1.15 a task can require named
verifiers' attestations and how many (a bond alone then never suffices),
no result is attested by its author, and no named worker is one of its
task's verifiers. Both proof stacks implement it, with differential
tests for every rule and failure, including an exploit proven against an
environment claim and the compensation that follows; the CLI has
`sion task …`, the MCP server `sion_task_*` tools, and the demo `tasks`.
A task can also fix a check in its spec (the commitment to its
description and to a command and its files), kept from the worker:
`sion task verify` runs only the check committed, in a sandbox, and
attests an output only if it passes; agents post with one, submit with
the attestations a task needs, and release a task they cannot complete
(`task_release`).
**Simplified:** settlements reserve backing in the one contended assurance
registry, so they are ordered one after another. **Not built:** batches
of outputs checked by beacon-chosen samples, assignment markets on the
ledger, automatic slashing for two signed outputs of one deterministic
task, capability leases at gateways, dataset manifests.

### Economics

**Built:** fees (a base fee plus a fee per KiB) go to the treasury;
slashed bonds compensate the parties who relied on the claim and pay a
share to the challenger, with the rest to the treasury. Validators stake,
earn a share of the fees collected in each epoch by weight (70 % on the
devnet; slashing remainders and released pools are never paid out)
and are slashed on signed evidence of equivocation: two consensus headers
for one round, or two live lane approvals of one object version. The
whole stake and unclaimed rewards are forfeited; the reporter receives a
share (5 % on the devnet, so reporting oneself still costs 95 %) and the
treasury the rest.

**Pooled stake (specification v1.11): built.** Anyone may delegate stake
to a validator without handing it over: a delegation is an object its
delegator owns, holding shares of the validator's pool. Delegations join
at the epoch's end; rewards are split between a validator's own and its
pool's earning stake, the operator keeping a commission (at most 25 %,
raised by at most 2 points at a time once anyone has delegated, effective
at the epoch's end); a delegation may not give a validator more than twice
an equal share of the bonded stake; an equivocation slashes a tenth of the
pool, unbonding delegations included; validators rank by own plus pooled
earning stake. Both proof stacks implement it, and the differential tests
add randomized workloads checked against a third, plain-arithmetic reading
of the split. Commands `sion stake …` and MCP tools `sion_stake_*`.
**Simplified:** one shared object holds every pool, so delegation
transactions are ordered one after another; a pool slash is a flat tenth.
**Not built:** redelegation without unbonding, delegating from an agent's
budget, compounding.

**Storage deals (specification v1.12): built.** A client escrows payment
for keeping data whose Merkle root it fixes (`PostDeal`), for a named
provider or any; the provider accepts with collateral and gets a custody
record it owns; each round it proves, owned-linear and many deals per
transaction, the leaves the beacon of the round's first Pulse picks (the
deferred phase records the beacon of every eighth Pulse in one of 12,500
beacon records, §6.3); it collects per proven round; the close refunds
unproven rounds to the client and returns the collateral less
`⌊bond × missed / rounds⌋`, or forfeits all of it to the treasury if the
client closes a deal abandoned for three rounds. Both proof stacks
implement it, and the differential tests run every operation and failure
through both. Commands `sion storage …` (including `serve`, a provider
loop that proves, collects and closes, then accepts deals whose data it
fetched in the background on its terms), MCP tools `sion_storage_*`, the
`storage` demo (an agent's memory archive kept by a laptop; a phone that
kept half of its data caught and its collateral forfeited),
`GET /v1/deals` (`?open=1` from an index), and the app's Storage tab,
which turns a device into a provider (tested against a devnet; run on the
desktop build). Outbound HTTP goes through `sion-http`: TLS with
X25519MLKEM768 first, deadlines, bounded bodies, no redirects, no local or
private addresses. An independent review changed the challenge, the
forfeit, open deals' collateral and the provider before release
([Reviews](book/src/security/reviews.md)). **Not built:** proof of
replication (a provider may fetch challenged leaves from elsewhere:
`storage:no-replication`), retrieval the ledger enforces (`sion storage
retrieve` gets a deal's data from a provider that serves it, checked
against the deal's root, but nothing makes a provider answer:
`storage:no-retrieval`), repair or re-assignment of a failed deal, deals funded from an agent's budget,
encryption in the tooling (proofs publish the challenged leaves: a client
encrypts private data itself), and storage proofs aggregated across deals
(each round's leaves go on the ledger: `storage:raw-proofs`).

**Not built:** payments to provers, a value-in-flight cap.

### Node and operations

**Networking between machines: simplified.** `sion node` runs one
validator per process, with its own data directory and HTTP API, and
`sion gateway` runs a gateway on its own; nodes talk QUIC (`quinn`) with
TLS 1.3, the hybrid key exchange X25519MLKEM768 only and ML-DSA-44
signatures only (rustls with aws-lc-rs, `net:aws-lc`). Each node has an
ML-DSA-44 network key, separate from a validator's dual consensus keys,
presented as a raw public key (RFC 7250, not an X.509 certificate,
`net:raw-public-keys`) and pinned at both ends (`net:transport-ml-dsa`). Validators generate their
keys locally (`sion keys validator`) and sign a network record binding
their network key and addresses with both consensus keys; `sion genesis`
collects the records into a genesis file, and a peer file lists the
gateways validators accept (`net:static-peers`). Every message class has
its own stream with a size limit and a priority; queues are bounded per
peer and class; quotas drop floods. A consensus session is the epoch (no
restart generation). Gateways ask again for late lane approvals and
validators pull missing Pulse signatures, so a run survives lost
messages; the fault injector tests this in process. Tested on one
machine: four `sion node` processes and a `sion gateway` certify, order
and execute transfers, every node reaches the same state, a light client
checks a certified Pulse against the pinned chain identity, unknown keys
and classical key exchange are refused, three validators continue
without the fourth (`crates/sion-cli/tests/network.rs`). **Simplified:** a
validator that stops cannot rejoin (`net:no-rejoin`); a standalone gateway
trusts the node it follows (`net:gateway-trusts-node`); there is no drain
window at epoch boundaries (`impl:reliable-handover`), no lane-lock
hand-over (`impl:lock-handover`), no TCP fallback, no HTTPS on the API, no
stake-weighted bandwidth, and peers do not follow committee changes. Not
yet tested across machines, under `netem`, or by fuzzing the decoders.

**Simulated (devnet):** `sion devnet` runs every validator in one
process, each on its own thread with its own state replica, exchanging
real signed messages through the same `Transport` seam over in-memory
channels, optionally through a seeded fault injector (loss, delay,
duplication, reordering and partitions per message class); the client
gateway is a single component that collects lane approvals.

**Built:** a segmented write-ahead log with checksums and torn-tail
truncation, periodic snapshots, replay that re-checks every commitment,
restart into a new consensus session and state sync for validators that
fell behind (on a devnet, whose validators restart together; see
`net:no-rejoin` for networks), and epoch boundary proofs kept on disk for
light clients (`GET /v1/epochs/{e}`).

## Trust assumptions this implementation adds

These are atoms in the formula of anything the devnet reports, on top of
the architecture's own:

| Atom | What it means |
|---|---|
| `impl:replay-proofs` | World Proofs are checked by re-execution, so "proof finality" costs a verifier as much as executing; succinctness is assumed, not delivered. |
| `impl:shared-primitives` | Both proof stacks use the same hashing, Merkle, signature, Assurance Algebra and clearing-certificate code; a bug there is common-mode. |
| `impl:devnet-keys` | Devnet validator keys are derived from the chain name; anyone can derive them. A devnet secures nothing of value. |
| `impl:in-process-network` | `sion devnet` only: validators share a machine and a process; network faults are exercised only through the in-process fault injector and the consensus simulator. A network of `sion node` processes does not carry this atom. |
| `impl:reliable-handover` | At an epoch boundary each validator starts a new consensus session; a validator that has not reached the boundary finishes the old session from messages already sent, which the in-process network always delivers. A real network needs a drain period: on a network of processes, a validator that misses old-session messages at a boundary can stall. |
| `net:transport-ml-dsa` | Networks: transport authentication rests on ML-DSA-44 alone (each node's network key, presented as an RFC 7250 raw public key and pinned at both ends). A forger could impersonate a peer at the transport layer (delay, drop, eclipse), not sign consensus messages, which carry dual signatures. |
| `net:aws-lc` | Networks: the TLS provider is aws-lc-rs, C code outside the workspace's audit scope (the workspace's own post-quantum code is pure Rust). |
| `net:raw-public-keys` | Networks: TLS authenticates nodes with RFC 7250 raw public keys (an ML-DSA-44 SubjectPublicKeyInfo, signed for in the handshake's CertificateVerify), not the self-signed ML-DSA X.509 certificates the design proposed, and with no channel-binding fallback. Both ends pin the exact key, so no certificate path is validated; the handshake relies on rustls's raw-public-key support, newer and less used than its certificate path. |
| `net:static-peers` | Networks: peers come from a genesis file and a peer file of signed records, read when a node starts, not from the state. A validator that joins by staking is reachable only by nodes started with its record in their peer file. |
| `net:partial-synchrony` | Networks: liveness needs message delays to become bounded after some time (assumed by the architecture; named because a real network now exercises it). |
| `net:gateway-trusts-node` | Networks: a gateway running alone learns committees and answers state queries from one node it trusts. A lying node can make it ask the wrong validators or show wrong balances; it cannot get a transaction certified that a committee did not approve. |
| `net:no-rejoin` | Networks: a validator that stops cannot rejoin. Restarting in a session it signed in needs the consensus signing log, and catching up to a later session needs Pulse sync; neither is built, so it refuses to start rather than risk equivocation. Each stopped validator is a permanent fault; more than a third of the stake stopped halts the chain. |
| `impl:slashed-seat` | A slashed validator keeps its seat, and its weight in quorums, until the epoch ends. |
| `impl:active-floor` | Validators can leave until one remains: nothing keeps the active set large enough to tolerate a faulty member, so the operators of the other validators can leave a single validator forming every quorum. |
| `impl:evidence-scope` | Equivocation evidence is not bound to this chain: a validator's keys must not be used on another network. |
| `impl:lock-handover` | Certificates are not drained when a lane committee changes: certificate finality for owned transactions holds within one committee's term (an owner who double-signs could get the second spend certified by a fully replaced committee). |
| `impl:hash-chain-beacon` | The beacon is a hash-chain commit-reveal; the last validator to reveal can bias it by withholding (at the cost of being seen). |
| `impl:public-amounts` | Amounts and balances are visible to everyone. |
| `impl:memory-index` | The ledger stores a memory log's index root without checking it: a memory query proof is complete relative to that root, and nothing on the ledger yet acts on evidence that an index leaves an atom out. |
| `impl:unaudited-pq-crates` | The FIPS 203/204/205 implementations are unaudited pure-Rust crates. |
| `storage:no-replication` | A storage proof shows that the provider could produce the challenged leaves during the round, not that it keeps a copy of its own: it could fetch them from another copy (or from the client) when challenged. Two deals with two providers are not proof of two copies. |
| `storage:raw-proofs` | Every storage proof carries its challenged leaves and paths on the ledger, about 17 to 22 KiB a deal a round with 4 challenges. Proofs are not aggregated across deals or rounds, so ledger bandwidth bounds how many deals a network can keep. |
| `storage:no-retrieval` | Getting data back is not part of the protocol: a provider proves custody, and returning the data to its client is up to them. |
| `app:agent-search` | The agents the app finds for the device come from the node's history of events, unproven, and only the Pulses the node still keeps: a node can leave out an agent the device backs, and the Agents tab would not show it. The standing and budget of every agent it shows are proven. |
| `app:first-use-pin` | The app pins the chain identity of the first node it verifies (trust on first use) unless its user pins one first. A first use against a lying node pins the liar's chain. |
| `app:plain-key` | The app keeps the device's key unencrypted in its directory (mode 0600 on Unix), not in the platform's keystore. Anything that can read the directory can spend. |
| `app:unproven-balances` | The balances the app shows come from the node unproven; its own receipts and the Pulses it verifies are proven. |
| `agent:local-policy` | An agent runtime's autonomy level, approvals (those people give for the rest of a session included), grants, taint, token budgets and its stop for a model going round in circles are enforced by the runtime on the machine it runs on. Only the agent's charter (payees, limits, budget, expiry, suspension) is enforced by the ledger. |
| `agent:provider-sees-context` | A model provider receives the conversation it is asked to continue, tool results included. |
| `agent:plain-sessions` | Agent sessions, with their conversations and journals, are stored unencrypted under `$SION_HOME/agents/sessions`. |
| `agent:local-skill-checks` | The runtime on the agent's machine checks a skill's bundle against its release when installing it, and takes a skill out of use when its release is no longer active at the start of each run. A session already running keeps the skill until it ends, and installed files changed on disk afterwards are not checked again. |
| `agent:eval-runs` | Evaluation runs are not proven. A verifier's attestation that a version passes an evaluation set rests on its bond and the bounty's adjudicator; with a model that varies between runs, another run can differ. The ledger refuses an attestation by the version's author (specification v1.15), but not one by an account the author controls under another name: such an attestation is worth its own bond. |
| `agent:channel-token` | Whoever holds the machine's channel token can start and continue the sessions of every profile on the machine, approvals included; whoever holds a profile's own token, that profile's, and approvals only with a token made for them; always within each profile's autonomy and the agent's charter. Tokens are kept as their hashes. The channel speaks plain HTTP. |
| `agent:gateway` | An agent gateway sees, and could change, the messages and answers it passes; it cannot act as an agent on the ledger, holding no key. It routes a profile's messages only to a host that proved the key of the agent its tokens name for the profile, and checks the agent's standing at most five seconds old, from the node it trusts: a message can reach an agent suspended within those seconds. It speaks plain HTTP unless put behind TLS. |
| `agent:messenger-identity` | Slack, Discord, Matrix, Signal, Telegram, WeChat, Zalo and X say who wrote a message: a bridge's approvers are who the messenger says they are, and anyone who can write in an allowed chat talks to the profile. |
| `agent:telegram-chats` | A Telegram bot trusts Telegram for who writes in the chats it answers; in a group chat, every member speaks for its owner, approvals included unless `--approvers` names who may decide. |
| `agent:pairing` | An approved pairing code lets in the chat it was sent to, on that channel and profile, until revoked: the operator trusts that whoever gave them the code is the person they mean to let in, and the messenger for who writes in that chat afterwards (`agent:messenger-identity`). Codes are kept in a file only the operator's user can read. |
| `agent:irc-nicks` | On IRC, who wrote a message is the nick the server gives: on a network that does not hold nicks to accounts, anyone can take a nick that is free, approvals included where `--approvers` names nicks. |
| `agent:email-from` | An email is taken as from the address its `From` names only as far as the mail provider's `Authentication-Results` show (DMARC passing for its domain, or DKIM or SPF passing for a domain aligned with it), read from the topmost results or the topmost of the server `EMAIL_AUTHSERV_ID` names: the provider is trusted to write its results above everything a sender wrote, and to check what it says it checked. With `EMAIL_TRUST_FROM`, `From` is taken as it is. Alignment is taken as one domain being the other or under it, not by the public suffix list. |
| `agent:imessage-handles` | On iMessage, who wrote a message is the handle Messages shows, as Apple's service vouches for it; an SMS (through a paired iPhone) carries the number the phone network gives, which can be forged. imsg, run as the operator's user with Full Disk Access, reads the whole Messages database for the bridge. |
| `agent:service-tokens` | A service's tokens (`sion agent service install --env`) are kept in plain text in a file only the user reads (systemd's environment file, or the launchd agent): whoever can read the user's files can read them. |
| `release:signing-key` | `sion update`, and the installer, install a release only if its `SHA256SUMS` are signed by a key the program (or, for the installer, the repository's `scripts/release/allowed_signers`) names: whoever holds that key, a secret the release workflow signs with, can publish what they install. `ssh-keygen` checks the signature; the installer's first fetch trusts HTTPS and GitHub, and a build whose file names no key does not update itself. |
| `agent:webhooks` | A webhook (WhatsApp, LINE, SMS through Twilio) is taken as its service's when it carries the service's signature with the secret the operator gave. The signatures name no time, so a webhook copied on its way could be sent again later and is answered again once its id has left the last thousand remembered; the bridge listens on plain HTTP, and TLS is the operator's proxy's. |
| `agent:teams-tokens` | A Teams webhook is taken as the Bot Connector's when its token is signed by a key `login.botframework.com` publishes (fetched over TLS, kept a day), issued by `https://api.botframework.com` for the bot's app ID, within five minutes of its times, by a key endorsed for the activity's channel, and naming the activity's service URL, where the reply goes with the app's token; who wrote a message is who the Bot Connector says (`agent:messenger-identity`). |
| `agent:google-chat-tokens` | A Google Chat webhook is taken as Chat's when its token is signed by a key Google publishes for `chat@system.gserviceaccount.com`, for the app's project number (or, with the endpoint URL as audience, is Google's ID token of the sender named, for that URL), within five minutes of its times; who wrote a message is who Chat says. With a service account's key, the bridge signs its token requests with it: whoever reads the key file can act as the app. |
| `agent:dingtalk-stream` | DingTalk's messages come over a connection the bridge opens to DingTalk with the app's credentials, over TLS: who wrote a message is who DingTalk says, and a voice note says what DingTalk heard. Replies go to the session webhook the message names, which is DingTalk's, or after it expires through the robot's API. |
| `agent:feishu-token` | A Feishu or Lark webhook is taken as theirs when it carries the event subscription's verification token: a secret in each event's body, as Feishu sends it, not a signature, so whoever saw one event on its way could send others; unless the subscription has an Encrypt Key, with which events are encrypted and signed (SHA-256 over the time, a nonce, the key and the body) and one not signed with it is refused. The address's check, which Feishu does not sign, is taken when it decrypts with the key and carries the token. |
| `agent:talk-signatures` | A Nextcloud Talk webhook is taken as the server's when it is signed with the bot's secret (HMAC-SHA256 over a random value and the body) and names the server. The signature names no time: a webhook copied on its way could be sent again, and is answered again once its message has left the last thousand remembered. |
| `agent:wecom-callbacks` | A WeCom callback is taken as WeCom's when it is signed with the callback's token (SHA-1 over the token, the time, a nonce and the ciphertext) and decrypts with the `EncodingAESKey` to a message for the company. The signature names a time the bridge does not bound: a callback copied on its way could be sent again, and is answered again once its message has left the last thousand remembered. Who wrote it is who WeCom says. |
| `agent:qq-signatures` | A QQ webhook is taken as QQ's when it is signed with Ed25519 under the key both sides derive from the bot's secret: whoever has the secret can sign events as QQ. The bridge signs the address's check back with the same key, only a plain token and time, so that nothing it signs can pass for an event. Who wrote a message is who QQ says. |
| `agent:ntfy-topics` | ntfy names no writer: whoever may publish to a topic the bridge answers speaks for the operator there, approvals included unless `--approvers` names who may decide (ntfy names no one, so no one may). A topic on a server that asks who publishes, or one whose name nobody can guess, is the operator's to keep. |
| `agent:phone-callers` | A phone call is taken as Twilio's when each of its steps is signed with the account's auth token for the public address Twilio was given; who calls is the number Twilio gives, which the phone network carries and which can be forged. What the caller said is what Twilio heard, and the reply is what Twilio speaks. |
| `agent:synology-chat-token` | A Synology Chat request is taken as the NAS's when it carries the bot's token: a secret in each request's body, as Synology Chat sends it, not a signature, so whoever saw one request on its way could send others. Who wrote it is who the NAS says; replies go to the incoming URL the operator gives, on their own network. |
| `agent:simplex-client` | SimpleX's terminal client is trusted with the bot's profile: its WebSocket API asks no password, so any program on the machine that reaches its port acts as the bot, and the bridge must be the client's only program. Who wrote is the contact or member the client says, as it numbers them; SimpleX names no one beyond that. |
| `agent:nostr-relays` | Nostr's relays carry the messages: one can withhold or delay them, not forge or read them, as every event is checked against its signature and a private message against who sealed it. Who wrote is the key that signed. An older message (NIP-04) shows the relays who writes to whom and when; a private one (NIP-17) hides both. `NOSTR_PRIVATE_KEY` speaks for the account, and signs the bridge in to the relays that ask. |
| `agent:home-assistant` | Home Assistant's token (`HOME_ASSISTANT_TOKEN`) acts as the user it was made for: calling a service does whatever that user may, in the home itself. Calls are external calls, which autonomy L4 asks a person for; what Home Assistant says (names its users gave) taints a session as a web page does. |
| `agent:voice` | The services `--hear` and `--speak` name receive the voice notes and the replies spoken; the profile acts on what the transcription says was said, within its autonomy and the agent's charter. A `command:` provider runs as the operator's user, outside any sandbox. |
| `agent:local-scheduler` | Scheduled jobs run only while `sion agent scheduler` runs on the machine (or a system timer calls it); a missed run happens once, late. |
| `agent:work-gate` | Who said something on a watch is who the watch says: a GitHub login as GitHub gives it, a chat's user as the messenger names them. Whoever can post as someone on a watch's list can ask the agent for work as them. In a chat, a bot is whoever the watch's bot list names: a messenger that gives no sign of a bot lets another bot pass as a person. The conversation a piece of work came from receives what the agent says about it: on a public repository, everyone. |
| `agent:work-assessment` | Work nobody asked for is taken on when a model, reading the message and the operator's rules, says a rule covers it: in `auto` mode, a message written to persuade that model can be taken on under a rule. Such work runs as content from outside (its session tainted) and its mandate names the rules; `observe` mode records the model's judgements for the operator to read first. |
| `agent:work-messages` | What the agent reports goes to the conversation the work came from, as a reply would: the runtime checks each message's length and purpose, and that one question waits at a time, not what it says. A session that holds confidential content is not held back from these messages, as it is from external calls. |
| `agent:work-runner` | Work happens only while `sion agent work run` runs for its profile (one runner at a time), reading the watches by polling. It reads a messenger's updates itself: a bridge on the same messenger and account would see only part of them. A runner stopped while it worked goes on with the interrupted run in the same session, whose journal holds a payment made then to once; the work tools are not journaled, so a message may be sent twice. |
| `agent:keyless-signer` | A keyless subagent is contained by the runtime that holds its nearest keyed ancestor's keys: the ledger lets only that ancestor sign for it, and holds it to its own caveats, but a subagent that could make that runtime sign something else as the ancestor would act within the ancestor's caveats, not its own. The runtime signs only for the session's own tool calls. |
| `agent:sandbox` | Commands an agent runs (its shell, its skills' scripts, MCP servers started in a sandbox) are contained only as far as the kernel's namespaces (bubblewrap) or the container runtime (Docker) hold: an escape gives them the runtime's user. Bubblewrap bounds how long a command runs and the size of each process, not how many processes it starts; Docker bounds those too. A container started by a runtime that is killed runs until its command ends. |
| `agent:sandbox-network` | With the network allowed, a sandboxed command reaches what that network reaches. Bubblewrap's `host` network is the machine's own, unfiltered: local services and a cloud's metadata address included. The `declared` network gives commands none, and skills' scripts only what their skills declare, through the egress proxy (`agent:declared-hosts`). A Docker network restricts only as its operator set it up: the runtime refuses Docker's `host` and `bridge` networks by name and cannot check the rest. |
| `agent:shell-repeats` | Shell commands, skills' scripts and file writes are not journaled: a command the model gives again runs again, so with the network a repeated command can repeat an action outside the machine. |
| `agent:workspace-content` | What an agent reads from its own workspace (files, and the output of commands without the network) does not taint its session, even content it saved there from outside in an earlier session. |
| `agent:mcp-unsandboxed` | An MCP server added without a sandbox runs as the operator's user, with the environment variables its configuration names: it can do whatever that user can. |
| `agent:mcp-effects` | What an MCP tool's call does is what its server does. The runtime knows it only from the annotations reviewed with the tool, or from the effect its operator set: a tool that does more than it says (moves money elsewhere, say) is caught only by the review, and an agent's charter does not bound money a tool moves outside the ledger. A pin shows that a tool's definition is the one reviewed, not that its server's code is. |
| `agent:consolidation` | A consolidated note is a model's reading of the episodes it cites. The gates check that it cites episodes it was shown, that the notes it replaces exist, its kind and its size, not that it reads the episodes right. Each note links to its episodes and names the model and the consolidator, so it can be checked against them. |
| `agent:recall-local` | Session search reads the sessions kept on the machine the agent runs on. A hit's check shows whether a stored session still hashes to the digest its episode recorded; a session never recorded (it did not finish, or ran without a memory log) is searched unchecked. |
| `memory:archive-key` | A memory archive is encrypted under a key derived from its writer's wallet seed: whoever holds the seed can open it, and on a devnet anyone who knows a named wallet's name. A provider sees an archive's size, which log it is of, and when it was made. |
| `storage:retrieval-replay` | A provider checks a retrieval request's signature and expiry (at most five minutes ahead), not whether it was seen before: within that window a copied request retrieves the data again. Retrieval speaks plain HTTP unless put behind TLS. |
| `agent:storage-from-account` | A storage deal an agent posts is paid from its account's balance, the money it pays fees with, not from its charter's budget: the charter's caveats do not bound it. |
| `agent:skill-pre-approval` | A skill a session has read pre-approves the calls its `allowed-tools` names until the session ends: where the autonomy level would ask a person, they run. A rule naming a command's first words (`Bash(git:*)`) covers whatever that program can be told to do through its arguments, inside the sandbox. A pre-approval never covers a call the level refuses, spending, or a session that has read content from outside, and the agent cannot grant one, since it writes only a skill's name, description and instructions: it comes from whoever placed the skill, or from the operator's consent when installing its release. |
| `agent:session-allowance` | A person who approves a call may allow, for the rest of the session, the same call again, or the calls of its tool a rule matches (`shell(git status:*)`, as a skill's `allowed-tools` writes it): those run without asking. A rule naming a command's first words covers whatever that program can be told to do through its arguments, inside the sandbox. An allowance never covers a call the level refuses, spending, or a session that has read content from outside (and none is given while it has); it is given only by someone who may decide, kept with the session with who gave it, and withdrawn with `/allowed off`. |
| `agent:token-estimate` | A run's token budget is held to an estimate of what each model call sends (a token for every three bytes, pictures as some text would be) and the most it may answer; the tokens a provider reports are counted as it reports them. A provider that counts more tokens for the same request (some scripts, pictures) can take a run past its budget by the difference, and a subagent's tokens count once its run has ended. |
| `agent:quarantine` | In quarantine, content from outside is held out of the agent's conversation and read by a reader that has no tools, whose answer is held to a schema the agent gives. An answer of values (booleans, numbers, strings from an enum, strings of a strict format) does not taint the session, though it is only what the content made the reader say, and can be false; an answer with free text taints it. The strings an answer gives are matched in arguments exactly as given (four characters or more), so an argument the agent writes from one in another form is not caught. A browser's pages, a desktop's screens and the shell's output are not held. |
| `agent:verifier` | A check the operator gives (`--verify`) runs in the agent's sandbox, in its workspace, where the agent can change what the check reads or runs there (its tests, its scripts, a test runner's settings). The check's own files (`--verify-files`) are bound into the sandbox only while it runs, read-only, so the agent's commands neither see nor change them, though a tool that reads files outside the workspace (an MCP server of the agent's own) could. A check that passed shows that the command exited with status 0 then, not that the work is right, and a score is what the command printed. |
| `agent:task-check` | A task's check is fixed by the commitment in its spec when the task is posted, and `sion task verify` runs only the check committed, in a sandbox with no network, on a fresh copy of its files, attesting the output only if it exits with status 0. The ledger does not see the run: an attestation made without it (`sion task attest`) counts the same, so what keeps an attestation honest is still its verifier's bond and the task's adjudicator. A check passes what it was written to pass (one that looks only at an output's form passes a wrong answer); it is not handed to the worker, but whoever the poster gives it to can pass it on. |
| `agent:comparison` | A comparison (`sion agent graph eval`) measures a profile's models on the set's cases with their stand-in tools, not on its real tools, and its figures are what those runs did: its intervals treat every run as independent, though repeats of a case are not, so they are narrower than they would be over as many different cases. A template that beat both baselines on one set is offered by the router for any of the profile's tasks, as the model judges them, until a later comparison on that set says otherwise. |
| `agent:judge` | A judge (`sion agent judge`, `--judge`) is a model's judgment of each step, not a check: it can be wrong, and what the steps quote can steer it, though it is told that they are data. Its calibration is its agreement with four labelled steps it was not shown, which says little about steps unlike them. |
| `agent:graph-replay` | A graph's run is pinned to the digest of its graph in the run's own file, which catches a graph changed after the run began, not one changed with its digest; a replay answers each node with what the run recorded, so it shows how the graph routes those answers, not that its models would answer the same again. |
| `agent:read-only-node` | A read-only node (`read_only`) gets the tools whose declared effect is to read: a tool that declares less than its calls can do is not held back (`agent:mcp-effects`). |
| `agent:confidential-label` | A session holds confidential content only as far as it is marked: by `--confidential`, by `read_file` under the paths named, or by a subagent's report or a graph node's input from a session that held some. The shell, scripts, MCP tools and the browser can read the same data without marking it, and a tool the operator contained is not held. |
| `agent:signed-decisions` | A signed decision shows that the wallet's keys signed it as recorded, not who held them; decisions made in chat apps are not signed, and a signature names the call by its key and the digest of its input, which a compacted session may no longer show. |
| `agent:imitated-notes` | Only the runtime's notes as it writes them are marked in a tool's result, case aside; other words that claim the same are not. The mark informs the model: what holds a call is the policy, and approvals are kept in the session's state, never read from text. |
| `agent:attack-tests` | The attacks tested are the ones listed, against a scripted model that obeys every instruction it reads: they show what the runtime enforces without the model, not that no other attack gets through, and what got through in them gets through. |
| `agent:a2a-webhooks` | An A2A task's updates go to the webhooks its client registered, which the server checks only for being HTTPS addresses that resolve to public ones (unless the operator allows this machine's network): whoever holds a token that reaches the profile can have its tasks' states sent anywhere public, and a webhook is told the task as it stands, its reply included. |
| `agent:dream-quote` | A note a dream keeps rests on the words of its session it quotes: that they are in the session's transcript, as they are written there, shows that they were said in it (by the person, a tool or the agent itself), not that the note is true, nor that it says what they meant. |
| `agent:dream-test` | A dream's test runs each held-out case with the core memory as it is and with the dream's notes in mind, every note as if kept in view, and records the notes only if no case passes fewer times with them: it tests what its cases check and no more, and a model that differs from the agent's (`--eval-model`) tests that model. |
| `agent:curated-log` | A curated log is written only with its curator's key, so whoever holds that key decides what it says; the agents that keep it in mind (`--shared-memory`) see its notes as its local store on their machine shows them, and recall searches only their own logs. |
| `agent:declared-hosts` | The egress proxy holds a skill's scripts to the hosts their skill declares by the name a script asks for, at addresses that are public where they resolve on the machine. A declared host that forwards what it is sent, serves what others put there, or shares a front (a CDN) whose other sites a script can ask for inside the encrypted tunnel takes a script further. Behind an upstream proxy, a name the machine cannot resolve goes to that proxy, and its network decides where the name leads. With Docker, declared hosts are not filtered: the operator's network decides. |
| `agent:secret-store` | Skills' secrets are kept encrypted under a key in a file beside them, readable only by the operator's user: whoever can read that file can read them. A script that receives a secret can send it wherever its skill may connect, and can change it so that it is not cut out of what the model sees: the runtime removes the value as it is, not every encoding of it. |
| `agent:kept-credentials` | The credentials `sion` itself uses (models' keys and addresses, speech services' keys) that `sion setup` or `sion agent credential add` keeps are encrypted under the key in a file beside them, readable only by the operator's user: whoever can read that file can read them, as whoever can read a process's environment can read the variables it would otherwise be given. A variable set in the environment is used instead of a kept one. |
| `agent:mcp-remote` | An MCP server over HTTP runs elsewhere, under whoever runs it: it sees the arguments of every call made to it and answers what it likes. Over HTTPS its certificate shows only that it holds the name in its URL; plain HTTP, which the client allows to any address the operator gives, is as private as the networks it crosses. A header whose value names an environment variable (`${VAR}`) sends that value with every request. |
| `agent:mcp-oauth` | OAuth tokens for MCP servers are kept encrypted under the secrets' key (`agent:secret-store`): whoever can read that key can act at that server as the person who consented, with the scopes granted, until the tokens expire or are revoked, and a refresh token renews them without the person. The login trusts the authorization server the server's metadata names: it checks that the metadata names the server, that the authorization server's own names it as issuer and that its answer comes back with the state and the issuer sent, and holds discovery and tokens to HTTPS at public addresses (plain HTTP when the MCP server is on this machine, private addresses when it is at one), but an authorization server of an attacker's that a server names is caught only by the person reading the page they consent on. `logout` revokes tokens only where the authorization server takes revocations; elsewhere they stay valid until they expire. |
| `agent:browser` | The browser is Chromium (or Chrome), contained by bubblewrap's namespaces where it runs in them (its own sandbox is off there, as it cannot nest inside), with the machine's managed browser policies; elsewhere by its own sandbox, as the operator's user, and never as root. Without bubblewrap it reaches the egress proxy through a port on this machine's loopback, which any program of the machine can use while it runs, the runtime's upstream proxy and its credentials included. |
| `agent:browser-hosts` | A browser held to the hosts the operator names acts on their pages after reading them, as the autonomy level allows: a page there can lead the model to carry what it knows to that site, in an address, a form or a post, and a named host that forwards what it is sent, or serves what others put there, takes it further. Without named hosts, its actions after a page wait for a person. |
| `agent:browser-logins` | A login is filled only into a field whose frame is of the login's origin, as the browser knows it: a page of that origin can still send what is typed in it elsewhere, so a login is as safe as its site's pages. The password is cut out of what the browser returns as it is, not in every form a page could show it in. |
| `agent:desktop` | A desktop's programs are contained as its sandbox contains them, their address space unbounded under bubblewrap; the X server they share lets each read the others' windows and send them input. A Docker desktop reaches what its Docker network reaches. |
| `impl:batch-size-fee` | A batch member's fee floor splits the cost of checking signatures by the size its batch declares. A signer that declares 256 members and sends one pays about a fifth of the base fee for a signature check; validators bound this by checking cheap things first and taking batches whole. |
| `impl:insecure-sim-signatures` | Only when a devnet is started with `--insecure-fast-signatures`: validators sign with a simulation MAC. The chain identity records it, and light-client reports say so. |
| `agent:web-search` | A search's results are the provider's (Brave, Tavily, or the operator's SearXNG): it sees every query, and what it returns is taken as from outside, never as instructions. |
| `agent:mcp-input` | What an MCP server asks a person is asked in the conversation its session answers, and the next reply there of whoever may decide answers it; a form that asks for a secret is declined, but a server may still ask for one in words, and an address it gives may lead anywhere: the person opens it. |
| `agent:mcp-sampling` | A server allowed to sample has the operator's model write what it asks, within the limits set, at the operator's cost; it sees none of the session's conversation, but what it asks is the server's. |
| `agent:own-mcp-servers` | sion's mail, calendar and documents servers act with the credentials they are given, as their owner: mail sent is the person's own, and an event added is in their calendar. What the mail and calendar servers read is from outside; documents are read as a workspace's files are, not as from outside, so a document someone else wrote is trusted as the person's own. The calendar's login goes only to the server `CALDAV_URL` names. |
| `agent:browser-files` | What a page downloads is kept in the workspace as from outside; a file chosen for a page's field goes to that page's site, whoever runs it. |
| `agent:signed-in-browser` | A browser given a profile the operator signed in with acts on the sites it is signed in to as the operator: whatever those sessions allow, within the hosts it may reach and the autonomy level, and a site sees what the agent does there as the operator's. The profile's cookies and passwords are kept by Chromium's basic store, under a key Chromium does not keep secret: they are as safe as the profile's folder, readable by the operator's user. One browser uses a profile at a time by a lock that names the process holding it. |
| `agent:browser-handover` | A page handed to the person is served on 127.0.0.1 to whoever holds its token, which is in the question the person is asked (in the terminal, or in the chat a bridge answers): a process on the machine, or anyone who reads the question, can see the page and act on it until the person replies (in a chat, ten minutes at most). What the person types goes to the page as typed, and the model sees what the page then shows of it, and the reply. A captcha is told by the https address of a frame of its maker's that the page shows: a page can show such a frame to be handed over, and a captcha the page draws itself, or one inside another site's frame, goes unseen. |
| `agent:rented-enclave` | A renter's requests are read only inside the enclave the attestation names: its keys are made there and bound in a report the chip signs, and the owner serves the traffic sealed. It rests on AMD's chain and its chips (a flaw in either, or a key taken from one, undoes it), on the measured code itself, which a measurement says was launched but not that it behaves, and on the renter naming a measurement it trusts: without one, what the enclave runs is only its own word. A simulated platform rests on nothing and is taken only when the renter names its key outright. A rented session is stripped of the owner's memory, files and reach by the profile it is started from, not by the enclave. |
| `agent:metered-tokens` | What a renter pays is the token count the enclave signed with the key its attestation binds, so the owner cannot inflate it; but the count is the enclave's own reading of what its model provider reported, and a renter cannot check it against the provider. Payment runs one request behind, so the owner carries one request's tokens as credit, and a renter carries the risk that a request it paid for was served badly. |
| `agent:agent-record` | An agent's record is what the ledger's events show: tasks settled to it, tasks it was named for and released, and its delivered results later proven false. Work done off the ledger is not in it, a result nobody challenged counts as good, and the number a record is turned into, and the ranking built on it, are a reader's own weighing rather than the ledger's. |
| `agent:measured-improvement` | A change the agent keeps was better beyond chance on the tasks it was tried on and regressed on no held-out task it already passed. Both bars are only as good as those tasks: a set that does not cover what the agent is used for, or one written to flatter a change, passes it, and nothing here chooses the tasks. The evidence recorded is the counts and the interval, not the runs themselves. |
| `agent:vouched-memory` | A vouching is a claim on the voucher's bond, so who stands behind a shared fact costs something and can be ruled against; it says the voucher stands behind the fact, not that the fact is true. A revocation rests on the revoker's signature alone. A revoked fact and whatever rests on it are left out by the reader, not removed from any log, and only links the atoms actually carry (derived-from, assumes) are followed: a fact drawn from another without saying so is not pulled with it. |
| `agent:project-budget` | A project's budget is counted here, in the file that holds the project, not escrowed on the ledger: it bounds what the agent records spending against the project, and the charter and its allotments are what actually bound what it can spend. A project that is ended or out of budget takes no more work through the project, which is not the same as the agent stopping. |
| `agent:acp-checkout` | An ACP order is made only once the payment for the session's total has executed on the ledger and its receipt verifies against the chain the merchant pinned, so a merchant is not paid in words; what was bought is the merchant's to deliver, and nothing on the ledger says it was. A buyer is bounded by its cap and by its Agent Charter, not by the merchant. The handler is Siontology's own: the ACP specification names it as an extension point, and no other ACP party is known to speak it. The bearer token is the only check of who is asking. |
| `agent:ap2-mandate` | A mandate is taken as its user's because its root is signed by a key the merchant trusts by `kid`: an agent provider's trusted surface, which AP2 says the agent must not be able to use, and that is the provider's promise, not something checked here. What a closed mandate approves is checked exactly (the checkout this merchant signed, the payee, the amount); what the open mandate's constraints allow is evaluated by this code's reading of AP2 v0.2, a draft, and the budget is counted in memory by the merchant that was paid. |
| `agent:meeting-voice` | A guest given a voice speaks whatever the agent says into the meeting, to everyone in it. It answers any line that names it, wherever the name falls, so a name that is also an ordinary word is answered when anyone happens to say it, and a transcriber's mishearing can both miss a question and invent one. What it says rests on the meeting's own words, which are content from outside. |
| `agent:github-webhooks` | A webhook is taken as GitHub's when it carries GitHub's signature with the operator's secret; the listener speaks plain HTTP, and TLS is the operator's proxy's. A delivery copied on its way could be sent again, and is taken once by its event's id. |
| `agent:imported-agent` | What an import brings over from OpenClaw or Hermes Agent into a profile's instructions is that agent's own files, which it may have written from what it read; the plan shows them and names the patterns worth a look, and the block tells the model they are another agent's notes, but once applied they reach every session's prompt as the operator's instructions do. |
| `agent:taught-skills` | A taught draft names elements by what the pages called them and keeps the values the person typed outside secret fields; whether a field is secret is judged from its type, its masking, its autocomplete and what its name and label say, so a secret typed into a field that looks ordinary in every way is recorded. The draft reaches an agent only through the import's audit and the operator's acceptance. |
| `agent:x-mentions` | An answer to a mention is checked for citing the sources it was given and naming no other address, not for saying what they say: a model can cite a source for a claim it does not make, and a search's results are whatever the service ranks for words a stranger chose. The reply is posted as the account's own. |
| `agent:skill-import-review` | A skill imported from a folder, a repository or a registry is installed on the operator's reading of its files, or on an audit of its root; the patterns the report names, and a registry's verdicts, are a guide to that reading, not a scan that finds all that a skill could do; a registry's digests show that the files are the ones it lists, not that they are safe. |
| `agent:code-mode` | A program that calls tools runs in the shell's sandbox with no network and nothing of the machine mounted; its calls are limited to tools whose effect is to read, as they declare it. A tool that says it only reads but changes something, or reaches out, does so from code as it would for the model, up to 200 times a program. What the program prints is the model's own reading of what the tools returned. A tool that waits for a person (a page handed over) is not called from code, but an MCP server that asks the person during a call it serves from code keeps the program waiting for the answer. |
| `agent:hooks` | The operator's hooks run as the operator, unsandboxed, with each call's input (and an after-hook its result) as JSON: they see what the calls they match carry. What an after-hook or a run's start hook prints is shown to the model as the operator's. A before-hook guards only the calls it matches, and only as well as its command checks them. |
| `agent:worktrees` | A subagent's worktree is a checkout of its parent's repository: it reads all that HEAD holds, and what it commits on its branch is merged only by its parent. Git runs for it on this machine with its git directory named outright and hooks, the file-system monitor and signing off; filters the repository's own configuration names still run on what it wrote, as they would for the operator. |
| `agent:kept-reasoning` | What a model thought is kept in its session's file as its provider gave it: encrypted where the provider encrypts it (OpenAI's reasoning items, Anthropic's signatures and redacted thinking), and readable where it does not (Anthropic's summaries, a chat completions server's reasoning text). It is given back to whatever answers at the same kind of provider, base URL and model, so a base URL that is a proxy is given it too. Whether it is the model's own, unchanged, is checked by the provider, not here. |
| `agent:live-voice` | A live conversation takes what its model of speech (or its loudness) finds near the microphone as the person's speech: without a wake word, what anyone says there goes to the transcriber and to the agent as the person's words; with `--wake-cloud`, everything said there goes to the transcriber's provider. The echo canceller takes out what the loudspeaker was given, not other sound: a voice from another device is speech. With turns told by what was said, what sounds unfinished is held a second and a half, and what is heard next within it is taken as its rest, whoever says it. |
| `agent:realtime-voice` | Through a realtime model, what the agent is asked is the realtime model's account of what the person said, and what the person hears is its account of the agent's answer: it is told to pass both on faithfully and to answer nothing itself, which a model may not do. It sees the conversation's utterances and the agent's answers. |
| `agent:meeting-notes` | A meeting's transcript is what Twilio's transcription or the transcriber heard, which may mishear and, unless speakers are told apart, does not say who spoke (by phone and as a guest, all is `meeting`; here, `me` and `others`); speakers told apart are the provider's reading of the recording, named in the order they first spoke, not who they are, and that provider hears the whole recording, which stays on the machine until it is heard (or with `--keep-recording`, after). The notes are the agent's reading of the transcript. Dialling in joins the meeting as a caller, whom its other participants may not be told about: say so, as the law where they are may require. |
| `agent:meeting-guest` | A guest that joins a meeting by its link is shown to everyone in it by the name it is given, and acts on the meeting's page by what its buttons and fields are called: a page that calls a button so is clicked so. Its microphone sends silence and its camera black, as Chromium is told to fake them, and Chromium's own sandbox holds the meeting's page; what the meeting plays is heard as `meeting`, whoever speaks. |
| `deploy:copied-keys` | `sion deploy --copy-home` copies the agents' wallets and their keys to the server: whoever controls the server controls them. |

## Light-client trust

A light client pins one value: the chain identity (`sion_light::ChainSpec::id`),
a digest of the genesis specification, the genesis validators with their
stake and public keys, and the shard layout. That fixes epoch 0's
committee. Every later committee is in the state, and the client follows
it one boundary at a time (`sion_light::EpochChain`): the certificate of an
epoch's last Pulse, checked against the committee it already trusts, and a
state proof of the validator set under that header give it the next
epoch's committee and keys. World Proofs carry the validator set the Pulse
began with, proven against its starting state root, so lane certificates
are checked against committees derived from the state, not from anything a
node serves. Without a pin, `sion verify` says the node is vouching for
itself.

## Reviews and what they found

Three independent reviews, run by agents that had not written the code,
read the consensus-critical libraries, the node, and the validator
machinery, and confirmed their findings with tests. A fourth, the
networking plan, found two more gaps while reading the node. Everything
below is fixed, with a regression test, unless marked otherwise.

- **State machine and libraries.** An unbounded chain of claim references
  made cost-of-corruption and reservation traversals recurse once per link
  until a validator's stack overflowed (fixed: depth limits at issuance,
  64 MiB thread stacks); a reservation split overflowed 128 bits for large
  capacities (fixed: exact arithmetic); a clearing settlement used an
  unchecked multiplication; bounded totals and batches had no size limits;
  certificate values had no size limit (all fixed in specification v1.2).
- **Node, consensus and light client.**
  - The signature pre-verification cache keyed on the transaction body but
    not its signatures, so a stripped-signature copy could split replicas
    (fixed: the key covers the signatures).
  - Expanding a certificate group copied attacker-sized signatures per
    reference before any check (fixed: groups are shape-checked first, and
    header payloads are bounded).
  - Lane locks kept a copy of the whole request per input and were never
    released, which also froze inputs of lagging, rejected or expired
    transactions (fixed: one shared copy per request; locks are released
    when the transaction succeeds, fails or expires, a rejected one keeping
    them until it expires; requests from lagging
    validators wait for them to catch up; the gateway accepts resubmission).
  - Lane validation skipped the checks execution makes before charging a
    fee, so zero-fee transactions were published for free (fixed), and
    Pulse data was unbounded (fixed: header payload limits; validators keep
    the published data rather than the extended square, and never sign a
    header without an availability commitment).
  - Certificate finality did not survive a restart, which starts a new
    epoch (fixed: approvals are logged before they leave a validator and
    locks are restored after a restart, so no conflicting spend can be
    certified and the original can be certified again).
  - Light-client sampling used a fixed seed (fixed: fresh randomness); the
    client did not check the node's transaction list against the certified
    order, or a receipt's position (fixed); a missing World Proof did not
    fail verification (fixed).
  - Transactions and validator messages were not bound to a chain (fixed:
    transaction signatures cover the chain configuration, specification
    v1.2; lane batches and consensus headers name the chain identity, and
    votes cover it through the header digest).
  - Certified groups went to a single validator (fixed: f + 1 validators);
    far-future headers, header signatures and beacon reveals were retained
    without bound (fixed: windows and caps); equivocation evidence was
    accepted across epochs (fixed); a re-included certificate's later
    rejection could hide the real receipt (fixed: the first execution is the
    receipt); forged Pulse numbers could make an unpinned client overflow
    (fixed); the prover's job queue and the faucet queue were unbounded
    (fixed).
  - **Open:** fee-paying transactions with large bodies can still make
    Pulses slow, since data-availability encoding of the largest allowed
    Pulse takes seconds on one core; an account query scans every object.
- **Staking, epochs and committees (specification v1.4).**
  - An honest validator could be slashed: the node released a rejected
    transaction's lane locks at once, although its inputs kept their
    versions, and then approved a second spend that the `Approvals`
    evidence treats as equivocation while the first transaction is
    unexpired. An owner could frame every validator that approved one of
    its transactions (fixed: a rejected transaction keeps its locks until
    it expires; specification v1.5 spells out the rule).
  - If every active validator was slashed within an epoch, the boundary
    kept the old active set, so slashed validators with nothing at stake
    certified the next epoch (fixed in specification v1.5: they lose their
    seats regardless, and the chain halts).
  - A simulator default paid reporters 50 % instead of the documented 5 %
    (fixed).
  - **Open:** the active set can shrink to a single validator
    (`impl:active-floor`).
- **Networking plan** ([research note](book/src/research/networking.md)).
  - The gateway capped certificate groups at 512 transactions but not in
    bytes, and consensus drops an item above its payload limit without a
    reply, so a burst of dual-signed transfers never executed (fixed:
    groups are split by encoded size, at most half the limit; a
    certificate too large to fit alone is refused with a reason).
  - Beacon reveals committed ahead of their Pulse were kept only in
    memory, so a validator restarting alone would compute a different
    beacon (fixed in storage format 4: records and snapshots carry them,
    as of each Pulse's close). **Open:** Pulse headers do not commit to
    them yet, which a validator syncing from certified headers will need
    (a specification version).
