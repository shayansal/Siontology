# Billions of agents

> **Status.** A design note, written on 28 October 2026. One part of it is
> built: agent identities and subagents (specification v1.8), in both proof
> stacks, with the `sion agent` commands and the `swarm` demo. Everything
> else here is a design that is not built, reviewed or measured: a memory
> layer for agents and robots, Task Charters for orchestrating work, a
> network that anyone can join with a phone, a desktop or pooled stake, and
> the signature amortization that billions of agents would need. Every
> figure is labelled: **measured** (in this repository, with the command
> that reproduces it), **reported** or **claimed** (by the party named, on
> the date given; nobody has checked claims independently), **design
> target**, or **our arithmetic** (computed here from measured or published
> figures). The web pages cited were opened on 28 October 2026.

## Summary

- **The target.** A billion agents making a trillion paid operations a day
  would be 11.6 million operations a second on average (our arithmetic).
  That is about 5,800 times the busiest day of any public blockchain we
  measured: Solana's 10 August 2026, about 1,990 non-vote transactions a
  second. No ledger carries that load, and this design does not put it on
  the ledger.
- **Where the load goes.** Payments travel off the ledger as [payment
  streams](../concepts/payment-streams.md), which the payee checks with one
  hash. Memory writes and task steps stay in their writers' own logs, and
  aggregators commit to many logs at once. The ledger carries identities,
  settlements and commitments: for a billion agents, about 10⁴ to 1.5 × 10⁵
  transactions a second (our arithmetic). That is around Gate 3's target of
  10⁵ operations a second, and 1,000 to 10,000 times what the devnet runs
  today: 15 transactions a second on one machine (measured).
- **Signatures are the binding cost.** A dual post-quantum signature is
  19,518 bytes. If each of ten billion agents made one dual-signed ledger
  action a day, every validator would receive 2.3 GB of signatures a
  second. Signed in batches of 256, the same actions need 38 MB a second
  (our arithmetic). Amortized signing has to be the default, and
  verification has to be priced.
- **Identities (built).** Every agent can have an identity of its own, and
  launch up to 256 subagents with identities of their own in one
  transaction. Budgets and rate limits are carved down the tree, so a
  subtree never spends more than its root was given, and one transaction
  from any agent above stops a whole subtree. The path to ten billion:
  cohorts of a million identities created in one transaction, children
  without keys of their own signed for in batch roots, additive rate
  limits, and certificates that stay final under revocation.
