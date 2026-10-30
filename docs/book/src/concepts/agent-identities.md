# Agent identities and subagents

Every AI agent on Siontology can have an **identity**: a unique id, its
own keys, a card that says what it is, a place in a tree of agents, and
limits the ledger enforces. An agent can launch **subagents**, and each of
them gets an identity of its own, with narrower limits and a share of its
parent's budget and rate. One transaction from any agent above stops, or
narrows, a whole subtree.

An agent identity is an [Agent Charter](agent-charters.md) with an
identity record. The charter's id is the agent's id. Everything a charter
guarantees holds for every identity in the tree, at every depth.

## What an identity holds

| Part | What it is | Where it lives |
|---|---|---|
| Id | The charter's object id: for a registered agent, derived from the transaction that created it; for a subagent, `H("sion/id/agent", parent ‖ index)`, its **lineage id** | The ledger |
| Lineage | Its tree's root and the path of indices down to it, from which anyone recomputes every ancestor's id | The ledger |
| DID | `did:sion:<chain>:<id>`: the [chain identity](../reference/chain-identity.md) and the agent's id, 64 hex digits each | The card |
| Account | The account that acts for the agent: one with its own dual post-quantum keys, or, for a keyless subagent, one derived from its id that its nearest keyed ancestor signs for | The ledger |
| Principal | Who launched it: the person or organization for a registered agent, the parent agent for a subagent | The ledger |
| Backer | The person or organization at the root, who answers for the whole tree | The ledger |
| Card | A commitment (SHA3-256, domain `sion/agent/card`) to the agent's card file: name, description, endpoint, skills, provider | The commitment on the ledger, the file wherever the agent publishes it |
| Depth | 0 for a registered agent, then one more per generation, at most 8 | The ledger |
| Caveats | Payees, a limit per payment, a token bucket (a burst it may spend at once and a rate that refills it each Pulse), expiry, evidence rules, receipts, whether it may launch subagents | The ledger |
| Budget | An allotment only this agent can spend, with its bucket | The ledger |
| Overlay | What happened to it afterwards: revoked, suspended (and by whom), narrowed | The ledger, at `H("sion/overlay", id)`, only once something happened |

