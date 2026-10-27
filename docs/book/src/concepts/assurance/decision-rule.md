# The decision rule

An agent with value *V* at stake accepts claim *c* when

```text
min( δ_c · CoC_low(c),  CoC(adjudicator_c) )  ≥  κ · V
```

- **κ ≥ 1** is the consumer's safety factor (2 in the standard policy).
- **δ ≤ 1** is the claim's detection factor (`detection_permille / 1000`):
  how reliably a falsehood of this kind is proven.
- **CoC(adjudicator)** caps trust at the cost of corrupting the dispute
  process. A claim with no adjudicator has no cap (∞). It also cannot be
  challenged at all in this implementation, because automatic slashing on
  cryptographic evidence is not built.

The network proves the formula and the bonds behind it; each consumer
evaluates its own policy locally, in time linear in the formula's size.

## Acceptance also needs backing

When the consumer intends to rely on the claim, acceptance also requires
that at least κ · V of the claim's backing is still **unreserved**, and
accepting reserves it (see [Reservations](reservations.md)). The node's
decision query (`sion claim decide`, `POST /v1/claims/{id}/decide`, the MCP
tool `sion_assess_claim`) checks both conditions; the `Accept` transaction
checks both and then reserves.

## The answer explains itself

A decision returns every intermediate value. This real answer from the
devnet is for a claim resting on a 50 USD bond, with an adjudicator bonded
at 200 USD, after another consumer had reserved 20 USD of its backing:

```text
$ sion claim decide b677314a931e86fee2403fe437822ec7d087f0df37dbd9452735b16177c41634 10
{
  "accept": true,
  "adjudicator_cap": {
    "Finite": 200000000
  },
  "coc": {
    "exact": true,
    "high": {
      "Finite": 50000000
    },
    "low": {
      "Finite": 50000000
    }
  },
  "detection_adjusted": {
    "Finite": 50000000
  },
  "effective": {
    "Finite": 50000000
  },
  "reasons": [
    "cost of corruption 50 covers κ·V = 20"
  ],
  "required": 20000000,
  "reservable": {
    "Finite": 30000000
  }
}
```

Amounts in JSON are micro-units (1 USD = 1,000,000); the `reasons` use
whole units.

| Field | Meaning |
|---|---|
| `accept` | The verdict |
| `coc` | The cost-of-corruption interval under the policy |
| `detection_adjusted` | `δ · CoC_low` |
| `adjudicator_cap` | The adjudicator's cost of corruption (`"Infinite"` if none) |
| `effective` | The smaller of the two |
| `required` | κ · V |
| `reservable` | The largest reservation the claim's backing can still take |
| `reasons` | Human-readable reasons, most important first |

Asking for 30 USD on the same claim fails twice over:

```text
  "reasons": [
    "cost of corruption 50 (after detection discount 1000/1000) is below κ·V = 60",
    "only 50 of backing is unreserved; κ·V = 60 is needed"
  ],
```

A falsified claim is refused first of all because it is not active:

```text
  "reasons": [
    "claim is not active (Falsified { at: Pulse(56), causes: [AtomId(05ab20b3)] })",
    "cost of corruption 0 (after detection discount 1000/1000) is below κ·V = 20",
    "only 0 of backing is unreserved; κ·V = 20 is needed"
  ],
```

## Choosing a policy

- `sion claim decide` uses the standard policy, with `--kappa` and
  `--distrust <atom,…>`. Distrusting matters only for atoms the policy
  would otherwise trust. Distrusting a bond atom changes nothing, because a
  bond is never trusted, only counted.
- `POST /v1/claims/{id}/decide` takes a full policy as JSON. With an empty
  policy (no trusted classes), a claim resting only on a dual signature is
  worth 0; with the standard policy it is worth ∞:

```text
$ curl -s -X POST http://127.0.0.1:8730/v1/claims/d47fe15d7cb445ece35b1d68c71f09afa05caace860068863642fa4aebecd23e/decide \
    -d '{"value": 10000000, "policy": {"trusted_atoms": [], "trusted_classes": [], "distrusted_atoms": [], "assumed_broken": [], "kappa": {"num": 2, "den": 1}}}'
{"accept":false,"adjudicator_cap":"Infinite","coc":{"exact":true,"high":{"Finite":0},"low":{"Finite":0}},"detection_adjusted":{"Finite":0},"effective":{"Finite":0},"reasons":["cost of corruption 0 (after detection discount 1000/1000) is below κ·V = 20","only 0 of backing is unreserved; κ·V = 20 is needed"],"required":20000000,"reservable":{"Finite":0}}
```

## Agent Charters use it too

A charter can require evidence above a threshold: a payment larger than
`evidence_above` must cite a claim that passes the decision rule under the
principal's `evidence_policy`, for the payment's amount. See
[Agent Charters](../agent-charters.md).
