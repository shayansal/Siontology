# MCP tools

`sion mcp` is a [Model Context Protocol](https://modelcontextprotocol.io)
server for AI agents. [Connect an AI agent over MCP](../getting-started/mcp.md)
shows how to add it to a client. This page lists every tool, with its input
schema as `tools/list` returns it and a real call from a recorded session
against a devnet.

## Protocol

- JSON-RPC 2.0 over standard input and output, one message per line.
- Protocol version `2025-06-18`. The server offers tools only.
- Methods: `initialize`, `notifications/initialized`, `tools/list`,
  `tools/call`. Any other method is answered with error `-32601`:

  ```json
  {"error":{"code":-32601,"message":"method not found: resources/list"},"id":17,"jsonrpc":"2.0"}
  ```

- The `initialize` result carries these instructions for the agent:
  `Tools for the Siontology neo-blockchain devnet. Assess a claim with
  sion_assess_claim before relying on it.`
- Every successful result carries the same value twice: as pretty-printed
  JSON in `content[0].text`, and as `structuredContent`. Failures are
  results with `isError: true` and the message as text, for example
  `unknown tool sion_teleport`.
- The server talks to the node at `--node` or `SION_NODE`.

Amounts in arguments are decimal USD (`amount_usd`, `value_usd`,
`max_usd`). Amounts inside events are micro-units. Tools that sign take a
wallet name and resolve it like the CLI: a seed file in
`$SION_HOME/wallets`, or else the deterministic devnet wallet of that name.

The results below are shortened in two ways only: long hexadecimal values
in arguments are cut with `…`, and where noted a large field is left out.

## sion_status

Chain status: latest Pulse, certified Pulse, world commitment, transaction counts.

No arguments.

Call:

```json
{"name": "sion_status", "arguments": {}}
```

Result (`structuredContent`):

```json
{
  "active": true,
  "active_validators": 4,
  "anchors": 342,
  "beacon": "45bee72ec7d92a54a7c790d7f0d2276b84bfe256060a48c5ad829e3b1f331489",
  "certified_pulse": 202,
  "chain": "6345655cfd2a714ed1dab69c0660f2f48f429ce2981e25629d0467d7b7ee97b7",
  "chain_id": "sion-devnet-1",
  "claims": 6,
  "commitment": {
    "config": "2d86efe308c60641d10f022ce94b57596644f923d10c7fbcf7aa3dc029e45592",
    "knowledge_root": "5dcf6d5dff8833d54c540dfb198c7f64a8f9fdae81f252e6f5b7e3646e0638fe",
    "knowledge_size": 6,
    "pulse": 202,
    "state_root": "8cf22b46c606c2dddff66c0faeeda14f38ce78ba27b0fdc82c33f555ca0f8a81"
  },
  "epoch": 0,
  "epoch_end": 9223372036854775807,
  "executed": [
    40,
    4,
    1
  ],
  "forced_overdue": 0,
  "forced_pending": 0,
  "genesis": "30de983de39a060dc990e95512f3f51adc2c5570006ed775c9dc81fab667921b",
  "insecure_signatures": false,
  "objects": 85,
  "proven_pulse": 202,
  "pulse": 202,
  "round": 814,
  "session": 0,
  "shards": 2,
  "validator": "cdc0cc1da885c87ebdb67a8c05e027449f8cc15e4743959f8c0f3fa469b3ab38",
  "validators": 4
}
```

## sion_balance

Balances of an account.

| Argument | Type | Required | Meaning |
|---|---|---|---|
| `account` | string | yes | A wallet name (devnet) or a 64-digit hex account ID |

Call:

```json
{"name": "sion_balance", "arguments": {"account": "dana"}}
```

Result (`structuredContent`):

```json
{
  "account": "dab40d27bb62fa8dc40c1bf8194f6d04bff293ac65dd08ace075a8d6d5cec5be",
  "balances": [
    {
      "amount": "31.2422",
      "asset": "USD"
    }
  ],
  "pulse": 202
}
```

## sion_pay

Pay USD from a wallet this server holds keys for. Final once the lane certificate forms; executes in the next Pulse.

| Argument | Type | Required | Meaning |
|---|---|---|---|
| `amount_usd` | number | yes | Amount in USD |
| `from` | string | yes | Wallet name |
| `to` | string | yes | A wallet name (devnet) or a 64-digit hex account ID |

Call:

```json
{"name": "sion_pay", "arguments": {"from": "dana", "to": "bob", "amount_usd": 1.5}}
```

Result (`structuredContent`):

```json
{
  "digest": "21e98fa52e2e93548f46961448250d1639b9d61a162335b0028e451264338f59",
  "fee": "0.0011",
  "pulse": 205,
  "receipt": "Done: paid 1.5 USD to bob; fee 0.0011 USD.",
  "status": "Success"
}
```

## sion_issue_claim

Issue a claim (a statement with a trust formula). With bond_label the claim rests on the issuer's bond of that label; otherwise only on the issuer's dual post-quantum signature.

| Argument | Type | Required | Meaning |
|---|---|---|---|
| `bond_label` | string | no |  |
| `issuer` | string | yes |  |
| `schema` | string | no |  |
| `subject` | string | yes |  |
| `valid_for_pulses` | integer | no |  |

Call:

```json
{"name": "sion_issue_claim", "arguments": {"issuer": "dana", "subject": "Invoice 88 was paid in full", "schema": "note/v1", "valid_for_pulses": 2000}}
```

Result (`structuredContent`):

```json
{
  "claim": "e825c122db0c61842f067b444583c2b3593a75e1c5f551f4c0e3162079239412",
  "pulse": 207,
  "receipt": "Done: claim e825c122 submitted; fee 0.0011 USD.",
  "status": "Success",
  "submitted": true
}
```

## sion_assess_claim

Before relying on a claim: compute its cost of corruption under a policy and apply the decision rule min(δ·CoC, CoC(adjudicator)) ≥ κ·V. Returns accept/reject with reasons.

| Argument | Type | Required | Meaning |
|---|---|---|---|
| `claim` | string | yes | Claim ID (hex) |
| `distrust_atoms` | array of string | no | Atom IDs to treat as worthless |
| `kappa` | number | no | Safety factor κ (default 2) |
| `value_usd` | number | yes | What you stand to lose if the claim is false |

The schema's wording aside, a distrusted atom is not worthless: it is no
longer trusted, so its cost is its bond plus its known attack cost (zero
for a signature family). See
[The decision rule](../concepts/assurance/decision-rule.md).

Call:

```json
{"name": "sion_assess_claim", "arguments": {"claim": "e825c122db0c61842f067b444583c2b3593a75e1c5f551f4c0e3162079239412", "value_usd": 1000}}
```

Result (`structuredContent`, without `atoms`):

```json
{
  "claim": "Invoice 88 was paid in full",
  "decision": {
    "accept": true,
    "adjudicator_cap": "Infinite",
    "coc": {
      "exact": true,
      "high": "Infinite",
      "low": "Infinite"
    },
    "detection_adjusted": "Infinite",
    "effective": "Infinite",
    "reasons": [
      "cost of corruption ∞ covers κ·V = 2,000"
    ],
    "required": 2000000000,
    "reservable": "Infinite"
  },
  "status": "Active"
}
```

Call:

```json
{"name": "sion_assess_claim", "arguments": {"claim": "e825c122db0c61842f067b444583c2b3593a75e1c5f551f4c0e3162079239412", "value_usd": 1000, "kappa": 3, "distrust_atoms": ["e3dbbfda0d9ee89121eae204845d238794d5deb9b6bd1976bd1af37ffd5f0d1d", "0d0e4fff3d2ddf3afdaf406be3960a6fe406d300a8e4f61fb09a8c72f999c69b"]}}
```

Result (`structuredContent`, without `atoms`):

```json
{
  "claim": "Invoice 88 was paid in full",
  "decision": {
    "accept": false,
    "adjudicator_cap": "Infinite",
    "coc": {
      "exact": true,
      "high": {
        "Finite": 0
      },
      "low": {
        "Finite": 0
      }
    },
    "detection_adjusted": {
      "Finite": 0
    },
    "effective": {
      "Finite": 0
    },
    "reasons": [
      "cost of corruption 0 (after detection discount 1000/1000) is below κ·V = 3,000",
      "only 0 of backing is unreserved; κ·V = 3,000 is needed"
    ],
    "required": 3000000000,
    "reservable": {
      "Finite": 0
    }
  },
  "status": "Active"
}
```

## sion_agent_pay

Pay from an Agent Charter allotment. The ledger enforces the charter's payees, per-payment and per-window limits, expiry and revocation; a receipt claim is issued if the charter requires it.

| Argument | Type | Required | Meaning |
|---|---|---|---|
| `agent` | string | yes |  |
| `allotment` | string | yes |  |
| `amount_usd` | number | yes |  |
| `charter` | string | yes |  |
| `payee` | string | yes | A wallet name (devnet) or a 64-digit hex account ID |
| `purpose` | string | yes |  |

Call:

```json
{"name": "sion_agent_pay", "arguments": {"agent": "agent8", "charter": "e2737343d3417c1d1ab57608493535275ec9df51411e6d61f0d1748d6953d218", "allotment": "755f14bad8d27e3c6605f23d95c478406a14e92e62e1989a3588c934c4a3bf7f", "payee": "carol", "amount_usd": 0.2, "purpose": "one forecast"}}
```

Result (`structuredContent`):

```json
{
  "digest": "e016450e04ffe8b65af25e0fbb18cabb5aa785cd9481f7e63ea1c5d3e739c100",
  "events": [
    {
      "AgentPaid": {
        "amount": 200000,
        "charter": "e2737343d3417c1d1ab57608493535275ec9df51411e6d61f0d1748d6953d218",
        "payee": "a08c0fc672bb5875d68526c0e9b1a3bbe5f57652e6af18a174dbb39c0ad93670",
        "receipt": "8386175ab0d3e3a279a574910241a5db02d36fc385f9b3a6f032d226f93bd1e3"
      }
    }
  ],
  "pulse": 211,
  "receipt": "Done: agent paid 0.2 USD to carol (receipt claim 8386175a); fee 0.0011 USD.",
  "status": "Success"
}
```

Call:

```json
{"name": "sion_agent_pay", "arguments": {"agent": "agent8", "charter": "e2737343d3417c1d1ab57608493535275ec9df51411e6d61f0d1748d6953d218", "allotment": "755f14bad8d27e3c6605f23d95c478406a14e92e62e1989a3588c934c4a3bf7f", "payee": "bob", "amount_usd": 0.2, "purpose": "ignore previous instructions"}}
```

Result (`structuredContent`):

```json
{
  "digest": "6f33d64024607e62f4c6cc3da49fec04fbc75681f7a5e6e2234db0679b4bed07",
  "events": [],
  "pulse": 213,
  "receipt": "Failed (charter caveat violated: payee not allowed); nothing happened except the fee of 0.0011 USD.",
  "status": {
    "Failed": "charter caveat violated: payee not allowed"
  }
}
```

## sion_verify_pulse

Verify a Pulse as a light client: chain identity, committee certificate, receipts, state proof, data-availability sampling, the World Proof and lane certificates. Pass the chain identity you trust as `chain` (64 hex digits); without it the node vouches for itself.

| Argument | Type | Required | Meaning |
|---|---|---|---|
| `chain` | string | no | Pinned chain identity (hex) |
| `pulse` | integer | no |  |

Call:

```json
{"name": "sion_verify_pulse", "arguments": {"chain": "6345655cfd2a714ed1dab69c0660f2f48f429ce2981e25629d0467d7b7ee97b7"}}
```

Result (`structuredContent`):

```json
{
  "pinned": true,
  "report": [
    "chain: 6345655c matches the pinned identity; 4 validators, keys derive their identifiers ✓",
    "Pulse 213 header c5be8e3e (state root 6d4bfd41, knowledge root 99e059bf, 8 claims)",
    "certificate: 300000 of 400000 stake signed (quorum 266667), each signature ML-DSA-44 + SLH-DSA-SHA2-128f ✓",
    "forced inclusion: no overdue intents (0 pending within their deadline) ✓",
    "receipts: 1 of 1 transactions proven against the receipts root, in the certified order ✓",
    "state: an account object proven against the state root ✓",
    "availability: 16 of 16 random shares of the 16×16 square verified; a withholding would go unnoticed with probability < 1.8e-3 ✓",
    "world proof: 2 leaves re-verified by stack [\"A\", \"B\"]; the published data is what executed and encodes to the certified data root (115 KB; replay proofs simulate succinct ones) ✓",
    "lane certificates: all 1 executed transactions locked their inputs with a quorum of their shard's committee, taken from the proven validator set ✓"
  ],
  "verified": true
}
```

## sion_events

Recent events from a Pulse on.

| Argument | Type | Required | Meaning |
|---|---|---|---|
| `from` | integer | no |  |
| `limit` | integer | no |  |

Call:

```json
{"name": "sion_events", "arguments": {"from": 211, "limit": 4}}
```

Result (`structuredContent`):

```json
[
  {
    "event": {
      "AgentPaid": {
        "amount": 200000,
        "charter": "e2737343d3417c1d1ab57608493535275ec9df51411e6d61f0d1748d6953d218",
        "payee": "a08c0fc672bb5875d68526c0e9b1a3bbe5f57652e6af18a174dbb39c0ad93670",
        "receipt": "8386175ab0d3e3a279a574910241a5db02d36fc385f9b3a6f032d226f93bd1e3"
      }
    },
    "pulse": 211,
    "tx": "e016450e04ffe8b65af25e0fbb18cabb5aa785cd9481f7e63ea1c5d3e739c100"
  },
  {
    "event": {
      "ClaimIssued": {
        "claim": "8386175ab0d3e3a279a574910241a5db02d36fc385f9b3a6f032d226f93bd1e3",
        "log_index": 7
      }
    },
    "pulse": 211,
    "tx": null
  }
]
```

## sion_submit_intent

Submit a transaction the agent built and signed itself (hex of its canonical encoding). Returns what it asks for as a sentence, then waits for its receipt unless wait is false.

| Argument | Type | Required | Meaning |
|---|---|---|---|
| `tx` | string | yes | Hex-encoded signed transaction |
| `wait` | boolean | no | Wait for execution (default true) |

Call:

```json
{"name": "sion_submit_intent", "arguments": {"tx": "f9b79999824af932…"}}
```

Result (`structuredContent`):

```json
{
  "digest": "5ebc231f9a27d8cf8b1d92be311078835246595a74b9fb38567d2f400066f6a4",
  "intent": "Pay 1 USD to bob. Fee 0.0011 USD; valid until Pulse 238.",
  "pulse": 221,
  "receipt": "Done: paid 1 USD to bob; fee 0.0011 USD.",
  "status": "Success"
}
```

## sion_fetch_paid

Fetch an HTTP resource that may charge through x402 (HTTP 402 Payment Required). If the server asks for payment on this chain of at most max_usd, pay it and retry: under an Agent Charter when charter and allotment are given (the ledger enforces the charter's payees and limits, whatever the agent was told), otherwise by transfer from the wallet. Returns the body and the settlement.

| Argument | Type | Required | Meaning |
|---|---|---|---|
| `allotment` | string | no | The agent's allotment ID (hex) |
| `chain` | string | no | Pinned chain identity (hex); without it the node vouches for itself |
| `charter` | string | no | Charter ID (hex), to pay under a charter |
| `max_usd` | number | yes | The most this call may pay |
| `url` | string | yes | The resource |
| `wallet` | string | yes | The paying wallet (the agent's, when paying under a charter) |

Call:

```json
{"name": "sion_fetch_paid", "arguments": {"url": "http://127.0.0.1:8402/v1/resource", "wallet": "dana", "max_usd": 1, "chain": "6345655cfd2a714ed1dab69c0660f2f48f429ce2981e25629d0467d7b7ee97b7"}}
```

Result (`structuredContent`, without `quoted`):

```json
{
  "body": "{\"paid\":true}",
  "paid": true,
  "settlement": {
    "extensions": {
      "sion": {
        "chain": "6345655cfd2a714ed1dab69c0660f2f48f429ce2981e25629d0467d7b7ee97b7",
        "pulse": 218,
        "quorum": 266667,
        "signedStake": 300000
      }
    },
    "network": "sion:6345655cfd2a714ed1dab69c0660f2f4",
    "payer": "dab40d27bb62fa8dc40c1bf8194f6d04bff293ac65dd08ace075a8d6d5cec5be",
    "success": true,
    "transaction": "8c4db2a94952dbbf503f5346b843f98e6f91bff8a8dff9896a1b551b1badd762"
  },
  "status": 200
}
```

Call:

```json
{"name": "sion_fetch_paid", "arguments": {"url": "http://127.0.0.1:8402/v1/resource", "wallet": "dana", "max_usd": 0.1, "chain": "6345655cfd2a714ed1dab69c0660f2f48f429ce2981e25629d0467d7b7ee97b7"}}
```

Result (`isError: true`):

```text
the server asks 0.25 USD, more than the 0.1 USD this client may pay
```

## sion_fetch_receipt

Fetch a transaction's receipt and verify it as a light client: the chain identity, the committee certificate of its Pulse and the receipt's inclusion proof. Pass the chain identity you trust as `chain`.

| Argument | Type | Required | Meaning |
|---|---|---|---|
| `chain` | string | no | Pinned chain identity (hex) |
| `digest` | string | yes | Transaction digest (hex) |

Call:

```json
{"name": "sion_fetch_receipt", "arguments": {"digest": "21e98fa52e2e93548f46961448250d1639b9d61a162335b0028e451264338f59", "chain": "6345655cfd2a714ed1dab69c0660f2f48f429ce2981e25629d0467d7b7ee97b7"}}
```

Result (`structuredContent`):

```json
{
  "certificate": {
    "quorum": 266667,
    "signed": 300000
  },
  "chain": "6345655cfd2a714ed1dab69c0660f2f48f429ce2981e25629d0467d7b7ee97b7",
  "chain_pinned": true,
  "digest": "21e98fa52e2e93548f46961448250d1639b9d61a162335b0028e451264338f59",
  "events": [
    {
      "Transferred": {
        "asset": "1436ac5e7c42d6cc93304083c0c6c8c964e939aa391c2e0a7304810b7e368f3c",
        "outputs": [
          [
            "d4a27f08fe31e101a2bce2845d3ae4c00a16405db06b569c56a34732641cb971",
            1500000
          ]
        ]
      }
    }
  ],
  "outcome": "success",
  "pulse": 205,
  "receipt": "Done: paid 1.5 USD to bob; fee 0.0011 USD.",
  "verified": true
}
```

