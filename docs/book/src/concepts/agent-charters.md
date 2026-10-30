# Agent Charters

An **Agent Charter** is the protocol-level contract between a principal and
an AI agent: limits the ledger enforces, a budget only the agent can spend,
a bond behind the agent's receipts, and a kill switch. A hijacked or
prompt-injected agent cannot exceed its caveats, because they are not in
its code. The worst case is bounded and known in advance.

A charter with an identity record is an agent's identity, and agents
launch subagents with identities of their own: see [Agent identities and
subagents](agent-identities.md).

## In the architecture

| Field | What it fixes | Example |
|---|---|---|
| Principal | Who answers for the agent | A company's treasury account |
| Capabilities | What the agent may do, as attenuable caveats | Pay listed suppliers only; at most 2,000 USD a day |
| Delegation | What sub-agents may inherit | Strictly narrower caveats |
| Model terms | Which models and providers may act | Provider attestation required above 500 USD |
| Receipts | Which actions must emit a receipt claim | Every payment |
| Bond | Stake behind the agent's receipts | 10,000 USD, slashed if a receipt is proven false |
| Kill switch | Who can revoke, and how fast | The principal; revocation is ordered by consensus and takes effect when it commits |

- **Limits live in the protocol.** Validators and the Fold check every
  agent operation against its Charter.
- **Payments stay consensus-free.** The agent spends from an allotment only
  it can write ([Escrow Lanes](escrow-lanes.md)). Until a revocation
  commits, the most it can spend is what remains in that allotment.
- **Delegation only narrows.** Each hop adds caveats and none can remove
  one, as in macaroons and Biscuit.
- **Receipts become reputation.** An agent's unchallenged receipts
  accumulate into reputation claims.

## Caveats in this implementation

| Caveat | Meaning |
|---|---|
| `payees` | Allowed payees; none means any payee |
| `max_per_payment` | The largest single payment |
| `burst`, `rate` | A token bucket: the most the agent may spend at once, and what refills it each Pulse. It starts full |
| `expires_at` | The last Pulse in which the agent may act |
| `evidence_above`, `evidence_policy` | Payments above this amount must cite an evidence claim that passes the decision rule under the principal's policy, for the payment's amount |
| `receipts` | Whether every payment issues a receipt claim |
| `may_delegate` | Whether the agent may create sub-charters |

## The transactions

- **`CreateCharter { agent, caveats, funding, agent_bond }`** (owned
  linear). The principal draws `funding + agent_bond` from its coins. The
  ledger creates the charter (a shared object in its own contention
  domain), an allotment of `funding` that only the agent can spend, and a
  bonded atom `bond:<principal>:charter:<id>` behind the agent's receipts,
  with `agent_bond` posted behind it.
- **`AgentPay { charter, payee, amount, purpose, evidence }`** (bounded).
  The agent pays from its allotment. The ledger checks the charter's
  standing (the overlays of this charter and of its ancestors, at most 9;
  a revoked or suspended one stops the payment, a narrowing adds its
  limits), then each caveat, in this order: enough remaining, not expired,
  payee allowed, per-payment limit, the limits of any narrowing above it,
  enough in the bucket, evidence. With receipts on, it
  issues a claim with schema `receipt.agent.payment/v1` whose subject is
  `agent paid <amount> USD for: <purpose>`, resting on the dual signature
  and the charter's agent atom.
- **`Delegate { charter, sub_agent, caveats, amount }`** (bounded). The
  agent moves part of its allotment to a sub-agent under a child charter.
  The child's caveats must attenuate the parent's: payees a subset, limits
  no higher, burst and rate no higher, expiry no later, receipts kept,
  delegation only if the parent allows it, and evidence required at no
  higher threshold. The child's burst and rate are carved out of the
  parent's bucket, and the tokens it starts with out of what the bucket
  holds (specification v1.14): the parent keeps only what it has not handed
  down, so a whole tree whose root has a burst σ and a rate ρ pays at most
  σ + ρ·τ over any τ + 1 consecutive Pulses.