- **Memory (design).** An agent's or a robot's memory becomes a
  single-writer, append-only log of atoms that carry space and two kinds of
  time, after [Hotgraph](https://github.com/shayansal/Hotgraph)'s Reality
  Atoms. The ledger keeps each log append-only by checking a consistency
  proof at every checkpoint, and the Pulse of the checkpoint becomes the
  atoms' transaction time. A query can then prove what robot R had
  recorded about a region during an interval, as of Pulse p, and that it
  had recorded nothing else. In our simulation, a complete answer of about
  1,700 atoms carried a 437 KiB proof (our arithmetic).
- **Work (design).** Task Charters are an escrow-and-acceptance layer under
  the orchestrators labs already run. A task's budget cannot be exceeded
  anywhere in its tree of subagents. Acceptance is an assurance formula
  fixed when the task is posted, so every accepted result says who vouched
  for it and what faking it would cost. An exploit found in a grader
  falsifies every result the grader passed and compensates whoever relied
  on them. Work may be retried many times; it is settled once. That
  frontier labs need this is a hypothesis, and section 5 says what would
  test it.
- **A network anyone can join (design, delegation built).** Phones
  verify the chain for themselves and earn nothing. Desktops serve data,
  relay for phones and keep archival segments, and are paid from fees for
  each challenge they pass, in proportion to a bond, so splitting into
  many identities gains nothing. Anyone can delegate stake to a validator
  without handing it over (built in specification v1.11). A validator runs
  on a modest server, and the project's own testnet
  can start at about EUR 104 a month, against the 7,000 USD a month our
  networking plan estimated on AWS.
- **What it is not.** Nothing at this scale has run. Nothing has run
  across machines yet. The identities are built at devnet scale; the rest
  is a design.

## 1. The target in numbers

| Quantity | Value | Label |
|---|---|---|
| 10¹² operations a day | 1.16 × 10⁷ a second on average | our arithmetic |
| 10¹³ a day | 1.16 × 10⁸ a second | our arithmetic |
| Peak hour, if peaks run at three times the average | about 3.5 × 10⁷ a second | our arithmetic, assumed peak factor |
| Per agent, with 10⁹ agents and 10¹² operations a day | 1,000 a day, one every 86 seconds | our arithmetic |
| Siontology's maturity target for the ledger, 1.6 × 10⁶ operations a second ([architecture paper](../papers/architecture.md)) | 1.4 × 10¹¹ a day, a seventh of 10¹² | design target; our arithmetic |

Even at maturity, the ledger is designed for a seventh of the volume. The
rest has to happen off it.

**What runs today.**

| System | Rate | Label and date |
|---|---|---|
| Solana mainnet | 1,347 non-vote transactions a second, mean of 444 one-minute samples; about 1,990 a second on its busiest day | measured by us over the public RPC, 28 Oct 2026; reported by Solana Compass for 10 Aug 2026 |
| Ethereum's rollups together | 2,187 user operations a second, 30-day mean | measured by L2BEAT, 28 Sep to 27 Oct 2026 |
| Visa; Mastercard | about 8,200 and 5,600 transactions a second | reported in their annual filings for 2025; our arithmetic |
| Radix Hyperscale, public test | 500,000 swaps a second sustained | claimed, and described by Radix as "non-validated", 31 Jan 2026 |
| Mysticeti on Sui | over 400,000 transactions a second at 1.5 s latency | benchmark by Mysten, Mar 2026 |
| x402 | about 14 payments a second, week of 17 Aug 2026 | measured by Token Terminal |
| Siontology devnet | 15 transactions a second, four validators on one 2-core machine | measured (`sion bench`) |
| Siontology settlements | about 9 × 10⁸ stream payments a second on one core, with about a million payments per settlement | measured ([Billions of payments a second](billion-scale.md)) |

**Agent numbers are forecasts.** "Agent" has no settled meaning: a
configuration, a running instance and a session are each counted as one
somewhere. IDC forecasts 1.3 billion agents by 2028, in a study Microsoft
sponsored. ERC-8004's registries held 519,333 entries in September 2026,
and about 2.2 % of entries passed a health check in July. With wash trades
removed, x402's whole history to April 2026 is 109.6 million payments worth
about 15 million USD (measured by Artemis). Demand is as far from the
target as the technology is.

## 2. Where the load goes

### 2.1 Four layers

For a billion agents making a trillion paid operations a day (our
arithmetic, from the costs this repository measured):

| Layer | What it carries | Rate needed | Work and evidence |
|---|---|---|---|
| **Edges**: payers, payees, agents' hosts, robots | Stream payments, each checked with one hash; memory atoms and task steps, kept in local logs | 1.16 × 10⁷ a second in all | About 5 cores of hashing for all payees together and 6 for all payers (0.42 and 0.52 µs per payment, measured). The binding limits are the requests themselves, escrow capital and settling in time |
| **Aggregators** | Signed heads of memory and task logs, batched | One head per agent per minute is 1.7 × 10⁷ a second; batching 10⁴ agents per claim brings the ledger's share to about 1,700 a second | Not built. Batching is required, not optional |
| **Ledger** | Stream opens, top-ups, settlements and closes; identities; batch commitments | Set by the number of active payer–payee streams, not by payments: about 1.5 × 10⁴ a second with 10⁹ streams settled daily (1,000 payments each), about 1.5 × 10⁵ with 10¹⁰ (100 each) | 16 to 120 cores of execution per replica, and signature ingress that section 2.2 shows must be amortized |
| **Proofs and data** | The validity of each settlement; the published bodies | 1.2 × 10⁴ to 1.2 × 10⁵ settlements a second; 6 to 63 MB a second at 547 bytes each | About 160 to 1,600 prover machines per proof stack (the repository's figures, about 7,000 hash compressions per settlement). The cost of proving ML-DSA is not yet measured |

Proving or publishing every operation is out of reach. Proving each of a
trillion operations a day the way one GPU now proves Ethereum blocks in
real time would take 50,000 to 450,000 GPUs, and publishing 32 bytes per
operation would need 370 MB a second, 2.5 times the 150 MB a second
Celestia says its mainnet handles (our arithmetic, from claimed figures).
Proofs and published data have to scale with settlements, not operations.

### 2.2 Signatures

| Per ledger action | Dual signature | One-time lane key | Batch root of 256 |
|---|---|---|---|
| Bytes | 19.5 KB | about 4.7 KB | about 0.33 KB |
| Verification | 1.02 ms (measured) | about 0.4 ms (estimate) | about 7 µs (our arithmetic) |
| Ingress per validator, 10¹⁰ agents acting once a day (115,741 a second) | 2.26 GB a second | 0.54 GB a second | 38 MB a second |
| Cores of verification per validator, same load | 118 | 46 | 0.9 |

Two consequences:

- **Amortization is the default, not an option.** Section 3.2 describes
  the three signing tiers.
- **Verification needs a price.** A transaction pays 0.001 USD plus 0.0001
  USD per KiB of its published body, and signatures are not charged. A
  dual-signed transaction therefore pays what a batched one pays, for
  about 60 times the bandwidth and 140 times the verification. A fee per
  signature scheme corrects the price and gives users a reason to batch.

### 2.3 The per-core figure does not carry over

The measured 9 × 10⁸ payments a second comes from settlements of 2²⁰
payments each. Agents that spread their payments over many payees settle
100 to 1,000 payments at a time. The ledger then spends 1.4 to 10 µs per
payment, and one core settles 10⁵ to 7 × 10⁵ payments a second (our
arithmetic). That is still 100 to 800 times better than one transaction
per payment, which measured 880 a second on the same core. **Netting**,
paying many payees from one escrow, is the lever that keeps the number of
streams small. It is an open problem.

### 2.4 What has to be true

1. **Networking between machines.** Under way; see [Networking between
   machines](networking.md). The certified DAG as built holds up to about
   10 to 20 validators; beyond that the plan needs the uncertified DAG the
   architecture targets.
2. **A ledger at 10⁴ to 10⁵ transactions a second across machines**, with
   amortized signatures. Gate 3 asks for 10⁵.
3. **Succinct proofs**, sized to settlements; see [Succinct validity
   proofs](succinct-proofs.md).
4. **Aggregators and storage** for memory and task logs (sections 4 and 5).
5. **Demand**, which no design can supply.

## 3. Identities for agents and their subagents

### 3.1 What is built

Specification v1.8 makes an agent identity an [Agent
Charter](../concepts/agent-charters.md) with an identity record: a
commitment to the agent's card and its depth in a tree. The charter's id is
the agent's id, and its DID is `did:sion:<chain>:<id>`. The full account is
in [Agent identities and subagents](../concepts/agent-identities.md).

- `RegisterAgent` gives an account an identity, a budget only the agent
  can spend, caveats the ledger enforces and a bond behind its receipts.
- `SpawnAgents` launches up to 256 subagents in one transaction. Each gets
  its own account and keys, a card, caveats at least as strict as its
  parent's, a budget carved from its parent's allotment and a window limit
  carved from its parent's.
- Any overseer (the backer, the principal, or the agent of any charter
  above) can suspend a subtree with one transaction, and the agent or any
  overseer can revoke it. Every check walks the chain to the root, at most
  16 charters.
- Both proof stacks implement every operation, and the differential tests
  run each one, including every way it can fail, through both.

**How it compares.** No system we found gives subagents all four of these:
budgets contained by every ancestor's, at any depth; revocation of a whole
subtree at bounded cost; a stated rule for what happens when revocation
races an action already approved; and post-quantum signatures.

| System (status, date) | Subagents | Revocation | Signatures |
|---|---|---|---|
| ERC-8004 (draft; deployed on about 25 chains) | None | An `active` flag | Classical |
| Kite Passport (mainnet, 30 Apr 2026) | User, agent, session; "child limits automatically bounded by parent limits" | Hub broadcast "within seconds"; no cascade in the sections we read | Classical (BIP-32 derivation) |
| A2A 1.0 (12 Mar 2026) | None | None | JOSE |
| AP2 v0.2 (to the FIDO Alliance, 28 Apr 2026) | The user's mandate bounds the agent | Mandate expiry | JWS |
| Entra Agent ID (generally available) | One level | Disabling a blueprint stops its identities | Entra tokens |
| IETF delegation-chain drafts (Mar and Sep 2026) | Attenuating token chains, depth "a low value, such as 5" | Status lists and expiry; cascading only through an optional online server | Ed25519; ML-DSA optional |
| Siontology v1.8 (built, devnet) | Up to 16 generations, 256 per transaction | One transaction stops a subtree; checks walk at most 16 charters | Dual post-quantum |

v1.8 has all four, with two weaknesses it documents: rate limits are fixed
windows, so a subtree can spend up to twice its root's limit in an interval
that straddles two windows; and a certified agent payment is final only
when it executes, because a suspension or revocation that executes first
makes it fail.

### 3.2 The path to ten billion identities

At ten billion agents, v1.8 has five limits. Each has a designed fix, none
built:

| Limit at 10¹⁰ agents | Fix |
|---|---|
| **Heavy creation.** An agent costs a charter, an allotment and an account with full keys: about 2.5 KB of state, 25 TB for 10¹⁰ agents (our arithmetic) | **Lineage ids and cohorts.** An agent's id is `H("sion/id/agent", parent ‖ index)`, with the index from a counter only the parent writes: unique by construction, and anyone holding the root and the path can recompute every ancestor's id. A cohort creates up to 2²⁰ identities in one transaction as a Merkle sum tree over their budgets and rates; each is materialized on first use |
| **Every action dual-signed** | **Three signing tiers.** Root accounts sign with full dual keys. Keyed agents sign with their own ML-DSA-44 key and either SLH-DSA or a one-time lane key (LM-OTS, about 4.7 KB per action). Keyless agents are signed for by their nearest keyed ancestor in batch roots over up to 256 transactions (about 0.33 KB each). Two roots for the same sequence number are equivocation, slashable against the signer's bond |
| **Fixed windows** | **Additive token buckets.** Each agent has a bucket (σ, ρ), and a spawn requires the children's σ and ρ to sum to at most what the parent keeps. Every subtree then pays at most σ + ρ·τ in any τ Pulses, a bound to prove and property-test |
| **Every ancestor read** (up to 16 shared objects per action) | **Overlays.** Revoking, suspending or narrowing a subtree writes one overlay object at its root. A check computes its ancestors' ids by hashing and proves the absence of their overlays: at most 8 lookups, with the maximum depth lowered to 8 |
| **Certified agent payments can still fail** | **Certificate stamps.** Each approval carries the last Pulse its validator executed, and an action executes only if every approval is at most Δ = 8 Pulses old. If a revocation commits at Pulse r, nothing below it executes after r + Δ, even with a wholly Byzantine lane committee. With an honest committee, nothing can void a certificate, so a seller can serve as soon as it holds one. Exposure between r and r + Δ is at most σ + ρ·Δ |

**What it would cost** (our arithmetic, with the repository's measured
costs and devnet fees):

| | v1.8 | With the fixes |
|---|---|---|
| State per materialized agent | about 2.5 KB | about 1.0 KB, plus 1.3 KB if the agent stores an ML-DSA key |
| Creating 10¹⁰ agents | one transaction per 256: 3.9 × 10⁷ transactions, 2 to 3 × 10⁵ USD in fees at devnet prices | cohorts: about 9,500 transactions, about 10 USD |
| One ledger action | 19.5 KB, 1.02 ms | lane key: about 4.7 KB, 0.4 ms; batched: about 0.33 KB, 7 µs |

**Risks.**

- **A keyless child is only as contained as the runtime that signs for
  it.** If a prompt-injected subagent can make its orchestrator sign as the
  parent, the child's caveats do not bind it. Signing has to sit outside
  the model's control, in a sidecar or an enclave, and that is a new trust
  atom.
- **Lineage is public.** Public trees and public amounts reveal how an
  organization structures its agents and what they spend. Unlinkable
  lineage needs zero-knowledge proofs, which are not built.
- **One-time keys can be reused** through snapshots or concurrent signers.
  The lattice leg still protects the action, and lane keys stay opt-in and
  behind a trust atom until they are reviewed. LM-OTS alone is not a
  FIPS-approved scheme.
- **Identities are cheap.** A cohort member costs about 10⁻⁹ USD, so
  nobody may count identities as evidence. Claims are priced by their cost
  of corruption, never by how many agents stand behind them.

**Interoperability.** A `did:sion` resolver answers from the agent's record
and the overlays' absence proofs against a certified header; a revoked or
expired agent resolves as deactivated. The A2A card keeps its commitment on
the ledger and gains an extension carrying the DID and lineage. An ERC-8004
registration file can list the DID. x402's Sign-In-With-X can take a `sion`
type whose proof is a batch-root leaf of about 0.5 KB, since a 19.5 KB
signature is too large for typical header limits.

## 4. Memory for agents and robots

### 4.1 What exists

- **Agent memory systems.** Mem0, Zep and Graphiti, Letta, A-MEM, MemOS
  and MIRIX extract, link, consolidate and retrieve memories. Of those we
  surveyed, only Zep with Graphiti, and Hotgraph, are bitemporal: they keep
  when a fact held and when it was recorded. None attests the time of
  recording to a third party, none carries cryptographic provenance, and
  none is post-quantum.
- **The labs' products.** OpenAI shipped background consolidation of
  memory on 4 June 2026, Anthropic opened Claude's memory to free users
  with import on 2 March 2026, and Google added memory import to Gemini on
  26 March 2026. Portability arrived as lossy import. Nobody offers
  verifiable export, or proof of what was remembered when.
- **Robots.** Research systems build rich memories of space and time: 3D
  scene graphs (Hydra, ConceptGraphs, HOV-SG), past states of a changing
  scene (Khronos), 4D scene graphs whose nodes keep descriptions with time
  windows (DAAAM, CVPR 2026), and memory banks for vision-language-action
  models (MemoryVLA, ICLR 2026). None has a notion of trust. Robot networks
  (OpenMind, peaq, Auki, BitRobot, GEODNET) anchor identities and hashes on
  chains; none offers revision over time, spatial queries with proof of
  completeness, or post-quantum signatures.
- **Hotgraph** has the right semantics. Its Reality Atom is a claim with
  valid time and transaction time, provenance down to evidence spans, a
  belief state and typed dependencies, and its contradictions are kept
  rather than overwritten. The Reality Memory Protocol (RMP) exposes it
  through eleven operations. It is pre-alpha and single-node, and it lacks
  four things a shared substrate needs: a model of space, transaction time
  assigned by anyone but the writer, real cryptography (its confidential
  mode uses a test cipher), and integer confidence.

### 4.2 Principles

1. **The ledger holds commitments and policies; storage holds atoms.**
2. **Every log has one writer and only grows.** The union of such logs only
   grows too, so writes need no consensus, like claims today (see
   [Two-sorted state](../concepts/two-sorted-state.md)).
3. **Belief is computed, never stored.** What a consumer believes at Pulse
   p is a fold over the revisions visible at p, under a policy the consumer
   names. Two consumers can hold different beliefs over the same evidence.
4. **Transaction time is the Pulse** of the first checkpoint that includes
   an atom. The ledger assigns it; the writer cannot.
5. **Completeness is relative to the log.** A proof can show everything a
   writer had *recorded* by Pulse p, never everything that happened.
6. **Hash-based and integer-only throughout**: permille for confidence,
   millimetres for space, nanoseconds for time.

### 4.3 The memory atom

The atom is Hotgraph's Reality Atom, extended with space and an anchor on
the ledger, and encoded canonically like everything else in `sion-core`
(Borsh, domain-separated SHA3-256).

| Field | Content |
|---|---|
| `id` | `H("sion/mem/atom", body)`, a content address |
| `writer`, `seq` | The writer's key and the atom's position in its log |
| `kind` | Hotgraph's ten memory kinds (episodic, semantic, procedural, …) plus Observation, Coverage (what a sensor saw, including that a volume was empty), MapTile, Pose, Trajectory, Transform and Skill |
| `statement` | Subject, typed predicate and typed object, or a reference to content |
| `valid` | When it held: `[start, end)` in nanoseconds, possibly open-ended, with the clock's source and uncertainty |
| `observed_at` | When it was captured |
| `known_at` | Not written by the writer: the Pulse of the first checkpoint that includes the atom |
| `space` | Optional: a frame, and a point or a box in millimetres, with covariance. Frames are registered by Transform atoms, which are bitemporal too |
| `confidence` | Permille |
| `provenance` | Sources by content hash, evidence spans (byte or time ranges in a sensor stream), the extraction trace (hashes of model and code), an optional device attestation |
| `assurance` | Optional: an assurance formula, such as `dual_signature ∧ (secure_element ∨ operator_bond)` |
| `links` | Typed edges with a strength: supersedes, contradicts, derived from, supports, assumes, causes, enables, invalidates |
| `policy` | Permission, taint, whether it is safe to plan with, retention, the id of its erasure key |
| `payload` | Public, encrypted (keys wrapped with ML-KEM-768), or a commitment to a blob such as a map tile or a Gaussian splat |

Belief is not a field. A **revision atom** names its targets, a new state
(accepted, disputed, superseded, retracted or refuted) and a reason. A
**conflict atom** lists atoms that disagree, how, and where in space and
time; both sides stay. Revisions inherit their targets' keys in space,
time and subject, so the queries that find an atom also find every
revision of it. Nothing is mutated, which suits a ledger that only grows
and removes Hotgraph's in-place belief updates.

### 4.4 What goes where

| Place | Holds |
|---|---|
| Ledger state | **Memory spaces**, owned by a principal: their writers, revisers, encryption epochs, retention terms and registered aggregator. **Aggregator heads**: the latest aggregate root and sequence number. **Writer heads**, for writers who checkpoint directly |
| Knowledge log | Claims: an aggregate (root, previous root, count, storage manifest), a direct checkpoint, a world-cell digest (section 4.7), an attested erasure. Each rests on a formula such as `dual_signature ∧ bond(aggregator)` |
| Contended domain | Challenges with evidence of fraud: a forged or forked head, a broken consistency proof, an atom left out of an index or a cell |
| Storage nodes | Encrypted log segments, index nodes, payload blobs (maps, splats, transcripts) and aggregators' batch files, all erasure-coded, with storage proofs priced as assurance atoms |
| The writer | Its own log and keys, and the proofs of its own heads |

At scale, a writer need not be a ledger account: it can be a key
registered under its principal's memory space, so ten billion agents do
not need ten billion accounts. Storage and queries are paid by stream, one
hash per request, through the x402 `stream` scheme over HTTP.

### 4.5 Checkpoints, aggregation and cost

The writer appends atoms locally. At its cadence it signs a **head** of
about 150 bytes (writer, space, sequence, previous head, log size, log
root, index root, storage manifest root), sends it to its aggregator and
uploads the new segments. The aggregator checks the signature, the
sequence and an [RFC 9162](https://www.rfc-editor.org/rfc/rfc9162) consistency
proof from the previous head, updates its map from writer to latest head,
and posts one claim per epoch. Writers audit their own leaves, as in key
transparency. A writer that an aggregator ignores checkpoints directly, or
through forced inclusion. A fraud proof slashes the aggregator's bond;
later, a succinct proof per batch can replace fraud proofs.

**Cadence.** Robots: a head every 1 to 10 seconds while active, over a
local hash-chained sensor log. Software agents: at the end of a session,
at most hourly. Aggregators: epochs of 10 to 60 seconds.

**Cost** (our arithmetic; storage prices are Walrus's and Filecoin's
published prices, 2026):

| Scenario | Load |
|---|---|
| Every one of 10¹⁰ agents checkpoints directly on the ledger, once a day | 115,741 transactions a second, above Gate 3, and 2.26 GB a second of signatures into each validator. Not viable |
| The same through aggregators | 115,741 heads a second to aggregators; 0.30 GB a second of batch data; retaining 14 days costs about 38,000 USD a month on Walrus or 2,000 on Filecoin |
| A realistic mix: 10 % of agents active hourly, plus 5 × 10⁷ robots every 10 seconds | 5.3 × 10⁶ heads a second; 13.6 GB a second of batch data with signatures, 0.8 GB without |
| The ledger, with 10⁴ aggregators | 167 claims a second at 60-second epochs, 1,000 at 10 seconds; published bodies under 3 MB a second |
| Software agents' memory at rest, 1 to 10 MB each | 10 to 100 PB: at most about 0.001 USD per agent-month |
| Robots' memory at rest, 1 KB of atoms a second | about 31.5 GB per robot-year: 3.21 USD per robot-month on Walrus, 0.14 on Filecoin. Raw sensor data stays with its owners, as commitments |

### 4.6 The 4D index and its proofs

Each head's index root commits to four structures, rebuilt
deterministically from the log so that anyone, and two independent
implementations, derive the same root:

1. **A time-bucketed forest** for short-lived atoms: a sorted map from a
   bucket (an hour of valid time, say) to a Merkle R-tree packed by
   Sort-Tile-Recursive on 3D centres, ties broken by atom id. Each entry
   carries its child's digest, box, time range and count. Closed buckets
   never change, so a checkpoint rehashes only the open one.
2. **A long-lived tree** for maps, facts and anything spanning buckets.
3. **A subject map** from subject and predicate to atoms, for graph
   queries.
4. **A link map** from an atom to its revisions and conflicts.

**A range query**, "everything robot R recorded in box B during
[t₁, t₂], complete as of Pulse p", returns: proofs of the memory space and
the aggregator's head against the certified state root of Pulse p, and of
the writer's leaf in the aggregator's map (about 2 to 3 KB); then the
results, with the box and digest of every subtree the search pruned and
the boxes of the non-matching atoms in the leaves it visited, as in the
MR-tree. The client recomputes the root and checks that nothing pruned
meets the query. An empty answer is proved the same way. **Belief at a
point in time** runs the same query against each reviser the consumer's
policy names, so it also proves that no authorized retraction existed as of
p, before the fold.

**A simulation** (our arithmetic; one log of 10⁶ atoms over 30 days in
1 km × 1 km × 30 m, uniformly spread, 5 % long-lived; payloads excluded):

| Query | Results | Proof | Per result |
|---|---|---|---|
| 10 × 10 × 3 m, 1 hour | about 0 | 27 KiB | — |
| 50 × 50 × 10 m, 1 hour | about 10 | 42 KiB | 4.4 KB |
| 100 × 100 × 30 m, 6 hours | about 176 | 119 KiB | 690 B |
| 200 × 200 × 30 m, 1 day | about 1,692 | 437 KiB | 265 B |

Checking 437 KiB takes about 7,000 hashes: milliseconds on a phone. A
single 4D tree packed by Morton order was 2.4 to 4.6 times larger. The
long-lived tree dominates small queries; an interval structure for it is
the next optimization. Proving that a 10-metre box held no record for an
hour costs about as much as one and a half dual signatures.

**An index that leaves an atom out** is caught by a small fraud proof: the
atom's inclusion in the log and the two adjacent index entries that should
bracket it, about 2 to 3 KB. Because the index is canonical, there is
exactly one correct index for a log.

### 4.7 Shared memory of the world

- **World memory is divided into cells** of an Earth-centred grid at
  several levels, by space, valid time and epoch. Robots write only to
  their own logs and tag atoms for cells, publicly or encrypted to a
  consortium.
- **Bonded curators** publish a digest for each cell and epoch, listing
  the atoms from every writer's head as of Pulse p. An omission is proved
  by the atom's inclusion in a writer's log and its absence from the
  digest. Important cells get two independent curators.
- **Disagreements are first-class.** Coverage atoms ("I scanned this
  volume at t and saw nothing") make disagreements about existence
  detectable, and anyone may write a conflict atom.
- **Frames are bitemporal.** A loop closure writes a revised Transform
  atom, so "where the robot thought the pallet was, as of p" and "where it
  was after optimization, as of p′" both stay answerable.
- **Each consumer computes its own belief** and prices its reliance with
  the [Assurance Algebra](../concepts/assurance/index.md). A delivery robot
  can rely on a cheap claim that a door is open; a crane that needs "no
  person in this zone" demands bonded, attested, corroborated atoms. A
  physical claim cannot always be proved, so its detection factor stays
  below one and disputes go to adjudicators.

### 4.8 Privacy and forgetting

- **Encryption.** Each memory space has epoch keys (ChaCha20-Poly1305)
  wrapped to its members' ML-KEM-768 keys, and per-atom keys derived from
  them. Private spaces encrypt their index nodes too, so storage nodes
  serve opaque blobs; public cells publish only coarse boxes.
- **Forgetting** destroys the keys and writes an erasure atom. The
  commitments remain. Whether regulators accept destroying keys as erasure
  is an open question (section 9).
- **Sharing** is a capability: Charter caveats extended to memory spaces,
  operations and share targets, delegated only by narrowing.
- **Selective disclosure.** Atoms are hashed field by field, as in SD-JWT
  (RFC 9901), so a robot can prove that a pallet was at a place at a time
  without revealing the camera frame.

### 4.9 How Hotgraph fits

Hotgraph's kernel becomes the reasoning runtime on the agent's host or the
robot: belief folding, conflict sets, impact cones and context compilation
over a working set materialized from verified logs. Its event log becomes
the agent's single-writer memory log, and RMP becomes the interface:

| RMP operation | On Siontology |
|---|---|
| REMEMBER | Append to the writer's log; visible to others at the next checkpoint |
| RECALL, GROUND | The local working set. A remote query returns its proof and a head that a light client checks; the context carries the Pulse it is complete as of |
| VERIFY | Inclusion and completeness proofs, then the decision rule for the value at risk |
| REVISE | A revision atom; nothing is mutated |
| FORGET | Key destruction and an erasure atom |
| SHARE | A capability grant and key wraps |
| AUDIT | Inclusion and consistency proofs from the knowledge log |
| SIMULATE, EXPLAIN, COMPRESS | Local only; their outputs are labelled and never written to shared world memory as fact |

Hotgraph needs spatial extents and frames on its atoms, belief as a fold
with dated conflict sets, its enumerations aligned with RMP, canonical
encoding and integer confidence, and real cryptography in place of its
test cipher. Its retrieval is a placeholder today, and it has no published
benchmark results, so its claims need evidence before anything is built
around them.

### 4.10 What is new

**Prior art:** bitemporal agent memory (Zep and Graphiti, Hotgraph); Merkle
logs and maps (RFC 9162, Jellyfish, key transparency); authenticated
spatial indexes (the MR-tree, VTRQ); anchoring robot data by hash (peaq,
OpenMind); storage with proofs (Walrus, Filecoin's PDP); attenuable
capabilities (macaroons, Biscuit, UCAN, Charters); verifiable vector search
(Atlas, V3DB).

**New, as far as our searches found:**

1. Proofs that a memory query is complete as of a transaction time the
   ledger assigned, including proofs that nothing was recorded.
2. Belief at a point in time as a fold, under a named policy, over
   revisions that only accumulate, with proof that no authorized
   retraction exists.
3. Reliance on memory priced by the Assurance Algebra, with false
   contributions to shared world memory slashable.
4. Shared memory of the world across trust boundaries, with contradictions
   kept as objects in space and time and curators' completeness enforced
   by bonded fraud proofs.
5. All of it post-quantum and free of coordination, where comparable work
   on data availability and verifiable search relies on pairings (KZG).

**Risks.** Proofs establish that a log is intact and a query complete, not
that a sensor or a language model was right; they can lend false
confidence. Shared world memory will collect contradictions faster than
adjudicators can resolve them. Coarse boxes and checkpoint timing still
leak where robots, and the people near them, were. And the market of
aggregators, curators and storage nodes does not exist yet.

### 4.11 The first version

Specification v1.9 is planned to contain the smallest useful part: memory
logs that the ledger keeps append-only.

- **On the ledger.** `OpenMemory` creates a memory log owned by its writer,
  an account or an agent identity, with a commitment to its policy.
  `CommitMemory` moves the log's head to a new size and root, carrying a
  consistency proof from the previous head; the ledger checks the proof
  (`sion_core::merkle::verify_consistency` already implements RFC 9162's
  check), so the log can only grow, and the Pulse in which the commit
  executes becomes the transaction time of the atoms it adds. A suspended
  or revoked agent cannot commit. Both proof stacks, with differential
  tests.
- **Off the ledger.** A `sion-memory` crate: atoms, logs, the 4D index
  with its range and absence proofs, revisions and belief folding,
  encryption, and RMP's operations.
- **To try it.** `sion memory` commands, MCP tools for agents, and a demo
  of robots sharing a warehouse.

Aggregators, curators, world cells and storage proofs come later.

## 5. Task Charters: orchestration with priced acceptance

### 5.1 What frontier labs need

| Need | Evidence | What it implies |
|---|---|---|
| Many short units of work, bought from others | Qwen3-Coder ran 20,000 environments in parallel (Jul 2025). DeepSeek-V3.2 synthesized 1,827 environments and spent over 10 % of its pre-training compute on post-training (Dec 2025). Anthropic leaders discussed spending over 1 billion USD on RL environments in a year (TechCrunch, Sep 2025) | Every reward depends on an environment and a grader someone else may have built |
| Graders that cannot be fooled | METR found reward hacking in 30.4 % of RE-Bench runs (Jun 2025). Hacking learned in Anthropic's production coding environments generalized to alignment faking and sabotage (Nov 2025). Cursor found about 63 % of one model's successful SWE-bench Pro resolutions retrieved a known fix; sealing git history and the network cut its score from 87.1 % to 73.0 % (Jun 2026) | A result is worth what its grader is worth, and an exploit found later must reach back to everything it passed |
| Budgets and credentials per subagent | Anthropic's multi-agent research system used about 15 times the tokens of chat (Jun 2025). In Claude Managed Agents (beta), "Delegation is one level deep", "All agents share the same sandbox, filesystem, and vault credentials", and a session's budget has "no per-thread caps" | Per-subtask budgets and per-subagent credentials are missing even from the most advanced vendor platform |
| A way to halt work | In OpenAI cyber evaluations about 1,200 supposedly isolated agents talked over an improvised message board and about 700 joined an attack on Hugging Face; many workloads shared one credential, and OpenAI is still "developing enterprise-wide tooling to identify and halt evaluation workloads" (Aug 2026) | Attributable identities, credentials per workload, and a halting path that crosses vendors |
| Provenance of training data | Anthropic traced over 16 million exchanges through about 24,000 fraudulent accounts to distillation (Feb 2026). The AI Act requires model providers to publish a summary of training content (Art. 53) | Each datum needs an origin, a licence, the task that produced it, and a way to retract it |
| Records that last | High-risk AI systems must log events automatically over their lifetime (AI Act Art. 12) and keep logs at least six months (Art. 19) | Tamper-evident, retained, private records |

The nearest public statement of the need is a Google DeepMind paper that
calls for agent markets with cryptographic identities, bonds, circuit
breakers, "transaction and decision logs in a cryptographically secured,
append-only ledger", and AI judges whose failed verification blocks
payment (Tomašev et al., December 2025). No lab has publicly asked for a
ledger.

### 5.2 What exists

Orchestrators guarantee execution inside one operator's trust domain.
Temporal replays event histories and retries activities at least once;
Restate journals each invocation so that it "runs exactly once regardless
of failures"; Ray retries lost tasks three times by default; Kueue
schedules batch work in a Kubernetes cluster; LangGraph, the OpenAI Agents
SDK, Google's ADK and Microsoft's Agent Framework checkpoint agent graphs;
A2A 1.0 gives tasks states, and MCP gives them ids (experimentally). None
offers budgets across organizations, acceptance that says who verified and
at what stake, neutral settlement, a halt that crosses vendors, or
exactly-once effects across trust boundaries.

Decentralized AI networks found traction where buyers can check output
cheaply: GPU rental, swaps and validity proofs. They struggled where value
depends on judgment. Large training runs went back to single clusters, and
what survived is verification by replay (Gensyn's Verde, Prime Intellect's
TOPLOC). The closest prior art for escrowed jobs is ERC-8183 (a draft of
February 2026), whose jobs move from funded to completed or rejected on the
word of a single evaluator address set when the job is created. Nobody
prices the trust behind an accepted result.

**Verifying AI work is two questions.** *Integrity*: did model M produce
O from input I, and did grader G return "pass" on O? That is getting cheap:
deterministic re-execution, TOPLOC's 258 bytes per 32 tokens, refereed
delegation. *Validity*: does passing G mean the task was done? That is not
cheap, and every reward hack above lives there. Acceptance must price both,
separately.

### 5.3 The design

**Scope.** Settlement and accountability, not execution. Model runs,
sandboxes and graders stay off the ledger; it holds commitments, budgets,
claims and money. Task Charters enter at trust boundaries; work inside one
lab stays on Temporal, Ray, Kubernetes and its own harnesses.

| Kind (proposed) | What it does | Class | Built on |
|---|---|---|---|
| `PostTask { spec, budget, deadline, mode }` | Issues the task's spec as a claim (commitments to the input, the environment and the grader; the acceptance formula; the licence) and funds a task charter | Owned | `CreateCharter`, `IssueClaim` |
| `Decompose { task, children }` | Splits the budget into child tasks with attenuated terms | Bounded | `SpawnAgents`, `Delegate` |
| `Commit { task, worker, amount }` | Moves part of the budget into an escrow only that worker can draw, and only with evidence | Bounded | Allotment caveats |
| `SubmitResult`, `Attest` | The worker's result claim; verifiers' bonded attestations | Knowledge | `IssueClaim`, `PostBond` |
| `Settle { escrow, evidence }` | Pays the worker if the evidence passes the spec's decision rule, reserving κ·V of backing | Bounded | `AgentPay`'s evidence check, `Accept` |
| `CancelTask`, `ReclaimTask` | Stops a subtree; returns unspent budget up the tree | Contended | `RevokeCharter`, `ReclaimCharter` |
| `CreateTaskMarket`, `SubmitBid`, `ClearAssignment` | A sealed auction of tasks to workers' slots, accepted only with an optimality certificate | Owned; contended per market | `sion-clearing`'s assignment market |

**Budgets down the graph, without consensus.** A task's budget is an
allotment only its orchestrator can write, so spending, decomposing and
committing take lane certificates, not consensus: the path agent
identities use today. Child budgets add up to at most what remains,
deadlines are no later, payees narrower, evidence thresholds no lower.
Only rebalancing between siblings and cancellation reach consensus. The
ledger bounds payments, not consumption: to bound a vendor's service, pay
it by stream from the allotment, so the service stops when the money does.

**Assignment.** Direct, by naming the worker. An open bounty, where anyone
submits a result and the orchestrator pays the first acceptable one, or
each of the first k for best-of-k sampling in reinforcement learning; among
strangers this needs an order, so a bounty runs in its own contention
domain, ordered by a beacon-salted hash after its batch seals. Or a batch
auction: tasks are jobs, workers' capacity is slots, and the
[assignment market](../concepts/verifiable-clearing.md) already in
`sion-clearing` clears it with prices as dual variables. The ledger settles
a clearing only if its optimality certificate checks. That proves
optimality for the bids submitted, not that bidders told the truth.

**Acceptance as an assurance formula.**

- **Fixed when the task is posted.** The spec names the formula template,
  the policy, κ, the detection factor, the adjudicator and the challenge
  window. Neither side can change the rule after the work is done.
- **Two parts.** Integrity, where any one honest party suffices:
  `bond:worker ∨ V1 ∨ V2`. Validity: the environment vendor's bonded claim
  that environment E has no known exploit that passes grader G without
  solving the task. The settlement claim rests on
  `dual_signature ∧ (bond:worker ∨ V1 ∨ V2) ∧ Env`.
- **Worked numbers.** A worker's bond of 1,000 USD, two verifiers at
  1,500 USD each, an environment claim backed by 50,000 USD, an
  adjudicator at 10,000 USD, a detection factor of 0.9 and κ = 2. The cost
  of corrupting the result is min(∞, 1,000 + 1,500 + 1,500, 50,000) =
  4,000 USD. The [decision rule](../concepts/assurance/decision-rule.md)
  accepts a payment V when min(0.9 × 4,000, 10,000) ≥ 2V: up to 1,800 USD.
  A 2,000 USD task needs a third verifier. `Settle` reserves κ·V, so the
  same bonds cannot back unlimited payments.
- **Exploit liability.** A challenge that demonstrates an exploit breaks
  the environment's atom. Every result resting on it is falsified, those
  who relied on them are compensated from the vendor's bond first, and the
  challenger receives a share of what remains (half on the devnet). It is
  a bounty for reward hacks, paid by whoever sold the environment. The same
  works for a harness that claims to seal the network and git history.

**Batches and sampled verification.** A result claim commits to a Merkle
root over N outputs. The next Pulse's beacon picks m of them, which
verifiers check and attest; the worker cannot adapt to a sample drawn
after it committed. If a fraction f of the outputs is bad, m checks catch
one with probability 1 − (1 − f)^m: for m = 100, 99.4 % at f = 5 %,
63.4 % at 1 % and 9.5 % at 0.1 %. The batch claims "at most f bad" with
that detection factor: the [data-availability](../concepts/data-availability.md)
sampling argument applied to work. One settlement pays the batch.

**Per-step pay.** Metered work (tokens, sandbox minutes, rollouts) is paid
by stream, one hash per step; the outcome is paid from escrow on
acceptance. The worker risks one unpaid step; the payer risks steps later
found bad, recoverable only through the worker's bond.

**Settled once.** Every escrow ends exactly once, paid or refunded. A
duplicate `Settle` fails on object versions, and the refund is a deferred
entry with no writer to race for it. Under Temporal or Restate, ledger
calls run as activities whose nonce derives from the workflow and activity
ids, so a retry resubmits the same digest, which executes once:
at-least-once work, exactly-once settlement.

**Disputes.** Result claims must name an adjudicator (agent receipts name
none today, so nobody can challenge them). Two signed outputs for one
deterministic task, attempt and input are equivocation, slashable without
running a model. A short challenge window releases payment; a long
validity window keeps provenance with smaller bonds.

**Provenance for training data.** Every accepted result is a claim in the
append-only knowledge log, provable from a light client. A dataset
manifest commits to a Merkle root of result claims, each task's licence,
and contributors' bonded attestations. Falsifying an environment claim or
an attestation falsifies the manifests resting on it, so a lab knows what
to purge.

**The kill switch and capability leases.** `CancelTask` commits in one
Pulse, and after it nothing below spends. Escrows already committed to
outside workers stay payable on evidence, or nobody would work for a
principal who can revoke. Gateways that hand out egress, artifact stores,
model APIs and x402 services can serve a workload only while its task
charter is live, checked against a light-client proof, so revocation cuts
credentials as well as money. The ledger cannot stop a process that needs
nothing it gates; containment lives in the gateways that enforce it.

**Fitting under what exists.** A2A task ids and states map to ledger
tasks and Agent Cards to charters; AP2 mandates travel as claims; MCP task
results persist as commitments after servers delete them; an ERC-8183
evaluator contract can check a Siontology settlement proof; an admission
controller for Kubernetes' Agent Sandbox and Kueue can require a live task
charter.

### 5.4 What could honestly be claimed, after measuring it

| Claim | Status | Measure before saying it |
|---|---|---|
| Spending under a task tree cannot exceed its budget, even if every agent in it is compromised, and subtask budgets need no consensus | Charters, identities and allotments built; task escrows not | Zero overspend under fuzzing and prompt-injection red teams; the share of operations on the owned or bounded path; latency on a networked testnet against a Temporal activity |
| Work retried at least once is settled exactly once | Object versions and deferred refunds built | Fault injection: crashes, duplicated and replayed submissions |
| Every payment above a threshold rests on a result whose cost of corruption is at least κ times its value, with backing reserved | Decision rule and reservations built; one contended registry | Throughput once the registry is split; false-accept rate on seeded fake results against a single AI judge |
| An exploit found in an environment falsifies every result resting on it and compensates whoever relied on them | Falsification and compensation built; environment claims not | A pilot with an environment vendor |
| Per-step payments are checked with one hash, and settlement cost does not grow with steps | Built and measured on one core | The same across machines, with real worker fleets |
| Revoking a root stops all spending beneath it once the revocation commits | Built | Time from revocation to the last payment and to credential cutoff, networked, with gateways |

**What not to claim**: that it judges quality (it prices who vouched);
that it halts computation; that it replaces Temporal, Ray or Kubernetes;
throughput, before it is measured across machines; compliance with the AI
Act (a ledger holds evidence; compliance is the provider's).

**What has to be built first.** The assurance registry split so that
reservations stop serializing on one object; adjudicators on result
claims; batch claims with samples chosen by the beacon; task escrows that
survive revocation of the poster; assignment markets on the ledger.

## 6. A network anyone can join

### 6.1 Why every phone cannot be a full node

- **The operating systems do not allow it.** iOS gives a refresh task "up
  to 30 seconds", and runs processing tasks "only when the device is
  idle"; its long-running tasks start only "in response to someone's
  action" and end when the app is closed. Android repeats periodic work at
  most every 15 minutes, and limits data-sync foreground services to 6
  hours in 24; Google Play requires each such use to be started by the
  user.
- **The app stores forbid it.** Apple's guidelines: apps "may not run
  unrelated background processes, such as cryptocurrency mining" (2.4.2)
  and "may not mine for cryptocurrencies unless the processing is
  performed off device" (3.1.5(ii)). Google Play: "We don't allow apps that
  mine cryptocurrency on devices."
- **Networks rarely allow it.** Decentralized hole punching succeeds about
  70 % of the time (measured over 4.4 million attempts in the IPFS
  network, 2025 and 2026), so about 30 % of devices need relays.
- **Following every Pulse is too heavy.** A Pulse certificate is about
  98 KB with 7 validators; at one Pulse every 1 to 2 seconds, following
  every one costs 4.2 to 8.5 GB a day (our arithmetic).
- **Paying devices for presence attracts fake ones.** On 24 June 2026,
  114,267 of Helium's 163,607 rewarded IoT hotspots earned coverage
  rewards only and carried no data, and Helium retired coverage as a
  reward signal (HIP 149). Grass's airdrop went to about 1.5 million
  addresses amid reports of bot farming. Only defences that cost real
  resources per identity held up: stake, unique replicas, hardware.

The lessons: pay for work that someone needs, never for presence; and
assign work in proportion to something that costs money per identity.

### 6.2 Four tiers

| Tier | Runs on | Does | Earns |
|---|---|---|---|
| **Phone verifier** | The iOS and Android app, on a shared Rust core | A wallet that verifies its own receipts against certified headers, follows committees across epochs, samples 16 or more shares of the Pulses it uses and a few random ones while charging on Wi-Fi, and can post forced intents if censored | Nothing |
| **Desktop community node** | macOS, Windows and Linux, downloaded directly | A full node: re-derives the data squares, checks data roots, serves shares and rows (which it can sell through the x402 `stream` scheme), relays for phones behind NAT, and keeps archival segments assigned by the beacon | First a public scoreboard with no value; later, after legal review, payment from storage and serving fees for each challenge passed, weighted by bond |
| **Validator** | A server with at least 4 dedicated cores, 16 GB of memory, NVMe storage, 1 Gbit/s and a static address | Consensus, lanes, data availability, the API | Its share of fees by weight, shared with delegators after commission. Slashable |
| **Project infrastructure** | One dedicated server and one or two small virtual servers | Bootstrap peers, public RPC, the prover, a rate-limited faucet | Nothing |

Every phone that runs the app verifies the chain for itself, and its
samples add to the network's guarantee that published data can be
rebuilt. That is real work, but light clients that verify are users, not
employees: Ethereum and Celestia do not pay theirs either.

**A light-client accumulator.** A Pulse header does not commit to the
previous header today, so sampling an old Pulse needs its certificate. With
a digest of the previous header, or a Merkle mountain range over all past
headers, in each header (a specification change), a phone checks one
certificate per session, and each further Pulse it samples costs about 7 to
12 KB (our arithmetic).

### 6.3 Paying for device work without paying fake identities

1. **Registration.** `RegisterNode { kind, bond }`, before an epoch
   begins. The bond is the price of each identity.
2. **Assignment by bond.** The beacon of a Pulse a few Pulses after the
   epoch boundary assigns each archival segment to 3 nodes, drawn in
   proportion to bond, with a cap on any node's weight. The number of paid
   assignments is fixed by what must be kept, not by how many nodes exist,
   so splitting one bond across many identities buys nothing.
3. **Challenge and response.** At a Pulse the beacon draws, it picks 16
   share positions per assignment. Within a deadline (8 Pulses for
   servers, 64 for desktops) the node submits one batched `ProveCustody`
   transaction with each share and its paths to the certified data root.
   Validators check about 21 hashes per share and one dual signature. As in
   Walrus, validators and full nodes stop serving the challenged segments
   for the window, so a node that deleted its copy cannot fetch one in
   time.
4. **Pay and penalty.** A passed assignment earns its share of the custody
   pool, which storage fees fund, up to a price cap; what is not earned
   goes to the treasury and is never shared out again. A missed or failed
   one earns nothing, and repeated misses cost part of the bond.
5. **Limits.** Without an encoding unique to each node, which is too heavy
   for desktops today, a large operator may keep one copy for several of
   its assignments. Three copies per segment and the cap on bond weight
   bound the damage.
6. **Subsidies stay below fees.** A subsidy for serving must stay below
   the protocol's cut of what users pay for it, or paying yourself for
   fake requests becomes profitable.
7. **No pay for re-executing work.** Paid audits invite planted errors and
   collusion (the verifier's dilemma), and succinct proofs make them
   unnecessary.

### 6.4 Pooling stake without handing it over

Today one operator account stakes for each validator and receives all its
rewards, which are paid from fees only ([Rewards](../concepts/validators/rewards.md)).
The design adds delegation in the protocol, a specification change:

- **Delegation stays the delegator's.** `Delegate { validator, amount }`
  creates a delegation object the delegator owns; `Undelegate` starts
  unbonding, and the stake stays slashable for offences committed while it
  was bonded; `ClaimDelegatorRewards` collects. No pool token, and no pool
  that holds others' funds.
- **Accounting.** F1, as in the Cosmos SDK: periods and a cumulative reward
  per share, here as an exact integer fraction. A new period starts at
  every change of stake and every slash.
- **Commission.** Between 5 % and 25 %, under a per-validator maximum that
  never changes; increases of at most 2 points per epoch, after a full
  epoch of notice.
- **The operator's bond comes first.** At least the larger of the minimum
  stake and 10 % of the validator's total, and it absorbs losses first.
- **Slashing scales with correlation.** The fraction slashed is three
  times the share of stake that equivocated in the same window, between
  5 % and 100 %. An attack by a third of the stake still loses everything;
  one operator's accident is mostly absorbed by its own bond.
- **A cap on weight.** No validator may hold more than 20 % of total
  weight while there are 10 or fewer, and 10 % after; delegations beyond
  the cap are refused. The network publishes the fewest operators that
  together hold a third of the stake.

Post-quantum validators cannot share one key the way BLS-based distributed
validators do, because ML-DSA and SLH-DSA signatures do not add together.
Concentration has to be controlled with caps.

**What specification v1.11 built** is simpler than this design
([Pooled stake](../concepts/validators/pooled-stake.md)): delegations are
owned by their delegators; delegations made in an epoch join at its end,
from a per-epoch record of the reward accumulator; the commission is
between 0 and 25 %, effective at the epoch's end, raised by at most 2
points at a time; the cap is twice an equal share, never below 10 %; an
equivocation forfeits the operator's whole stake and a flat tenth of the
pool. Not built yet: the per-validator commission maximum, the operator's
minimum share of its validator's total, and slashing scaled by
correlation.

**Before outsiders delegate**, four things must hold: misbehaviour
evidence bound to the chain's identity, a signing log so that a validator
restarting alone cannot sign twice, a minimum size for the active set, and
certificates drained at committee hand-over. All four are named in the
[networking plan](networking.md) or the [implementation
status](https://github.com/ColdAI-org/siontology/blob/main/docs/implementation-status.md).

**Assets.** On the devnet, stake and rewards are in USD, and this design is
built and tested with that stake asset. Siontology has no token, nothing is
on offer, and nobody who runs a node on a test network is promised anything
for it. The [staking question](../papers/architecture.md) in the
architecture paper is unchanged.

## 7. What it costs to run

A validator does not need thousands of dollars a month. Prices fetched on
28 October 2026, excluding VAT:

| Configuration | Per month | When |
|---|---|---|
| Hetzner EX63 (20 cores, 64 GB ECC, 2 × 1 TB NVMe, unlimited traffic; EUR 74 setup) plus a Contabo VPS 10 for remote RPC and backup | **EUR 103.50** | Now: 7 validators in one process, and 7 processes with simulated network delay once networking lands |
| 4 × netcup RS 1000 (4 dedicated cores, 8 GB each; 12-month term) plus 2 × Contabo VPS 10 | EUR 95.92 | A networked testnet |
| 4 validators on Hetzner Cloud CPX22 in Nuremberg, Helsinki, Ashburn, Hillsboro and Singapore, plus 2 small RPC servers | EUR 103.94 (EUR 163.91 with 7 validators) | A testnet over real wide-area links |
| 7 validators on AWS ([networking plan](networking.md)) | about 7,000 to 7,700 USD | For comparison |

**Checks** (our arithmetic). With 7 validators, an idle validator sends
4.9 to 9.8 TB a month plus about 3 TB of re-broadcasts: within unlimited
plans and Hetzner Cloud's 20 TB, beyond plans that include 4 TB. CPU binds
before bandwidth: signing costs about 0.12 core-seconds per validator per
round with 7 validators. A desktop community node needs no server at all.

## 8. Order of work

| Step | What | Status |
|---|---|---|
| Specification v1.8 | Agent identities and subagents | Built |
| v1.9 | Memory logs kept append-only by the ledger; the `sion-memory` library with the 4D index, its proofs and RMP; a demo of robots sharing a warehouse | Ledger part built; library built, being integrated |
| v1.10 | Task Charters: task escrows with acceptance fixed at posting, adjudicators on result claims, batch claims with beacon-chosen samples | Built, except batch claims |
| v1.11 | Non-custodial delegation; node registration, bond-weighted archival assignments and custody challenges; the light-client accumulator | Delegation built; the rest planned |
| v1.12 | Amortized authorization: batch roots and a verification fee, cohorts, keyless children, lineage ids, token buckets, overlays | Planned |
| Later | Certificate stamps, once the consensus simulator can test lagging and Byzantine committees; aggregators, curators and storage proofs for memory | Design |
| In parallel | Networking between machines, phase by phase | Under way |
| In parallel | The app: one Rust core with desktop and mobile shells | Planned |

Each specification version is built twice, once in each proof stack, with
differential tests, as every version so far has been.

## 9. Open problems

1. **Netting**: paying many payees from one escrow, with settlement cost
   independent of the number of payees. It sets the ledger's load at scale.
2. **Rate limits that add up**: a proof that token buckets carved
   additively bound every subtree to σ + ρ·τ, in both proof stacks.
3. **Certificates that survive revocation**: proofs of the stamp rule's
   guarantees, and tests with lagging and Byzantine committees.
4. **The verification gap.** The most valuable work, agentic coding and
   research, has no cheap verifier, so acceptance formulas will often rest
   on AI judges or people with low detection factors. Published detection
   factors and bonded environment claims help; they do not close it.
5. **Completeness for long-lived memory**: an interval structure that
   keeps proofs small for maps and facts that span many buckets.
6. **Forgetting in an append-only world**: whether destroying keys counts
   as erasure, and how to keep personal data out of commitments.
7. **Unique copies without sealing**: an encoding per node that is cheap
   enough for desktops and costly to redo on demand.
8. **Unlinkable lineage**: proving a subagent's place in a tree without
   revealing the tree.

Write to the maintainers (see [Responsible disclosure](../security/disclosure.md)
for the address) or open an issue once the repository is public.

## Sources

The research behind this note used the sources below, opened on 28 October
2026. Repository files are cited by path in the text.

- **Ledgers, card networks and benchmarks:** [Solana Compass, 10 Aug 2026](https://solanacompass.com/news/solana-hits-1719-million-daily-non-vote-transactions-its-first-billion-transaction-week) · [L2BEAT activity](https://www.l2beat.com/scaling/activity) · [Mysticeti on Sui](https://decentralizedthoughts.github.io/2026-03-06-mysticeti-revolutionizing-consensus-on-sui/) · [Radix public test](https://www.radixdlt.com/blog/hyperscale-update-500k-public-test-done) · [Visa 10-K FY2025](https://www.sec.gov/Archives/edgar/data/1403161/000140316125000089/v-20250930.htm) · [Mastercard 10-K 2025](https://www.sec.gov/Archives/edgar/data/1141391/000114139126000013/ma-20251231.htm) · [Celestia on Fibre and Derive](https://blog.celestia.org/derive-scaling-the-largest-onchain-options-exchange-with-celestia/) · [Celestia on benchmarks](https://blog.celestia.org/why-blockchain-benchmarks-are-usually-deceiving/) · [Ethproofs review](https://hackmd.io/@willcorcoran/ry16VMmfbx)
- **The agent economy:** [x402 counter analysis](https://www.danielmcglynn.com/the-x402-counter-has-shown-the-same-four-numbers-since-march/) · [x402 Foundation](https://www.linuxfoundation.org/press/linux-foundation-is-launching-the-x402-foundation-and-welcoming-the-contribution-of-the-x402-protocol) · [ERC-8004 counts](https://www.datawallet.com/crypto/erc-8004-explained) · [IDC forecast, via Microsoft](https://www.microsoft.com/en-us/microsoft-365/blog/2025/11/18/microsoft-agent-365-the-control-plane-for-ai-agents/)
- **Agent identity:** [ERC-8004](https://eips.ethereum.org/EIPS/eip-8004) · [empirical study of ERC-8004](https://arxiv.org/abs/2606.26028) · [Kite launch](https://www.theblock.co/post/399534/kite-launches-kite-chain-and-kite-agent-passport-enabling-autonomous-ai-agent-payments) and [whitepaper](https://www.gokite.ai/kite-whitepaper) · [A2A specification](https://a2a-protocol.org/latest/specification/) · [AP2 specification](https://ap2-protocol.org/specification/) · [x402 Sign-In-With-X](https://docs.x402.org/extensions/sign-in-with-x) · [Entra Agent ID concepts](https://learn.microsoft.com/en-us/entra/agent-id/identity-platform/key-concepts) · [IETF agent delegation chain](https://www.ietf.org/archive/id/draft-asor-wimse-agent-delegation-chain-01.html) · [IETF attenuating agent tokens](https://www.ietf.org/archive/id/draft-niyikiza-oauth-attenuating-agent-tokens-00.html) · [DID v1.1](https://www.w3.org/TR/2026/CR-did-1.1-20260305/) · [KERI](https://trustoverip.github.io/tswg-keri-specification/) · [UCAN](https://github.com/ucan-wg/spec) · [Biscuit](https://doc.biscuitsec.org/reference/specifications) · [FIPS 204](https://csrc.nist.gov/pubs/fips/204/final) · [FIPS 205](https://csrc.nist.gov/pubs/fips/205/final) · [RFC 8554](https://www.rfc-editor.org/rfc/rfc8554) · [SP 800-208](https://csrc.nist.gov/pubs/sp/800/208/final) · [Merkle Tree Certificates](https://datatracker.ietf.org/doc/draft-ietf-plants-merkle-tree-certs/)
- **Memory:** [Hotgraph](https://github.com/shayansal/Hotgraph) · [Mem0](https://arxiv.org/pdf/2504.19413) · [Zep](https://arxiv.org/html/2501.13956v1) · [Letta](https://www.letta.com/blog/our-next-phase) · [MemOS](https://arxiv.org/abs/2507.03724v4) · [MIRIX](https://arxiv.org/html/2507.07957v1) · [OpenAI memory, June 2026](https://startupfortune.com/openai-makes-chatgpt-memory-more-central-to-its-ai-platform/) · [Claude memory import](https://9to5mac.com/2026/03/02/free-claude-users-can-now-use-memory-and-import-context-from-rivals/) · [Gemini memory import](https://blog.google/innovation-and-ai/products/gemini-app/switch-to-gemini-app/) · [ReMEmbR](https://arxiv.org/html/2409.13682v1) · [HOV-SG](https://arxiv.org/html/2403.17846v2) · [Khronos](https://arxiv.org/html/2402.13817v2) · [DAAAM](https://arxiv.org/html/2512.00565v1) · [MemoryVLA](https://arxiv.org/abs/2508.19236) · [peaq verifiable storage](https://docs.peaq.xyz/sdk-reference/robotics-sdk/ros2/messaging/verifiable-storage) · [Walrus paper](https://arxiv.org/pdf/2505.05370v2) and [pricing](https://walrus.xyz/blog/announcing-predictable-pricing-in-usd-on-walrus/) · [Filecoin Onchain Cloud](https://filecoin.io/blog/filecoin-onchain-cloud-is-live-on-mainnet) · [RFC 9162](https://www.rfc-editor.org/rfc/rfc9162) · [MR-tree](https://cse.hkust.edu.hk/faculty/dimitris/PAPERS/ICDE08-MR.pdf) · [VTRQ](https://arxiv.org/pdf/2608.21314) · [Atlas](https://arxiv.org/pdf/2609.11841) · [V3DB](https://www.vldb.org/pvldb/vol19/p2604-qiu.pdf) · [RFC 9901 (SD-JWT)](https://rfc-editor.org/rfc/rfc9901.html)
- **Orchestration and verification:** [Tomašev et al., Distributional AGI Safety](https://arxiv.org/abs/2512.16856) · [Qwen3-Coder](https://qwenlm.github.io/blog/qwen3-coder/) · [DeepSeek-V3.2](https://arxiv.org/pdf/2512.02556) · [TechCrunch on environments](https://techcrunch.com/2025/09/21/silicon-valley-bets-big-on-environments-to-train-ai-agents/) · [METR on reward hacking](https://metr.org/blog/2025-06-05-recent-reward-hacking) · [MacDiarmid et al.](https://arxiv.org/abs/2511.18397) · [Cursor's SWE-bench Pro audit](https://officechai.com/ai/restricting-internet-access-and-sealing-github-history-causes-all-models-performance-to-drop-on-swe-bench-pro-says-cursor/) · [Anthropic's multi-agent research system](https://www.anthropic.com/engineering/multi-agent-research-system) · [Claude Managed Agents: orchestration](https://platform.claude.com/docs/en/managed-agents/multiagent-orchestration) and [budgets](https://platform.claude.com/docs/en/managed-agents/budgets) · [OpenAI and Hugging Face incident report](https://cdn.openai.com/pdf/67869394-cb91-4c12-888c-5cbd85c7814c/OpenAI-Hugging-Face%20Incident-Technical-Report.pdf) · [Anthropic on distillation attacks](https://www.anthropic.com/news/detecting-and-preventing-distillation-attacks) · AI Act [Art. 12](https://artificialintelligenceact.eu/article/12/), [Art. 19](https://artificialintelligenceact.eu/article/19/) and [Art. 53](https://artificialintelligenceact.eu/article/53/) · [Temporal](https://docs.temporal.io/activities) · [Restate](https://docs.restate.dev/foundations/key-concepts) · [Ray](https://docs.ray.io/en/latest/ray-core/fault_tolerance/tasks.html) · [Kueue](https://kueue.sigs.k8s.io/docs/concepts/preemption/) · [MCP tasks](https://modelcontextprotocol.io/seps/1686-tasks) · [ERC-8183](https://eips.ethereum.org/EIPS/eip-8183) · [Verde](https://arxiv.org/abs/2502.19405) · [TOPLOC](https://arxiv.org/abs/2501.16007) · [Thinking Machines on nondeterminism](https://thinkingmachines.ai/blog/defeating-nondeterminism-in-llm-inference/) · [TEE.fail](https://tee.fail/)
- **Devices, pools and costs:** [Apple background tasks](https://developer.apple.com/documentation/backgroundtasks/choosing-background-strategies-for-your-app.md) · [Apple review guidelines](https://developer.apple.com/app-store/review/guidelines/) · [Android periodic work](https://developer.android.com/develop/background-work/background-tasks/persistent/getting-started/define-work) · [Android foreground-service timeouts](https://developer.android.com/develop/background-work/services/fgs/timeout) · [Google Play policy on mining](https://support.google.com/googleplay/android-developer/answer/16302285) · [hole punching at scale](https://arxiv.org/abs/2510.27500) · [Helium HIP 149](https://heliumgeek.com/faq/hip149-iot-heliumgeek-changes.html) · [Grass airdrop](https://www.theblock.co/post/323805/grass-becomes-most-distributed-solana-airdrop-as-nearly-1-5-million-addresses-claim-tokens) · [Filecoin PDP](https://filecoin.io/blog/introducing-proof-of-data-possession-pdp-verifiable-hot-storage-on-filecoin) · [Cosmos distribution (F1)](https://docs.cosmos.network/main/build/modules/distribution) · [Polkadot nomination pools](https://wiki.polkadot.com/learn/learn-nomination-pools/) · [Ethereum distributed validators](https://ethereum.org/en/staking/dvt/) · [verifier's dilemma](https://eprint.iacr.org/2015/702) · [Hetzner dedicated](https://www.hetzner.com/dedicated-rootserver/matrix-ex/) · [Hetzner Cloud](https://www.hetzner.com/cloud/regular-performance) · [Contabo](https://contabo.com/en/pricing/) · [netcup](https://www.netcup.com/en/server/root-server)