The card follows the shape of an [A2A](https://a2a-protocol.org) agent
card, with a `sion` section naming the agent's id, account and principal.
`sion agent card-template` prints one. The ledger stores only the card's
commitment, so a card can be served from anywhere and checked against it.

## Launching subagents

`SpawnAgents` launches up to 256 subagents in one transaction. For each, the
parent names the subagent's account, card, caveats and budget. The ledger
checks, for every subagent, that its caveats are at least as strict as the
parent's, that the parent's allotment covers the budgets and that its
bucket covers the bursts and rates. It then creates each subagent's
charter at its lineage id, one level deeper, and an allotment holding its
budget and a full bucket.

- **Siblings never contend.** Each subagent spends only its own allotment,
  so a swarm's payments are owned transactions, certified without
  consensus.
- **Limits only narrow.** A subagent cannot get a payee, a limit, a
  lifetime or a right to launch subagents that its parent does not have,
  and evidence its parent requires is judged by its parent's policy. An
  expired agent launches nothing, and nothing is launched already expired.
- **Budgets are carved, not copied.** What a parent gives its subagents
  leaves its own allotment.
- **Rates add up.** A subagent's burst and rate are carved out of its
  parent's bucket, and the tokens it starts with out of the parent's
  tokens: the parent keeps only what it did not hand down. A subtree whose
  root has a burst `σ` and a rate `ρ` therefore pays at most `σ + ρ·τ` in
  any `τ + 1` consecutive Pulses, however many subagents it launches and
  however deep (specification v1.14; a property test checks it on random
  trees). Until v1.14, limits were fixed windows, so across a window's
  boundary a subtree could pay twice its limit; until v1.8, each child
  could spend its parent's whole limit.
- **Ids are computed.** A subagent's id comes from its parent's id and an
  index that only the parent writes (a counter in its allotment), so it is
  unique by construction, and anyone holding a tree's root and an agent's
  path recomputes every ancestor's id without reading them.
- **Depth is bounded.** A tree holds at most 9 generations (depth 0 to 8),
  so checking an agent reads at most 9 overlays.

## Cohorts: a million subagents in one transaction

`SpawnCohort` commits to up to 2²⁰ (1,048,576) subagents at once
(specification v1.14). The parent builds a **Merkle sum tree** over the
members, each a budget, a burst, a rate, a card and an optional account,
and publishes only its root, with the sums of the members' bursts and
rates. The ledger carves those sums from the parent's bucket, reserves the
cohort's lineage indices and stores the cohort. A member costs nothing on
the ledger until it is needed.

`MaterializeAgents` then creates up to 128 members at a time: for each,
the parent shows its leaf and the path to the root, and the ledger creates
its charter at its lineage id and pays its budget from the parent's
allotment. Every node of the tree carries the sums of its leaves, so a
path that verifies carries the sums the root commits to: whatever the
parent put in the rest of the tree, the members that can ever be
materialized never carry more burst or rate than was carved. A member's
bucket starts as if it had existed since the cohort: full if the parent
gave up the tokens then, otherwise filling at its rate from the cohort's
creation. A member materializes once; a second attempt finds its id taken.

## Keyless subagents

A subagent need not have keys of its own (specification v1.13). A parent
that names no account for a child spawns it **keyless**: its account is
`H("sion/agent/keyless", id)`, derived from its id, and no policy hashes to
such an account, so no key controls it. The child acts only through
[batches](batched-authorization.md) that its **nearest keyed ancestor**
signs: the first agent above it that has keys of its own.

- **Nothing to generate or keep.** A keyless subagent costs no key pair,
  no seed on disk and no policy on the ledger: launching a thousand of them
  makes a thousand charters and allotments, nothing more.
- **One signature for many.** The ancestor signs one batch of up to 256
  transactions, from any of its keyless descendants and its own, with one
  authorization. Each member still spends only its own allotment, under
  its own caveats, so the ledger contains each subagent exactly as it
  contains one with keys.
- **Who can act for it is fixed.** The ancestor is found by walking the
  child's chain upward past keyless agents, so only one account can ever
  sign for a keyless subagent, and it already oversees it: it can suspend
  or revoke it, and its own budget contains the child's. A keyed subagent
  below the ancestor ends the walk, so its keyless children are its own to
  sign for.

A keyless subagent pays its fees from coins its account holds: its parent
gives it some, as `sion agent spawn --keyless --gas` and the runtime's
`agent_spawn` do. Its containment is that of the runtime holding its
ancestor's keys: a subagent that could make that runtime sign as the
ancestor would act beyond its own caveats, within the ancestor's. The
signing has to stay outside the models' reach, which is a trust atom
(`agent:keyless-signer`).

## Containment: suspension, narrowing and revocation

The **overseers** of an agent are its backer, its principal and the agent
of every charter above it. An overseer acts only in **good standing**: one
whose own charter, or any charter above it, is revoked or suspended has no
power over the agents below it until that is lifted, so a compromised
agent that its backer has suspended cannot revoke or narrow its
subagents meanwhile. What overseers do is written to the agent's
**overlay**, one object per agent that exists only once something
happened to it.

| Action | Who may do it | Effect |
|---|---|---|
| `SuspendAgent` | Any overseer in good standing | The agent and everything below it stop: every payment or spawn in the subtree fails until the suspension is lifted. The highest suspender is recorded: a suspension from higher in the tree replaces one from lower down, never the reverse |
| `ResumeAgent` | The recorded suspender, in good standing, or the backer | The subtree runs again |
| `NarrowAgent` | The agent itself or any overseer in good standing; only the backer may widen or lift | Every payment in the subtree must also go to an allowed payee, stay under a smaller largest payment and come before an earlier end; the subtree may be barred from spawning |
| `RevokeCharter` | The agent itself, or any overseer in good standing | Permanent. The subtree stops for good, and the backer can reclaim the unspent budgets |
| `SetAgentCard` | The agent itself | Replaces the card commitment |

Every check of an agent computes its ancestors' ids from its lineage and
reads their overlays, from the agent up to the root: at most 9 reads, most
of them proofs that no overlay exists. So a suspension, narrowing or
revocation anywhere above takes effect for everything below at once, with
one write. It is ordered by consensus and takes effect when it executes:
a payment executed in the same Pulse still succeeds, because owned
transactions run before contended ones. A suspended agent's funds stay in
place; only a revoked or expired one can be reclaimed.

**What a certificate means for an agent.** A lane certificate locks a
payment's inputs, so no conflicting spend can be certified. For an agent's
payment it does not promise success: if a suspension, narrowing or
revocation of the agent, or of any agent above it, executes before the
payment does, the payment is checked against it when it executes. An
agent's payment is final when it executes, not when it is certified. A rule
that would make certified agent payments final as well (stamping approvals
with the Pulse their validators had executed, and applying a revocation
only to approvals made after it) is a design under study, not built.

## The transactions

| Kind | Class | Spec |
|---|---|---|
| `RegisterAgent { agent, card, caveats, funding, agent_bond }` | Owned | §5.35 |
| `SpawnAgents { charter, children }` | Bounded (owned) | §5.36 |
| `SpawnCohort { charter, root, count, burst, rate, full, caveats }` | Bounded (owned) | §5.55 |
| `MaterializeAgents { cohort, members }` | Bounded (owned) | §5.56 |
| `SetAgentCard { charter, card }` | Contended, in the charter's domain | §5.37 |
| `SuspendAgent { charter }` | Contended, in the charter's domain | §5.38 |
| `ResumeAgent { charter }` | Contended, in the charter's domain | §5.39 |
| `NarrowAgent { charter, narrowing }` | Contended, in the charter's domain | §5.57 |
| `RevokeCharter { charter }` | Contended, in the charter's domain | §5.16 |

Identities are in [specification](../specification.md) version 1.8,
keyless subagents in version 1.13 (§4.3, §5.36), and lineage ids, buckets,
overlays and cohorts in version 1.14 (§5.13, §5.55 to §5.57). Both proof
stacks implement them. The differential tests run every operation,
including each way it can fail, through both.

## Scale

Measured on a 2-core virtual machine (Intel Xeon at 2.80 GHz), idle, with
a release build of specification v1.14 on 30 October 2026 (`sion bench
--only lineage`, three runs):

| What | Measured |
|---|---|
| Building the sum tree of a cohort of 1,048,576, by the client | 1.78 to 1.85 s on one core |
| The `SpawnCohort` that commits to it | 19,906 bytes, almost all of them its signatures; a fee of 0.0011 USD |
| `MaterializeAgents` of 128 members | 1,523 bytes and 0.000142 USD a member, each with a proof of 20 siblings; 119 to 127 µs a member to execute on one core, the transaction's signature check included |
| A materialized keyless member | 868 bytes of state: its charter and its allotment |
| A payment by an agent at depth 8 | 14 objects read, 10 of them proven absent (the overlays of the agent and its 8 ancestors, and the coin the payment creates); a witness of 7,317 bytes |

By our arithmetic, ten billion identities take 9,537 `SpawnCohort`
transactions, about 10.5 USD in fees at devnet prices. A member costs
nothing more until it is materialized, and then 868 bytes of state and
0.000142 USD. The [benchmarks guide](../guides/benchmarks.md#lineage-and-cohorts)
has the whole run.

## Try it

- `sion demo swarm --local`: an orchestrator registered by `acme` launches
  12 researchers in one transaction; one of them launches 3 sub-researchers;
  all 15 pay for a tool call at once; a payment above a subagent's limit is
  refused; `acme` suspends the orchestrator and a sub-researcher two levels
  down cannot pay; `acme` resumes it; the orchestrator revokes a researcher
  and `acme` reclaims its budget.
- `sion demo batch --local`: an orchestrator launches 64 keyless
  subagents and signs once for a payment from each.
- [Agent identities from the command line](../guides/agents.md), with
  cohorts and narrowing.
