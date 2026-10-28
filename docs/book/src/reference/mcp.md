# MCP tools

`sion mcp` is a [Model Context Protocol](https://modelcontextprotocol.io)
server for AI agents. [Connect an AI agent over MCP](../getting-started/mcp.md)
shows how to add it to a client. This page lists every tool, with its input
schema as `tools/list` returns it and, for all but the Task Charter tools
(whose setup the [guide](../guides/tasks.md) walks through), real calls
from recorded sessions against a devnet.

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

## sion_task_post

Post a task (Task Charters): escrow money for work whose acceptance rule is fixed now. A result is paid only if it rests on every required claim (for example an environment vendor's bonded 'no known exploit' claim) and on the worker's bond or a named verifier's attestation, and passes the poster's decision rule; κ times the payment of its backing is then reserved, so a result later proven false compensates the poster. Without worker, the task is a bounty anyone may settle. With charter and allotment, the escrow comes from that agent's budget under its caveats.

| Argument | Type | Required | Meaning |
|---|---|---|---|
| `adjudicator` | string | no | Adjudicator atom id (hex) results must name |
| `allotment` | string | no | The agent's allotment (hex), with charter |
| `amount_usd` | number | yes | Escrow, paid in full for an accepted result |
| `charter` | string | no | Post from this agent's budget (charter id, hex) |
| `deadline_pulses` | integer | no | Pulses until the deadline (default 1000) |
| `detection_permille` | integer | no | Largest detection factor results may declare (default 900) |
| `kappa` | number | no | Safety factor κ (default 2) |
| `reliance_pulses` | integer | no | Pulses the backing of a paid result stays reserved (default 1000) |
| `requires` | array of string | no | Claim ids every result must rest on |
| `spec` | string | no | The task's description; its bytes are committed |
| `verifiers` | array of string | no | Verifiers whose attestations count (wallets or accounts) |
| `wallet` | string | yes | The poster's wallet (the agent's, with charter) |
| `worker` | string | no | The worker (wallet or account); omit for an open bounty |

See [Task Charters from the command line](../guides/tasks.md) for the same steps as commands.

## sion_task_attest

Attest, as a verifier the task names, that an output passes: issues an attestation claim resting on the verifier's bond bond:<verifier>:verifier (or bond_label).

| Argument | Type | Required | Meaning |
|---|---|---|---|
| `bond_label` | string | no |  |
| `output` | string | yes | The output; its bytes are committed |
| `task` | string | yes | Task id (hex) |
| `wallet` | string | yes |  |

See [Task Charters from the command line](../guides/tasks.md) for the same steps as commands.

## sion_task_submit

Submit a result for a task: issues a result claim with exactly the formula the task requires, over the attestations given (claim ids, in order). The worker needs a task bond (bond:<worker>:task-worker), however small.

| Argument | Type | Required | Meaning |
|---|---|---|---|
| `attestations` | array of string | no |  |
| `output` | string | yes | The output; its bytes are committed |
| `task` | string | yes | Task id (hex) |
| `wallet` | string | yes |  |

See [Task Charters from the command line](../guides/tasks.md) for the same steps as commands.

## sion_task_settle

Settle a task with your result claim and its attestations (claim ids, in the order the result lists them). The ledger pays only if the result passes the rule fixed when the task was posted, and only once.

| Argument | Type | Required | Meaning |
|---|---|---|---|
| `attestations` | array of string | no |  |
| `result` | string | yes | Result claim id (hex) |
| `task` | string | yes | Task id (hex) |
| `wallet` | string | yes |  |

See [Task Charters from the command line](../guides/tasks.md) for the same steps as commands.

## sion_task_show

Show a task: poster, worker, escrow, deadline, the acceptance rule and its state.

| Argument | Type | Required | Meaning |
|---|---|---|---|
| `task` | string | yes |  |

See [Task Charters from the command line](../guides/tasks.md) for the same steps as commands.

## sion_memory_open

Open a memory log on the ledger: an append-only log of memory atoms kept off the ledger, whose head the ledger moves only along proofs that the log grew. With agent, the log belongs to that agent identity (the wallet must be its agent) and cannot grow while the agent is suspended.

| Argument | Type | Required | Meaning |
|---|---|---|---|
| `agent` | string | no | Agent identity id (hex) |
| `wallet` | string | yes |  |

Call:

```json
{"name": "sion_memory_open", "arguments": {"wallet": "robot-7"}}
```

Result (`structuredContent`):

```json
{
  "log": "5bc688d4a6571cfe9e3dfda1a1cda7bdd76bb77257eb822381500a2ef5e2c93d",
  "pulse": 13,
  "receipt": "Done: memory log 5bc688d4 opened; fee 0.0012 USD.",
  "status": "Success"
}
```

## sion_memory_remember

Record a memory atom in a log's local store: a statement (subject, predicate, value) with when it held and, for robots, where (a box in a frame, in millimetres). It reaches the ledger's commitment at the next sion_memory_commit.

| Argument | Type | Required | Meaning |
|---|---|---|---|
| `at` | array of integer | no | Its place: x, y, z in millimetres |
| `confidence_permille` | integer | no | Confidence in thousandths (default 1000) |
| `frame` | string | no | The frame its place is in, for example warehouse-3 |
| `from_s` | integer | no | When it began to hold, seconds since the Unix epoch (default now) |
| `kind` | string | no | observation (default), coverage, world-state, semantic, episodic, procedural, preference, goal, plan, … |
| `log` | string | yes | Memory log id (hex) or a unique prefix |
| `predicate` | string | yes |  |
| `seconds` | integer | no | For how long it held (default open-ended) |
| `size` | array of integer | no | Half the size of its box: dx, dy, dz in millimetres |
| `subject` | string | yes |  |
| `value` | string | yes | true/false, an integer, @name for an entity, or text |
| `wallet` | string | yes |  |

Call:

```json
{"name": "sion_memory_remember", "arguments": {"wallet": "robot-7", "log": "5bc688d4", "subject": "pallet-17", "predicate": "at", "value": "@aisle-4", "frame": "warehouse-3", "at": [41000, 11000, 500], "size": [600, 400, 500], "from_s": 1790812800, "seconds": 30, "confidence_permille": 900}}
```

Result (`structuredContent`):

```json
{
  "id": "9622667160c390e3e39ec78984b160a21f08b18f760a287c77105241eadeaa6b",
  "log": "5bc688d4a6571cfe9e3dfda1a1cda7bdd76bb77257eb822381500a2ef5e2c93d",
  "seq": 0,
  "size": 1,
  "uncommitted": 1
}
```

Call:

```json
{"name": "sion_memory_remember", "arguments": {"wallet": "robot-7", "log": "5bc688d4", "subject": "dock-door-2", "predicate": "open", "value": "true", "kind": "world-state", "from_s": 1790812808}}
```

Result (`structuredContent`):

```json
{
  "id": "cd354fc96f13118cc26347040956d92a88a63835b4d2de80cfa17c0bc68c285c",
  "log": "5bc688d4a6571cfe9e3dfda1a1cda7bdd76bb77257eb822381500a2ef5e2c93d",
  "seq": 1,
  "size": 2,
  "uncommitted": 2
}
```

## sion_memory_commit

Commit a memory log's local head to the ledger with a proof that it extends the committed one. The Pulse it executes in becomes the transaction time of the atoms it adds.

| Argument | Type | Required | Meaning |
|---|---|---|---|
| `log` | string | yes | Memory log id (hex) or a unique prefix |
| `seal` | boolean | no | Close the log for good |
| `wallet` | string | yes |  |

Call:

```json
{"name": "sion_memory_commit", "arguments": {"wallet": "robot-7", "log": "5bc688d4"}}
```

Result (`structuredContent`):

```json
{
  "pulse": 16,
  "receipt": "Done: memory log 5bc688d4 now holds 2 atoms (head 1); fee 0.0011 USD.",
  "status": "Success"
}
```

## sion_memory_recall

Everything a memory log had recorded in a box of a frame during an interval, as of its committed head, with a proof of completeness checked against the index root the ledger holds (an empty answer is proved too).

| Argument | Type | Required | Meaning |
|---|---|---|---|
| `frame` | string | yes |  |
| `from_s` | integer | yes | Interval start, seconds since the Unix epoch |
| `log` | string | yes | Memory log id (hex) or a unique prefix |
| `max` | array of integer | yes | Upper corner x, y, z in millimetres |
| `min` | array of integer | yes | Lower corner x, y, z in millimetres |
| `until_s` | integer | yes | Interval end (exclusive) |

Call:

```json
{"name": "sion_memory_recall", "arguments": {"log": "5bc688d4", "frame": "warehouse-3", "min": [40000, 10000, 0], "max": [42000, 12000, 3000], "from_s": 1790812800, "until_s": 1790812860}}
```

Result (`structuredContent`):

```json
{
  "as_of": {
    "index": "e2f53bbcd00b673fd659c051eeffeacd19b0f3503249b1ab5346039a7c4da018",
    "pulse": 16,
    "size": 2
  },
  "atoms": [
    {
      "confidence_permille": 900,
      "id": "9622667160c390e3e39ec78984b160a21f08b18f760a287c77105241eadeaa6b",
      "kind": "observation",
      "seq": 0,
      "statement": "pallet-17 at aisle-4",
      "valid_from_s": 1790812800,
      "valid_until_s": 1790812830
    }
  ],
  "log": "5bc688d4a6571cfe9e3dfda1a1cda7bdd76bb77257eb822381500a2ef5e2c93d",
  "proof_bytes": 327,
  "verified": "complete against the index root the ledger holds"
}
```

## sion_stake_pools

Every validator with its own stake, its pool of delegated stake (and how much of it earns), its commission and its weight in the active set.

No arguments.

Call:

```json
{"name": "sion_stake_pools", "arguments": {}}
```

Result (`structuredContent`):

```json
{
  "pools": [
    "epoch 0, ending after Pulse 12; stake asset USD (test stake)",
    "validator 91fa8cbb  operator operator-1  active  own 100,000  delegated 0 (earning 0)  commission 0%  weight 100000",
    "validator 9c813b07  operator operator-3  active  own 100,000  delegated 0 (earning 0)  commission 0%  weight 100000",
    "validator d7c1b77d  operator operator-0  active  own 100,000  delegated 0 (earning 0)  commission 0%  weight 100000",
    "validator f28a5456  operator operator-2  active  own 100,000  delegated 0 (earning 0)  commission 0%  weight 100000"
  ]
}
```

## sion_stake_delegate

Delegate stake to a validator's pool without handing it over: the wallet keeps a delegation object holding pool shares. It counts towards the validator's weight and earns a share of its rewards, less the validator's commission, from the end of this epoch; it shares the validator's slashing risk (a tenth of the pool on equivocation). Refused if it would give the validator more than twice an equal share of the bonded stake.

| Argument | Type | Required | Meaning |
|---|---|---|---|
| `amount` | number | yes | Stake, in the stake asset (at least 1) |
| `validator` | string | yes | Validator id (hex) or a unique prefix |
| `wallet` | string | yes |  |

Call:

```json
{"name": "sion_stake_delegate", "arguments": {"wallet": "alice", "validator": "d7c1b77d", "amount": 5000}}
```

Result (`structuredContent`):

```json
{
  "events": [
    {
      "Delegated": {
        "amount": 5000000000,
        "delegation": "59e26b64b3d1183081dc7f252445f320d888e0eb821d1bf3297af89d3beee293",
        "owner": "42424444042f6eae3f1f69e9d99e163216bd317aed703c8eb19a98346b1eeb23",
        "shares": 5000000000,
        "validator": "d7c1b77db0ae57df54351fc2e4c60d8557ec8a9ba3f01e567431a45fb8198c59"
      }
    }
  ],
  "pulse": 9,
  "receipt": "Done: alice delegated 5,000 USD to validator d7c1b77d as delegation 59e26b64, earning from the epoch's end; fee 0.0011 USD.",
  "status": "Success"
}
```

Call:

```json
{"name": "sion_stake_delegate", "arguments": {"wallet": "alice", "validator": "d7c1b77d", "amount": 0.5}}
```

Result (`structuredContent`):

```json
{
  "events": [],
  "pulse": 11,
  "receipt": "Failed (invalid transaction: a delegation must be at least 1); nothing happened except the fee of 0.0011 USD.",
  "status": {
    "Failed": "invalid transaction: a delegation must be at least 1"
  }
}
```

## sion_stake_show

An account's delegations: validator, shares, the stake behind them, state (pending, earning or unbonding) and the rewards owed now.

| Argument | Type | Required | Meaning |
|---|---|---|---|
| `account` | string | yes | A wallet name (devnet) or a 64-digit hex account ID |

Call:

```json
{"name": "sion_stake_show", "arguments": {"account": "alice"}}
```

Result (`structuredContent`):

```json
{
  "account": "42424444042f6eae3f1f69e9d99e163216bd317aed703c8eb19a98346b1eeb23",
  "delegations": [
    {
      "delegation": "1f2c13cae58b162e4ce60c58dadb97c5118c9cd12738b3279b7db07b04220a29",
      "phase": "pending",
      "rewards_owed": "0",
      "shares": "5000000000",
      "since": 7,
      "stake": "5,000",
      "state": "pending: earns from the end of this epoch",
      "validator": "d7c1b77db0ae57df54351fc2e4c60d8557ec8a9ba3f01e567431a45fb8198c59",
      "withdrawable_at": null
    }
  ],
  "epoch": 0
}
```

## sion_stake_claim

Collect a delegation's rewards (USD).

| Argument | Type | Required | Meaning |
|---|---|---|---|
| `delegation` | string | yes | Delegation id (hex) or a unique prefix |
| `wallet` | string | yes |  |

## sion_stake_undelegate

Stop a delegation earning: collect its rewards and start unbonding. The stake stays slashable until it is withdrawn after the unbonding period.

| Argument | Type | Required | Meaning |
|---|---|---|---|
| `delegation` | string | yes | Delegation id (hex) or a unique prefix |
| `wallet` | string | yes |  |

## sion_stake_withdraw

Withdraw an unbonded delegation's stake.

| Argument | Type | Required | Meaning |
|---|---|---|---|
| `delegation` | string | yes | Delegation id (hex) or a unique prefix |
| `wallet` | string | yes |  |

## sion_storage_post

Have a file kept by a storage provider (spec v1.12): escrows price × rounds for data whose Merkle root the deal fixes. A provider that accepts posts collateral and must prove every round that it holds leaves picked at random; it is paid per proven round, and loses collateral for missed rounds (all of it if it abandons the deal). Without a provider, any provider may accept. Returns the deal id.

| Argument | Type | Required | Meaning |
|---|---|---|---|
| `challenges` | integer | no | Leaves challenged per round (1 to 16; default 4) |
| `collateral` | number | no | USD the provider must post (default 0) |
| `locator` | string | no | Where providers fetch the data (default file://<path>) |
| `path` | string | yes | The file to have kept |
| `price` | number | yes | USD paid per proven round |
| `provider` | string | no | Wallet name or account of the provider; omit for an open deal |
| `round_length` | integer | no | Pulses per round (8 to 1,000,000; default 20) |
| `rounds` | integer | yes | Rounds (1 to 1,000,000) |
| `wallet` | string | yes |  |

Call:

```json
{"name": "sion_storage_post", "arguments": {"wallet": "robot-7", "path": "…/patrol-archive.bin", "price": 0.25, "rounds": 4, "round_length": 10, "collateral": 1}}
```

Result (`structuredContent`):

```json
{
  "deal": "b33840bf8bdca7c9a00b8559c4b4f48725cab1daa257c3feb05e42e41c98e68e",
  "leaves": 5,
  "pulse": 9,
  "receipt": "Done: storage deal b33840bf posted: 5 leaves, 4 rounds at 0.25 USD each, 1 USD of collateral, open to any provider; fee 0.0012 USD.",
  "root": "89ba09bb4f58b297917884e1102400263c6635d0a9f26903566fc60d78cf9a7f",
  "status": "Success"
}
```

## sion_storage_deals

Storage deals with their terms, state and, once accepted, the provider's progress (rounds proven, the round under way, the end).

| Argument | Type | Required | Meaning |
|---|---|---|---|
| `deal` | string | no | One deal (id or unique prefix) |
| `open` | boolean | no | Only deals still open for acceptance |

Call:

```json
{"name": "sion_storage_deals", "arguments": {"open": true}}
```

Result (`structuredContent`):

```json
{
  "deals": [
    {
      "accept_by": 207,
      "challenges": 4,
      "client": "2dbd6e3f6651a35a4e623838bd7e21f89b2fea15c38d66824cb72e6d111226da",
      "collateral": "1",
      "deal": "b33840bf8bdca7c9a00b8559c4b4f48725cab1daa257c3feb05e42e41c98e68e",
      "escrow": "1",
      "leaves": 5,
      "locator": "file://…/patrol-archive.bin",
      "price": "0.25",
      "provider": null,
      "root": "89ba09bb4f58b297917884e1102400263c6635d0a9f26903566fc60d78cf9a7f",
      "round_length": 10,
      "rounds": 4,
      "state": {
        "phase": "open"
      }
    }
  ]
}
```

Call:

```json
{"name": "sion_storage_deals", "arguments": {"deal": "b33840bf"}}
```

Result (`structuredContent`):

```json
{
  "accept_by": 207,
  "challenges": 4,
  "client": "2dbd6e3f6651a35a4e623838bd7e21f89b2fea15c38d66824cb72e6d111226da",
  "collateral": "1",
  "deal": "b33840bf8bdca7c9a00b8559c4b4f48725cab1daa257c3feb05e42e41c98e68e",
  "escrow": "0",
  "leaves": 5,
  "locator": "file://…/patrol-archive.bin",
  "price": "0.25",
  "provider": null,
  "root": "89ba09bb4f58b297917884e1102400263c6635d0a9f26903566fc60d78cf9a7f",
  "round_length": 10,
  "rounds": 4,
  "state": {
    "at": 13,
    "forfeited": "0",
    "missed": 0,
    "phase": "closed",
    "proven": 0,
    "refund": "1"
  }
}
```

## sion_storage_close

Close a storage deal: the client refunds an open deal, or ends an accepted one once it is over or once its provider has missed three rounds in a row (the provider then forfeits its collateral to the treasury); the provider may close one that is over. Pays the provider for proven rounds and refunds the escrow of unproven ones.

| Argument | Type | Required | Meaning |
|---|---|---|---|
| `deal` | string | yes | Deal id (hex) or a unique prefix |
| `wallet` | string | yes |  |

Call:

```json
{"name": "sion_storage_close", "arguments": {"wallet": "alice", "deal": "b33840bf"}}
```

Result (`structuredContent`):

```json
{
  "events": [],
  "pulse": 11,
  "receipt": "Failed (unauthorized: only the client closes an open deal); nothing happened except the fee of 0.0011 USD.",
  "status": {
    "Failed": "unauthorized: only the client closes an open deal"
  }
}
```

Call:

```json
{"name": "sion_storage_close", "arguments": {"wallet": "robot-7", "deal": "b33840bf"}}
```

Result (`structuredContent`):

```json
{
  "events": [
    {
      "DealClosed": {
        "deal": "b33840bf8bdca7c9a00b8559c4b4f48725cab1daa257c3feb05e42e41c98e68e",
        "forfeited": 0,
        "missed": 0,
        "paid": 0,
        "refund": 1000000,
        "returned": 0
      }
    }
  ],
  "pulse": 13,
  "receipt": "Done: storage deal b33840bf closed: 1 USD refunded, 0 USD paid, 0 USD of collateral returned, 0 USD forfeited, 0 round(s) missed; fee 0.0011 USD.",
  "status": "Success"
}
```