- **`RevokeCharter { charter }`** (contended). The principal revokes (since
  v1.8 also the backer, any ancestor's agent, or the agent itself). The
  revocation is written to the charter's overlay (v1.14). Every payment
  under the charter, or under any charter delegated from it, that executes
  after the revocation fails. Payments in the same Pulse run first, because
  owned-path transactions execute before contended ones (see
  [Pulses](pulses.md)).
- **`ReclaimCharter { charter }`** (contended). Once a charter is dead
  (revoked, expired, or an ancestor revoked), its principal or root
  principal returns what is left in its allotments to the root principal,
  whose funds they were.

The CLI has `sion charter create`, `pay` and `revoke`. `Delegate` and
`ReclaimCharter` are available through the Rust API, signed transactions
over HTTP, or the MCP tool `sion_submit_intent`; the `agents` demo
delegates.

## On the devnet

A charter that lets `agent7` pay only `gpu-cloud`, at most 50 USD per
payment and 80 USD at once, its bucket refilled by 0.1 USD a Pulse:

```text
$ sion charter create acme agent7 --payees gpu-cloud --max-payment 50 --burst 80 --rate 0.1 --funding 200 --bond 100 --lifetime 5000
acme signs: Give agent7 an Agent Charter funded with 200 USD and a 100 USD bond behind its receipts: only to gpu-cloud, at most 50 USD per payment, 80 USD at once, refilled by 0.1 USD a Pulse, until Pulse 5005, a receipt claim for every payment, may delegate narrower limits. Fee 0.0012 USD; valid until Pulse 25.
Pulse 7: Done: charter 080027af created; fee 0.0012 USD.
  digest 4753898eb688e58574a372b6f5d7bfec695214f11411221d4fed06823df6133b
charter 080027af99f5541e72560fbc961349c486c4623e4622a5fc23bff96668a14cea
allotment 3a51139ff80e74baf2c49f40627288db4833a493bce4a6fd75ed714b27a31f6e
```

What the ledger did with five payments and one revocation (the full
commands are in [Charters for AI agents](../guides/charters.md)):

```text
Pulse 10: Done: agent paid 30 USD to gpu-cloud (receipt claim e06955c0); fee 0.0012 USD.
Pulse 13: Failed (charter caveat violated: payee not allowed); nothing happened except the fee of 0.0011 USD.
Pulse 16: Failed (charter caveat violated: payment 60 above per-payment limit 50); nothing happened except the fee of 0.0011 USD.
Pulse 19: Done: agent paid 45 USD to gpu-cloud (receipt claim 38c2ea63); fee 0.0011 USD.
Pulse 22: Failed (charter caveat violated: the bucket holds 6.2, below the payment 10); nothing happened except the fee of 0.0011 USD.
Pulse 26: Done: charter 080027af revoked; fee 0.0011 USD.
Pulse 28: Failed (charter caveat violated: charter revoked); nothing happened except the fee of 0.0011 USD.
```

A refused payment still costs its fee, paid from the agent's own coins:
the agent needs a little USD of its own for fees.

The receipt is a claim backed by the 100 USD agent bond:

```text
$ sion claim decide e06955c0…777e 40
{
  …
  "accept": true,
  "adjudicator_cap": "Infinite",
  "reasons": [
    "cost of corruption 100 covers κ·V = 80"
  ]
}
```

## In this implementation

- **Built**: charters, caveats, allotments, attenuated delegation,
  revocation through the whole delegation chain, reclaiming dead
  allotments, evidence requirements, receipts as claims.
- **Not built**: model terms (which models and providers may act), and
  challenges against receipts. Receipt claims name no adjudicator, so
  nobody can challenge them, and the agent bond behind them cannot be
  slashed through them.
- Charters are payment charters only: an agent's allotment is USD, and its
  only actions are paying and delegating.
