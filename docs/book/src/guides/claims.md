# Issuing and assessing claims

A claim is a statement with a formula of the assumptions it rests on. This
guide issues one, looks at what the ledger records, prices it with the
decision rule and relies on it. The concepts are in
[Claims and the knowledge log](../concepts/claims.md) and
[The Assurance Algebra](../concepts/assurance/index.md). Every output below
comes from one devnet session.

## Issue a claim

```text
Usage: sion claim issue [OPTIONS] <FROM> <SUBJECT>

Arguments:
  <FROM>     Issuer's wallet
  <SUBJECT>  One-line statement

Options:
      --schema <SCHEMA>            Schema [default: note/v1]
      --bond <BOND>                Rest it on your bond of this label
      --adjudicator <ADJUDICATOR>  Adjudicator atom ID
      --valid-for <VALID_FOR>      Validity in Pulses [default: 10000]
```

Without `--bond`, a claim rests only on its issuer's dual signature: the
formula is `module-lattice ∨ sha2-256`, two cryptographic atoms, either of
which protects it. With `--bond <label>`, it also rests on the issuer's
bond of that label, `bond:<issuer>:<label>`: the formula becomes
`(module-lattice ∨ sha2-256) ∧ bond`. The bond must exist when the claim
joins the log ([below](#a-claim-on-a-bond-that-does-not-exist)), and it
must be yours.

Here dana first posts a 50 USD bond, and a judge posts a 200 USD bond as an
adjudicator (see [Bonds and challenges](bonds-and-challenges.md)). Then
dana issues a claim on her bond, naming the judge:

```text
$ sion claim issue dana "Shipment 7 arrived at the buyer's dock" --schema trade.delivery/v1 --bond deliveries --adjudicator c007f9d7ea8bd2ce4000c8a422629a482b03e9bfbada1c910f1c257d7f4202c5 --valid-for 5000
dana signs: Publish the claim "Shipment 7 arrived at the buyer's dock" (trade.delivery/v1), valid for 5000 Pulses, resting on 3 assumptions and 0 other claims. Fee 0.0011 USD; valid until Pulse 39.
Pulse 22: Done: claim 9cfc2803 submitted; fee 0.0011 USD.
  digest c048b93dfe52393aa03c4ca8dadaf47d1a01082d1b84f21c6752adcbabc17c04
claim 9cfc28039b98694e547d9cc469a9b550c1aa1ae3e92fbdbeec419e5c4ac2ccb1 (joins the knowledge log at the end of Pulse 22)
```

"3 assumptions" are the two signature families and the bond. The claim's
identifier is the hash of its issuer and body. It joins the knowledge log
in the deferred phase of its Pulse, after every transaction of that Pulse.

## Look at it

`sion claim show` prints the claim, its atoms, its status, its cost of
corruption under the standard policy, and its inclusion proof in the
knowledge log. An excerpt (the payload, the subject's bytes, is cut):

```text
$ sion claim show 9cfc28039b98694e547d9cc469a9b550c1aa1ae3e92fbdbeec419e5c4ac2ccb1
{
  "atoms": [
    …
    {
      "atom": {
        "attack_cost": null,
        "bonded": true,
        "class": "Economic",
        "id": "94d5e302f3e8e746e194c7ef107eea0b7e474aaae51409841add560bf804cc5d",
        "label": "bond:dab40d27bb62fa8dc40c1bf8194f6d04bff293ac65dd08ace075a8d6d5cec5be:deliveries"
      },
      "bond": {
        "amount": 50000000,
        "atom": "94d5e302f3e8e746e194c7ef107eea0b7e474aaae51409841add560bf804cc5d",
        "locked_until": 5122,
        "owner": "dab40d27bb62fa8dc40c1bf8194f6d04bff293ac65dd08ace075a8d6d5cec5be",
        "pool": 0,
        "reserved": 0,
        "slashed": 0
      },
      "broken": false
    },
    …
  ],
  "claim": {
    "body": {
      "adjudicator": "c007f9d7ea8bd2ce4000c8a422629a482b03e9bfbada1c910f1c257d7f4202c5",
      "challenge_period": 100,
      "detection_permille": 1000,
      "formula": {
        "And": [
          {
            "Or": [
              {
                "Atom": "e3dbbfda0d9ee89121eae204845d238794d5deb9b6bd1976bd1af37ffd5f0d1d"
              },
              {
                "Atom": "0d0e4fff3d2ddf3afdaf406be3960a6fe406d300a8e4f61fb09a8c72f999c69b"
              }
            ]
          },
          {
            "Atom": "94d5e302f3e8e746e194c7ef107eea0b7e474aaae51409841add560bf804cc5d"
          }
        ]
      },
      "payload": { "Public": [ … ] },
      "schema": "trade.delivery/v1",
      "subject": "Shipment 7 arrived at the buyer's dock",
      "valid_for": 5000
    },
    "id": "9cfc28039b98694e547d9cc469a9b550c1aa1ae3e92fbdbeec419e5c4ac2ccb1",
    "issued_at": 22,
    "issuer": "dab40d27bb62fa8dc40c1bf8194f6d04bff293ac65dd08ace075a8d6d5cec5be",
    "log_index": 0
  },
  "coc_standard_policy": {
    "exact": true,
    "high": {
      "Finite": 50000000
    },
    "low": {
      "Finite": 50000000
    }
  },
  …
  "status": "Active"
}
```

- `locked_until` 5122 is the end of the claim's validity (Pulse 22 + 5000)
  plus its 100-Pulse challenge period: the bond cannot be withdrawn while a
  claim resting on it can still be challenged.
- The cost of corruption is 50 USD: the standard policy trusts the two
  cryptographic atoms, so what an attacker would forfeit is the bond.
- Amounts are micro-units.

## Price it before relying on it

`sion claim decide <claim> <value>` applies the
[decision rule](../concepts/assurance/decision-rule.md) for a value at
risk, with κ = 2 unless you pass `--kappa`:

```text
$ sion claim decide 9cfc28039b98694e547d9cc469a9b550c1aa1ae3e92fbdbeec419e5c4ac2ccb1 10
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
    "Finite": 50000000
  }
}
```

For 30 USD, κ·V is 60, more than the bond:

```text
$ sion claim decide 9cfc28039b98694e547d9cc469a9b550c1aa1ae3e92fbdbeec419e5c4ac2ccb1 30
{
  "accept": false,
  …
  "reasons": [
    "cost of corruption 50 (after detection discount 1000/1000) is below κ·V = 60",
    "only 50 of backing is unreserved; κ·V = 60 is needed"
  ],
  "required": 60000000,
  …
}
```

| Field | Meaning |
|---|---|
| `coc` | The cost-of-corruption interval under the policy; `exact` when the low and high bounds agree |
| `detection_adjusted` | The cost after the claim's detection discount (`detection_permille`) |
| `adjudicator_cap` | The adjudicator's own bond: a ruling cannot be trusted beyond what the adjudicator forfeits |
| `effective` | The smaller of the two |
| `reservable` | Backing not yet reserved by others |
| `required` | κ·V |
| `accept` | Whether `effective` and `reservable` both reach κ·V |

`--kappa 5` asks for more margin (κ·V = 50 for a value of 10, which the
bond just covers). `--distrust <atom>` stops trusting an atom the policy
would otherwise trust: its cost becomes its bond plus its known attack
cost, instead of infinite. Distrusting dana's bond atom changes nothing
(its cost was its bond already); distrusting both signature families of a
claim that rests only on them brings its cost to 0. The MCP tool
`sion_assess_claim` takes the same options
([MCP tools](../reference/mcp.md#sion_assess_claim)).

## Rely on it

`sion accept` applies the decision rule on the ledger and, if it passes,
reserves κ·V of the claim's backing for you until a Pulse:

```text
$ sion accept erin 9cfc28039b98694e547d9cc469a9b550c1aa1ae3e92fbdbeec419e5c4ac2ccb1 10 --pulses 500
erin signs: Rely on claim 9cfc2803 for up to 10 USD until Pulse 523, reserving its backing. Fee 0.0012 USD; valid until Pulse 43.
Pulse 26: Done: reserved 20 USD of claim 9cfc2803's backing; fee 0.0012 USD.
```

The same decision afterwards shows less backing left for anyone else:

```text
$ sion claim decide 9cfc28039b98694e547d9cc469a9b550c1aa1ae3e92fbdbeec419e5c4ac2ccb1 10
{
  "accept": true,
  …
  "reservable": {
    "Finite": 30000000
  }
}
```

If the claim proves false, erin is compensated from the slashed bond up to
the value she reserved for; see [Bonds and challenges](bonds-and-challenges.md).
If the decision rule fails on the ledger, the transaction fails and only
the fee is charged:

```text
Pulse 22: Failed (invalid transaction: claim not accepted: cost of corruption 10 (after detection discount 1000/1000) is below κ·V = 200; only 10 of backing is unreserved; κ·V = 200 is needed); nothing happened except the fee of 0.0011 USD.
```

## A claim on a bond that does not exist

The checks on a claim's formula run when it joins the log, in the deferred
phase, not when the transaction executes. A claim resting on a bond that
was never posted is submitted, and then rejected:

```text
$ sion claim issue alice "Cargo 12 is insured" --bond cargo
alice signs: Publish the claim "Cargo 12 is insured" (note/v1), valid for 10000 Pulses, resting on 3 assumptions and 0 other claims. Fee 0.0011 USD; valid until Pulse 20.
Pulse 2: Done: claim 746b061e submitted; fee 0.0011 USD.
  digest cb2eb5807e930d7b2ecaa3723efc04f1dbbd93f540ba150d713839b35f2a752d
claim 746b061e50894c72c1799e253da547496dc914a5297ff82eed6cd83dc264e05a (joins the knowledge log at the end of Pulse 2)

$ sion events --from 2 --limit 5
Pulse     2  {"ClaimSubmitted":{"claim":"746b061e50894c72c1799e253da547496dc914a5297ff82eed6cd83dc264e05a"}}
Pulse     2  {"ClaimRejected":{"claim":"746b061e50894c72c1799e253da547496dc914a5297ff82eed6cd83dc264e05a","reason":"unknown atom b758c281aa42036717b91a0324bf7530786e85ccfbdfc71dc0b629c9424ca309"}}

$ sion claim show 746b061e50894c72c1799e253da547496dc914a5297ff82eed6cd83dc264e05a
error: unknown claim
```

The CLI reports the submission as done because the transaction succeeded;
check the events (or `sion claim show`) to see whether the claim joined the
log. Post the bond first, then issue the claim.

## Claims from agents

AI agents issue and assess claims through the MCP tools
`sion_issue_claim` and `sion_assess_claim`, and every payment under an
Agent Charter can issue a receipt claim automatically
([Charters for AI agents](charters.md)).
