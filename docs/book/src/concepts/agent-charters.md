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
| `max_per_window`, `window` | The largest total within a window of `window` Pulses |
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
  The agent pays from its allotment. The ledger checks the charter chain
  (this charter and up to 15 ancestors; any revoked one stops the payment),
  then each caveat, in this order: enough remaining, not expired, payee
  allowed, per-payment limit, window total, evidence. With receipts on, it
  issues a claim with schema `receipt.agent.payment/v1` whose subject is
  `agent paid <amount> USD for: <purpose>`, resting on the dual signature
  and the charter's agent atom.
- **`Delegate { charter, sub_agent, caveats, amount }`** (bounded). The
  agent moves part of its allotment to a sub-agent under a child charter.
  The child's caveats must attenuate the parent's: payees a subset, limits
  no higher, the window no shorter, expiry no later, receipts kept,
  delegation only if the parent allows it, and evidence required at no
  higher threshold. The child's window limit is carved out of the parent's
  (specification v1.8): the parent's own limit becomes what it has not
  handed down.
- **`RevokeCharter { charter }`** (contended). The principal revokes (since
  v1.8 also the backer, any ancestor's agent, or the agent itself). Every
  payment under the charter, or under any charter delegated from it, that
  executes after the revocation fails. Payments in the same Pulse run
  first, because owned-path transactions execute before contended ones
  (see [Pulses](pulses.md)).
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
payment and 80 USD per 100 Pulses:

```text
$ sion charter create acme agent7 --payees gpu-cloud --max-payment 50 --max-window 80 --window 100 --funding 200 --bond 100 --lifetime 5000
acme signs: Give agent7 an Agent Charter funded with 200 USD and a 100 USD bond behind its receipts: only to gpu-cloud, at most 50 USD per payment, 80 USD per 100 Pulses, until Pulse 5078, a receipt claim for every payment, may delegate narrower limits. Fee 0.0012 USD; valid until Pulse 98.
Pulse 81: Done: charter b5a996a1 created; fee 0.0012 USD.
  digest dd6f1072604e2a537b12d032ce3382c328bd0f49143af36d62bda333ea82e10f
charter b5a996a110d819b4aff62bade475dc17a1a29edc7cbca838535ccac9e5884c22
allotment a78700d352f435d63964e670fd7e95f47367e5769c8616983de6c165784e1b44
```

What the ledger did with five payments and one revocation (the full
commands are in [Charters for AI agents](../guides/charters.md)):

```text
Pulse 85: Done: agent paid 30 USD to gpu-cloud (receipt claim 9d0032a4); fee 0.0012 USD.
Pulse 88: Failed (charter caveat violated: payee not allowed); nothing happened except the fee of 0.0011 USD.
Pulse 90: Failed (charter caveat violated: payment 60 above per-payment limit 50); nothing happened except the fee of 0.0011 USD.
Pulse 92: Done: agent paid 45 USD to gpu-cloud (receipt claim c70a4257); fee 0.0011 USD.
Pulse 94: Failed (charter caveat violated: window total 85 above limit 80); nothing happened except the fee of 0.0011 USD.
Pulse 99: Done: charter b5a996a1 revoked; fee 0.0011 USD.
Pulse 101: Failed (charter caveat violated: charter revoked); nothing happened except the fee of 0.0011 USD.
```

A refused payment still costs its fee, paid from the agent's own coins:
the agent needs a little USD of its own for fees.

The receipt is a claim backed by the 100 USD agent bond:

```text
$ sion claim decide 9d0032a485de130c5900c88a4c647f02be6f3a7c49cadf251856d36859ac807c 40
{
  "accept": true,
  ...
  "reasons": [
    "cost of corruption 100 covers κ·V = 80"
  ],
  ...
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
