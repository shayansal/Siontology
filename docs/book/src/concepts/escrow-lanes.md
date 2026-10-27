# Escrow Lanes

Some shared state has a bound rather than a single owner: seats in a venue,
a budget, a rate limit, a supply cap. Ordering every decrement through
consensus would be slow; ignoring the bound would overdraw it. Escrow Lanes
split the bound so that each part has one writer.

## How it works

- **Split the total.** A shared total *Q* is split into **allotments** that
  sum to at most *Q*, each with a single writer, such as a sales channel, a
  merchant terminal or an agent.
- **Spend locally.** Each writer draws down its own allotment with ordinary
  lane certificates and never coordinates with the others.
- **Rebalance rarely.** When an allotment runs low, the total's manager
  issues a new one from the unallocated amount. Only that rebalance goes
  through consensus.

The invariant, never exceeding *Q*, holds throughout. This is the escrow
method and the demarcation protocol of distributed databases, applied to a
ledger.

## The objects

A **bounded total** is a shared object with a label, an asset, a manager,
the total, the unallocated amount, its allotments with their writers and
initial amounts, and what writers have returned. Its invariant: the sum of
every allotment ever issued plus what is unallocated never exceeds the
total plus what was returned.

An **allotment** is an object owned by its writer. It records its source (a
bounded total or a charter), its writer, its asset, the remaining amount,
and, for charter allotments, the charter's caveats and the current spending
window.

## The operations

| Transaction | Class | What it does |
|---|---|---|
| `CreateBounded { label, asset, total, allotments }` | Owned linear | Draws the total from the sender's coins, creates the bounded total and up to 256 allotments |
| `SpendAllotment { to, amount }` | Bounded | The writer pays from its allotment |
| `ReturnAllotment` | Bounded | The writer consumes its allotment; the remainder returns to the total at the end of the Pulse |
| `Rebalance { bounded, holder, amount }` | Contended | The manager issues a new allotment from the unallocated amount |

Agent Charters use the same mechanism: an agent's budget is an allotment
only the agent can write, so its payments need no consensus. See
[Agent Charters](agent-charters.md).

## In this implementation

- **Built**: bounded totals, allotments, spending, returning, rebalancing,
  the invariant (checked by the simulation chain after every Pulse).
- **No CLI command yet.** Bounded totals are reachable through the Rust API
  (`sion_exec::TxKind`), signed transactions submitted to
  `POST /v1/transactions`, or the MCP tool `sion_submit_intent`. Charter
  allotments have CLI commands; see [Charters for AI agents](../guides/charters.md).
- **Different from the architecture**: the paper runs reservations against
  a bond's capacity on Escrow Lanes. Here the assurance registry is one
  contended object, and every reservation (`Accept`) goes through
  consensus. The implementation status names this as the first thing to
  split for production.
