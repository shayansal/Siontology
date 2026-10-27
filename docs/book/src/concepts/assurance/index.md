# The Assurance Algebra

A ledger cannot see the world. Blockchains call this the oracle problem and
treat it as a flaw. The Assurance Algebra treats it as a priced, bounded
risk.

Every claim carries a formula of the assumptions it rests on. From that
formula, any consumer computes, under its own policy, the minimum an
attacker must spend or forfeit to make the claim false: the claim's **cost
of corruption**. An agent relies on a claim only when that cost exceeds what
it has at stake, and it **reserves** the claim's backing, so that no claim
is relied on beyond what slashing can repay. If the claim is proven false,
the bonds behind it are **slashed** and the parties who relied on it are
**compensated** first.

The pieces, each with its own chapter:

| Piece | One line |
|---|---|
| [Atoms and formulas](atoms-and-formulas.md) | Assumptions are atoms; claims combine them with `∧` (joint use) and `∨` (alternatives) |
| [Cost of corruption](cost-of-corruption.md) | `min` over `∧`, sum over atom-disjoint `∨`, a sound interval when atoms are shared |
| [The decision rule](decision-rule.md) | Accept when `min(δ · CoC_low, CoC(adjudicator)) ≥ κ · V` |
| [Reservations](reservations.md) | Every acceptance reserves `κ · V` of the backing, pushed down the formula |
| [Bonds, challenges and slashing](bonds-challenges-slashing.md) | Bonds are locked; anyone can challenge; victims are paid first |
| [Insurance](insurance.md) | Failure probabilities from the same formula |

## A worked example from the architecture paper

An agent must decide whether to pay invoice #1042 for 25,000 USD. The claim
*invoice payable* joins three inputs:

- delivery confirmed by three independent witnesses, each bonded at
  20,000 USD;
- the invoice arithmetic, proven by two proof stacks;
- the supplier's identity credential, from a bank bonding 1,000,000 USD.

The cost of corruption is `min(60,000; ∞; 1,000,000) = 60,000 USD`:
delivery is the weak link. With κ = 2 the agent pays invoices up to
30,000 USD automatically; above that it asks for a fourth witness. If three
financiers each wanted to lend 25,000 USD against the same invoice, the
first would reserve 50,000 USD of the 60,000 USD backing, and the second
would be refused until more evidence was posted.

## The same idea, run on the devnet

`sion demo invoice` runs a smaller version end to end. The invoice rests on
the supplier's dual signature and on either the supplier's bond or a
delivery claim backed by the shipper's bond:

```text
Pulse  15  supplier issues "Invoice #1042: 10,000 USD payable"  [dual-signature ∧ (bond:supplier ∨ delivery)]
           cost of corruption (standard policy): 50,000 USD, exact
           = 30,000 (supplier's bond) + 20,000 (shipper's bond): a forger must corrupt both routes
           rely on  10000 USD at κ = 2 → ACCEPT: cost of corruption 50,000 covers κ·V = 20,000
           rely on  30000 USD at κ = 2 → REJECT: cost of corruption 50,000 (after detection discount 1000/1000) is below κ·V = 60,000
Pulse  18  financier funds the invoice and reserves 20,000 USD of its backing (κ·V): nobody else can rely on the same bonds beyond what slashing can repay
Pulse  20  buyer challenges the supplier's attestation (deposit 1,000 USD)
Pulse  22  judge upholds: supplier's bond slashed (30,000 USD); the invoice still holds through the delivery route
Pulse  24  buyer challenges the shipper's delivery attestation, under the shipper's own adjudicator
Pulse  26  judge upholds: 2 claims falsified (the delivery, and the invoice resting on it)
           financier compensated 10,000 USD automatically from the slashed bonds (balance 199,999.9988 → 209,999.9988)
```

## Why this matters for AI agents

An agent cannot judge a document's authenticity the way a person does, but
it can evaluate a formula and compare two numbers. The decision rule gives
every agent the same machine-checkable trust test. Forged or prompt-injected
inputs fail it unless someone puts real money behind them. The MCP tool
`sion_assess_claim` exposes exactly this test (see
[Connect an AI agent over MCP](../../getting-started/mcp.md)).

## In this implementation

- **Built**: atoms, formulas over atoms and claims, policy-relative cost of
  corruption intervals with an exact solver for shared atoms, the decision
  rule, reservations, bonds with lock periods, challenges, adjudication,
  slashing with compensation, falsification that propagates to dependent
  claims, and failure probabilities. Property tests cover the algebra.
- **Simplified**: the assurance registry is one contended object; every
  claim, bond and reservation operation touches it.
- **Not built**: automatic slashing for cryptographically checkable
  contradictions (every challenge goes to an adjudicator), and an insurance
  market.
