# Agent identities and subagents

Every AI agent on Siontology can have an **identity**: a unique id, its
own keys, a card that says what it is, a place in a tree of agents, and
limits the ledger enforces. An agent can launch **subagents**, and each of
them gets an identity of its own, with narrower limits and a share of its
parent's budget. One transaction from any agent above stops a whole
subtree.

An agent identity is an [Agent Charter](agent-charters.md) with an
identity record. The charter's id is the agent's id. Everything a charter
guarantees holds for every identity in the tree, at every depth.

## What an identity holds

| Part | What it is | Where it lives |
|---|---|---|
| Id | The charter's object id, unique by construction (derived from the transaction that created it) | The ledger |
| DID | `did:sion:<chain>:<id>`: the [chain identity](../reference/chain-identity.md) and the agent's id, 64 hex digits each | The card |
| Account | The account that signs for the agent, with its own dual post-quantum keys | The ledger |
| Principal | Who launched it: the person or organization for a registered agent, the parent agent for a subagent | The ledger |
| Backer | The person or organization at the root, who answers for the whole tree | The ledger |
| Card | A commitment (SHA3-256, domain `sion/agent/card`) to the agent's card file: name, description, endpoint, skills, provider | The commitment on the ledger, the file wherever the agent publishes it |
| Depth | 0 for a registered agent, then one more per generation, at most 15 | The ledger |
| Caveats | Payees, limits per payment and per window, expiry, evidence rules, receipts, whether it may launch subagents | The ledger |
| Budget | An allotment only this agent can spend | The ledger |
| Status | Active, suspended (and by whom), or revoked | The ledger |

The card follows the shape of an [A2A](https://a2a-protocol.org) agent
card, with a `sion` section naming the agent's id, account and principal.
`sion agent card-template` prints one. The ledger stores only the card's
commitment, so a card can be served from anywhere and checked against it.

## Launching subagents

`SpawnAgents` launches up to 256 subagents in one transaction. For each, the
parent names the subagent's account, card, caveats and budget. The ledger
checks, for every subagent, that its caveats are at least as strict as the
parent's and that the parent's allotment covers the budgets. It then
creates each subagent's charter, with an identity one level deeper, and an
allotment holding its budget.

- **Siblings never contend.** Each subagent spends only its own allotment,
  so a swarm's payments are owned transactions, certified without
  consensus.
- **Limits only narrow.** A subagent cannot get a payee, a limit, a
  lifetime or a right to launch subagents that its parent does not have.
- **Budgets are carved, not copied.** What a parent gives its subagents
  leaves its own allotment.
- **Rates are carved too.** A subagent's limit per window comes out of its
  parent's: the parent's own limit becomes what it has not handed down. The
  window limits of a whole subtree therefore add up to at most its root's,
  however many subagents it launches. (Windows are fixed, not sliding, so
  in an interval that straddles two windows a subtree can spend up to
  twice that.) Until specification v1.8, each child could spend its
  parent's whole window limit, so 100 children could spend 100 times the
  parent's rate.
- **Depth is bounded.** A tree holds at most 16 generations, so every
  check walks at most 16 charters.

## Containment: suspension and revocation

The **overseers** of an agent are its backer, its principal and the agent
of every charter above it.

| Action | Who may do it | Effect |
|---|---|---|
| `SuspendAgent` | Any overseer | The agent and everything below it stop: every payment or spawn in the subtree fails until the suspension is lifted. The first suspender is recorded |
| `ResumeAgent` | The suspender, or the backer | The subtree runs again |
| `RevokeCharter` | The agent itself, or any overseer | Permanent. The subtree stops for good, and the backer can reclaim the unspent budgets |
| `SetAgentCard` | The agent itself | Replaces the card commitment |

Every check of an agent walks its chain up to the root, so a suspension or
revocation anywhere above takes effect for everything below at once. A
suspension or revocation is ordered by consensus and takes effect when it
executes: a payment executed in the same Pulse still succeeds, because
owned transactions run before contended ones. A suspended agent's funds
stay in place; only a revoked or expired one can be reclaimed.

**What a certificate means for an agent.** A lane certificate locks a
payment's inputs, so no conflicting spend can be certified. For an agent's
payment it does not promise success: if a suspension or revocation of the
agent, or of any agent above it, executes before the payment does, the
payment fails when it executes. An agent's payment is final when it
executes, not when it is certified. A rule that would make certified agent
payments final as well (stamping approvals with the Pulse their validators
had executed, and applying a revocation only to approvals made after it)
is a design under study, not built.

## The transactions

| Kind | Class | Spec |
|---|---|---|
| `RegisterAgent { agent, card, caveats, funding, agent_bond }` | Owned | §5.35 |
| `SpawnAgents { charter, children }` | Bounded (owned) | §5.36 |
| `SetAgentCard { charter, card }` | Contended, in the charter's domain | §5.37 |
| `SuspendAgent { charter }` | Contended, in the charter's domain | §5.38 |
| `ResumeAgent { charter }` | Contended, in the charter's domain | §5.39 |
| `RevokeCharter { charter }` | Contended, in the charter's domain | §5.16 |

All of them are in [specification](../specification.md) version 1.8, and
both proof stacks implement them. The differential tests run every
operation, including each way it can fail, through both.

## Scale

An identity costs the ledger two objects: the charter and the allotment.
A spawn of 256 subagents is one transaction with one pair of signatures.
Checking an agent reads at most 16 charters. Revoking a subtree is one
write, whatever its size. These are properties of the rules, not
measurements.

## Try it

- `sion demo swarm --local`: an orchestrator registered by `acme` launches
  12 researchers in one transaction; one of them launches 3 sub-researchers;
  all 15 pay for a tool call at once; a payment above a subagent's limit is
  refused; `acme` suspends the orchestrator and a sub-researcher two levels
  down cannot pay; `acme` resumes it; the orchestrator revokes a researcher
  and `acme` reclaims its budget.
- [Agent identities from the command line](../guides/agents.md).
