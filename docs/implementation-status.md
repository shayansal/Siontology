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
| Agents: a runtime that lets a model work as an agent identity, within its charter (beyond the paper) | Partly built (runtime with providers and a scripted model, memory, the agent's own skills, skills from others installed through bonds and audits, self-improvement with verified promotion, subagents, hiring, schedules and channels, a sandboxed shell, skills' scripts, MCP servers, storage deals) |
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
| Interfaces: x402, AP2, ERC-8004, C2PA adapters | Partly built (x402) |
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
2026-10-21), any OpenAI-compatible API, or a scripted model; fallback with
a 30-second cooldown; tools that declare an effect (read, write, external,
spend); autonomy levels L0 to L6 with approvals that expire after 15
minutes; taint for 24 hours after content from outside, during which
external calls and spending need approval; a journal that returns the
recorded outcome of a repeated call and never repeats one whose outcome is
unknown; a turn budget of 12, plus 4 per turn of progress, at most 32;
compaction; a kill switch; sessions that serialise, pause for approval and
resume; and trajectories in chat format. `sion agent run` gives it the
ledger as tools (balance, payments under the agent's charter, transaction
status), files in a workspace and public web pages
([Agents at work](book/src/guides/agent-runtime.md)). With a memory log,
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
([Skills from others](book/src/guides/skills.md)): a release is a claim
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
([Skills that get better](book/src/guides/skill-improvement.md)): an
evaluation set is cases with stand-in tools and checks, fixed by the
digest of its bytes; `sion skill evolve` shows a model the cases a
version fails and scores the variants it proposes, under gates (purpose,
size, novelty, no regression), keeping a Pareto front, and keeps the best
as a draft; a bounty is a Task Charter whose spec names the release, the
set and the cases to pass, with its verifiers, adjudicator and required
claims (such as the set's author vouching for it, `skill.evals/v1`) fixed
before any version exists; verifiers run the set themselves and attest;
the next release rests on the settled result and records it, and
`sion skill lineage` follows the evidence back. After a finished task
with at least five successful tool calls and no skill written, the
runtime asks the agent whether to write one, with only its skill tools.
Scripted models take conditions on the conversation, to stand in for a
model that follows a skill. Teams, schedules and channels
([Teams, schedules and channels](book/src/guides/agent-teams.md)): with
`--delegate`, an agent spawns subagents (a budget carved from its
allotment under attenuated caveats, fee money from its account, a session
of their own, two levels deep at most), and posts, follows and delivers
Task Charters; profiles keep how to run an agent under a name;
`sion agent schedule` and `sion agent scheduler` run a profile's task
once, every interval or on cron (UTC), and deliver its answer to a file,
a webhook or Telegram; `sion agent serve` (HTTP with a bearer token) and
`sion agent telegram` (long polling, chats allowed by id) carry
conversations, each a session, with approvals answered in the channel.
Commands, scripts and MCP servers
([Commands, scripts and MCP servers](book/src/guides/agent-tools.md)):
`--shell` runs `/bin/sh` commands in a sandbox (bubblewrap, or a Docker
image pinned by digest and never pulled) that writes only the workspace
and reaches no network unless the operator gives one (refusing Docker's
`host` and `bridge`); `skill_run` runs a skill's own scripts there, with
the network only for skills that declare it; `sion agent mcp add` reviews
an MCP server's tools and pins each by a digest of its definition, and a
session offers only tools that still match, with effects from the reviewed
annotations or the operator, in both eras of the protocol (2026-07-28 and
the handshake of 2025-11-25 and before); `--storage` lets an agent keep a
workspace file with storage providers and follow and close its deals.
Calls meant to run again (commands, scripts, file writes) are not
journaled, and a replayed outcome says so. The spawns of one turn go
together, in one `SpawnAgents` transaction and one transfer of fee money,
and their sessions run at the same time; their keyless payments are signed
in batches as they arrive. Tested with the scripted model and with
stand-ins for the providers' and Telegram's APIs: this environment reaches
neither. The sandbox tests run bubblewrap for real, and Docker with
`SION_TEST_DOCKER_IMAGE`. Not built: permissions that skills declare and
the runtime enforces, search over past sessions, consolidation, the memory
archive with storage providers, MCP over HTTP, and routing channels across
machines
([Reliable, self-improving agents](book/src/research/agents.md)).

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
and a canonical price band. Sealed double auctions and assignment markets
are implemented; the market demo clears at one price.

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

**Not built:** adapters for AP2, ERC-8004 and C2PA.

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
the person's say. Tested against a devnet over HTTP
(`crates/sion-app-core/tests/devnet.rs`). The Tauri 2 shell in
`apps/sion-app` (its own Cargo workspace) builds and runs on Linux; it was
run under a virtual display against a devnet and its tabs captured.
**Not built:** the Android and iOS builds (configured, not attempted: no
SDK here), a key in the platform's keystore (`app:plain-key`), proofs of
the balances it shows (`app:unproven-balances`), bandwidth served by
devices, and installers or code signing.

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
`storage:no-replication`), retrieval (`storage:no-retrieval`), repair or
re-assignment of a failed deal, deals funded from an agent's budget,
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
| `app:first-use-pin` | The app pins the chain identity of the first node it verifies (trust on first use) unless its user pins one first. A first use against a lying node pins the liar's chain. |
| `app:plain-key` | The app keeps the device's key unencrypted in its directory (mode 0600 on Unix), not in the platform's keystore. Anything that can read the directory can spend. |
| `app:unproven-balances` | The balances the app shows come from the node unproven; its own receipts and the Pulses it verifies are proven. |
| `agent:local-policy` | An agent runtime's autonomy level, approvals, grants and taint are enforced by the runtime on the machine it runs on. Only the agent's charter (payees, limits, budget, expiry, suspension) is enforced by the ledger. |
| `agent:provider-sees-context` | A model provider receives the conversation it is asked to continue, tool results included. |
| `agent:plain-sessions` | Agent sessions, with their conversations and journals, are stored unencrypted under `$SION_HOME/agents/sessions`. |
| `agent:local-skill-checks` | The runtime on the agent's machine checks a skill's bundle against its release when installing it, and takes a skill out of use when its release is no longer active at the start of each run. A session already running keeps the skill until it ends, and installed files changed on disk afterwards are not checked again. |
| `agent:eval-runs` | Evaluation runs are not proven. A verifier's attestation that a version passes an evaluation set rests on its bond and the bounty's adjudicator; with a model that varies between runs, another run can differ. The ledger refuses an attestation by the version's author (specification v1.15), but not one by an account the author controls under another name: such an attestation is worth its own bond. |
| `agent:channel-token` | Whoever holds the HTTP channel's bearer token can start and continue the sessions of every profile on the machine, approvals included, within each profile's autonomy and the agent's charter. The channel speaks plain HTTP. |
| `agent:telegram-chats` | A Telegram bot trusts Telegram for who writes in the chats it answers; in a group chat, every member speaks for its owner, approvals included. |
| `agent:local-scheduler` | Scheduled jobs run only while `sion agent scheduler` runs on the machine (or a system timer calls it); a missed run happens once, late. |
| `agent:keyless-signer` | A keyless subagent is contained by the runtime that holds its nearest keyed ancestor's keys: the ledger lets only that ancestor sign for it, and holds it to its own caveats, but a subagent that could make that runtime sign something else as the ancestor would act within the ancestor's caveats, not its own. The runtime signs only for the session's own tool calls. |
| `agent:sandbox` | Commands an agent runs (its shell, its skills' scripts, MCP servers started in a sandbox) are contained only as far as the kernel's namespaces (bubblewrap) or the container runtime (Docker) hold: an escape gives them the runtime's user. Bubblewrap bounds how long a command runs and the size of each process, not how many processes it starts; Docker bounds those too. A container started by a runtime that is killed runs until its command ends. |
| `agent:sandbox-network` | With the network allowed, a sandboxed command reaches what that network reaches. Bubblewrap's `host` network is the machine's own, unfiltered: local services and a cloud's metadata address included. A Docker network restricts only as its operator set it up: the runtime refuses Docker's `host` and `bridge` networks by name and cannot check the rest. |
| `agent:shell-repeats` | Shell commands, skills' scripts and file writes are not journaled: a command the model gives again runs again, so with the network a repeated command can repeat an action outside the machine. |
| `agent:workspace-content` | What an agent reads from its own workspace (files, and the output of commands without the network) does not taint its session, even content it saved there from outside in an earlier session. |
| `agent:mcp-unsandboxed` | An MCP server added without a sandbox runs as the operator's user, with the environment variables its configuration names: it can do whatever that user can. |
| `agent:mcp-effects` | What an MCP tool's call does is what its server does. The runtime knows it only from the annotations reviewed with the tool, or from the effect its operator set: a tool that does more than it says (moves money elsewhere, say) is caught only by the review, and an agent's charter does not bound money a tool moves outside the ledger. A pin shows that a tool's definition is the one reviewed, not that its server's code is. |
| `agent:storage-from-account` | A storage deal an agent posts is paid from its account's balance, the money it pays fees with, not from its charter's budget: the charter's caveats do not bound it. |
| `impl:batch-size-fee` | A batch member's fee floor splits the cost of checking signatures by the size its batch declares. A signer that declares 256 members and sends one pays about a fifth of the base fee for a signature check; validators bound this by checking cheap things first and taking batches whole. |
| `impl:insecure-sim-signatures` | Only when a devnet is started with `--insecure-fast-signatures`: validators sign with a simulation MAC. The chain identity records it, and light-client reports say so. |

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
