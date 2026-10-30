# HTTP API

A devnet serves JSON over HTTP on `--api` (default `127.0.0.1:8730`). The
CLI, the MCP server and light clients use it. This page lists every route,
with real responses from a devnet; long values are cut with `…`.

## Conventions

- Requests and responses are JSON (`Content-Type: application/json`).
  Responses carry `Access-Control-Allow-Origin: *`.
- Identifiers (accounts, objects, claims, transactions) are 64 hex digits.
- Amounts are micro-units (10⁻⁶ of the asset), as integers.
- Errors are `{"error": "<message>"}` with status 400 for a bad request and
  404 for anything not found:

```text
$ curl -s http://127.0.0.1:8730/v1/nothing
{"error":"no route for /v1/nothing"}
$ curl -s http://127.0.0.1:8730/v1/accounts/xyz
{"error":"\"xyz\" is not a 64-digit hex identifier"}
```

- Queries are answered by validator 0 from its current state, between
  Pulses. The answer does not prove itself; light clients check what they
  need against certified headers ([Light-client verification](../guides/light-client.md)).

| Method | Path | What |
|---|---|---|
| GET | `/v1/status` | Chain status |
| GET | `/v1/validators` | The chain specification, the validator set and the current committees |
| GET | `/v1/epochs/{e}` | The proof linking epoch `e − 1` to `e` |
| GET | `/v1/genesis` | The genesis specification and its digest |
| GET | `/v1/accounts/{id}` | An account's coins, balances, allotments, delegations and custody records |
| GET | `/v1/objects/{id}` | An object with a state proof |
| GET | `/v1/agents/{id}` | An agent: its charter and the overlays of its lineage, each with a state proof, its status and its bucket (spec v1.14) |
| GET | `/v1/claims/{id}` | A claim, its status, cost of corruption and inclusion proof |
| POST | `/v1/claims/{id}/decide` | The decision rule for a value at risk |
| POST | `/v1/transactions` | Submit a signed transaction |
| POST | `/v1/batches` | Submit a whole batch of transactions under one authorization (spec v1.13) |
| GET | `/v1/transactions/{digest}` | A transaction's status, effects and receipt proof |
| POST | `/v1/faucet` | Devnet faucet |
| GET | `/v1/pulses/latest`, `/v1/pulses/{n}` | A Pulse's header, certificate and summary |
| GET | `/v1/pulses/{n}/data` | A Pulse's data-availability header |
| GET | `/v1/pulses/{n}/data/{row}/{column}` | One data share with its proof |
| GET | `/v1/pulses/{n}/world` | A Pulse's World Proof |
| GET | `/v1/events?from=&limit=` | Events from a Pulse on |
| POST | `/v1/forced` | Post a forced intent, bypassing the gateway |
| GET | `/v1/forced` | Forced-inclusion obligations |
| GET | `/v1/committees` | The current committees and registered keys, Borsh in hex (for a gateway in another process) |
| GET | `/v1/deals` | Storage deals with their state (`?open=1`: only those open for acceptance) |
| POST | `/v1/devnet/censor` | Adversary simulation: censor accounts |

## `GET /v1/status`

```text
$ curl -s http://127.0.0.1:8730/v1/status
{"active":true,"active_validators":4,"anchors":372,"beacon":"487b695b4a93bf659ec40b36c0108348760f20fe711414f9b76073b1e16287a4","certified_pulse":221,"chain":"6345655cfd2a714ed1dab69c0660f2f48f429ce2981e25629d0467d7b7ee97b7","chain_id":"sion-devnet-1","claims":8,"commitment":{"config":"2d86efe308c60641d10f022ce94b57596644f923d10c7fbcf7aa3dc029e45592","knowledge_root":"99e059bfe40288f5b83abfef816535bec4166266d30f638f11438847cb5860b6","knowledge_size":8,"pulse":221,"state_root":"fd80b0d5aa73b4c1820cc6146ff605bf1cf41a473e5ee3ce17f886189d6f275f"},"epoch":0,"epoch_end":9223372036854775807,"executed":[45,5,1],"forced_overdue":0,"forced_pending":0,"genesis":"30de983de39a060dc990e95512f3f51adc2c5570006ed775c9dc81fab667921b","insecure_signatures":false,"objects":92,"proven_pulse":221,"pulse":221,"round":890,"session":0,"shards":2,"validator":"cdc0cc1da885c87ebdb67a8c05e027449f8cc15e4743959f8c0f3fa469b3ab38","validators":4}
```

| Field | Meaning |
|---|---|
| `chain_id` | The chain's configured name |
| `chain` | The chain identity, which headers carry and light clients pin |
| `genesis` | The genesis digest |
| `validator` | The validator answering |
| `active` | Whether that validator is in the active set |
| `active_validators` | Validators in the active set |
| `validators` | Validator threads in the devnet, standby included |
| `epoch`, `epoch_end` | The validator-set epoch and its last Pulse |
| `session` | The consensus session: restart generation × 2³² + epoch |
| `pulse` | The last executed Pulse |
| `certified_pulse` | The last Pulse with a committee certificate |
| `proven_pulse` | The last Pulse with a verified World Proof (`null` without a prover) |
| `commitment` | The current world commitment: Pulse, state root, knowledge root and size, configuration digest |
| `round`, `anchors` | The consensus round in this session, anchors committed |
| `executed` | Transactions executed by this process: successful, failed, rejected |
| `claims`, `objects` | Claims in the knowledge log, objects in the state |
| `shards` | Lane shards |
| `insecure_signatures` | Whether validators sign with the simulation MAC |
| `beacon` | The latest beacon value |
| `forced_pending`, `forced_overdue` | Forced intents within their deadline, and past one |
| `dropped_items` | Consensus payload items this validator refused as larger than a header can carry (`max_payload_bytes`); it should stay 0 |

## `GET /v1/validators`

The chain specification a light client checks against its pin, the
validator set, and the committees derived from it. The response is large:
it includes every registered validator's public keys. Selected fields:

```text
$ curl -s http://127.0.0.1:8730/v1/validators | jq -c '{chain_id, pulse, epoch, epoch_start, epoch_end, session, params}'
{"chain_id":"e2fdac57652ce347915b7fc92c08f327ac785407b2bd956ddff374fd5f335416","pulse":48,"epoch":2,"epoch_start":49,"epoch_end":72,"session":2,"params":{"epoch_length":24,"max_active":100,"min_stake":10000000000,"reporter_permille":50,"reward_permille":700,"seats":4,"stake_asset":"1436ac5e7c42d6cc93304083c0c6c8c964e939aa391c2e0a7304810b7e368f3c","unbonding":8}}
```

| Field | Meaning |
|---|---|
| `chain` | The `ChainSpec`: genesis digest, genesis validators with weights, their keys, shards, seats, insecure flag |
| `chain_id` | The chain identity (here: the hash; in `/v1/status` the same name means the configured name) |
| `genesis`, `insecure_signatures`, `pulse` | As in `/v1/status` |
| `epoch`, `epoch_start`, `epoch_end`, `session` | The current epoch and consensus session |
| `params` | The validator-set parameters ([Configuration](configuration.md#validator-set-validatorparams)) |
| `committee` | The committee that signs Pulse headers: `{chain, epoch, shard, members: [[validator, weight], …], total}` |
| `shards` | Each shard's lane committee, in the same shape, with one seat per member and the lane epoch |
| `keys` | The public keys of every registered validator |
| `rotations` | Shards whose lane committee was replaced this epoch |
| `validators` | One record per registered validator: `validator`, `operator`, `stake`, `rewards`, `status`, `registered_at`, `active` |

A record, as JSON:

```json
{"validator":"a6f32863…","stake":0,"rewards":0,"status":{"Slashed":{"at":34}},"active":false}
```

`status` is `"Joining"`, `"Active"`, `"Leaving"`, `{"Exited":{"withdrawable_at":…}}`,
`{"Slashed":{"at":…}}` or `"Withdrawn"`.

## `GET /v1/epochs/{e}`

The proof a light client needs to move from epoch `e − 1` to epoch `e`:

| Field | Meaning |
|---|---|
| `epoch` | `e` |
| `boundary` | The certificate of epoch `e − 1`'s last Pulse: `{header, signatures}` |
| `validators` | The validator-set object after that Pulse, `{id, object, proof}`, with a sparse-Merkle proof against the header's state root |

```text
$ curl -s http://127.0.0.1:8730/v1/epochs/1 | jq -c '{epoch, boundary_pulse: .boundary.header.pulse, boundary_epoch: .boundary.header.epoch, signers: [.boundary.signatures[][0][0:8]], next_epoch_start: .validators.object.data.Validators.epoch_start}'
{"epoch":1,"boundary_pulse":24,"boundary_epoch":0,"signers":["04dde303","2ed647f1","cdc0cc1d"],"next_epoch_start":25}
```

Errors:

```text
$ curl -s http://127.0.0.1:8730/v1/epochs/0
{"error":"no proof for that epoch (it has not begun, or began before this validator kept proofs)"}
$ curl -s http://127.0.0.1:8730/v1/epochs/x
{"error":"the epoch must be a number"}
```

Epoch 0 has no proof: the chain identity fixes it. Right after an epoch
begins, the proof may answer `the epoch's boundary Pulse is not certified
yet` for a few seconds.

## `GET /v1/genesis`

`{digest, spec}`: the genesis digest and the full genesis specification
(configuration, funded accounts with their policies, validators,
validator-set and forced-inclusion parameters). Selected fields:

```text
$ curl -s http://127.0.0.1:8730/v1/genesis | jq -c '{digest, config: .spec.config, validator_params: .spec.validator_params, forced: .spec.forced}'
{"digest":"30de983de39a060dc990e95512f3f51adc2c5570006ed775c9dc81fab667921b","config":{"base_fee":1000,"chain_id":"sion-devnet-1","challenger_share_permille":500,"faucet":"bfd4410ce0b06f78a2b8454cad0e7073e2ac159ce5022ee8ba52b0187fe4d636","fee_per_kib":100,"max_claim_window":1000000,"max_formula_size":256,"max_payload_bytes":65536,"shard_count":2},"validator_params":{"epoch_length":9223372036854775807,"max_active":100,"min_stake":10000000000,"reporter_permille":50,"reward_permille":700,"seats":4,"stake_asset":"1436ac5e7c42d6cc93304083c0c6c8c964e939aa391c2e0a7304810b7e368f3c","unbonding":20},"forced":{"max_pending":4096,"window":8}}
```

## `GET /v1/accounts/{id}`

`{account, object, coins, balances, allotments, pulse}`: the account object
(`null` if the account has never sent a transaction), its live coins,
balances by asset, and the allotments it can spend.

```text
$ curl -s http://127.0.0.1:8730/v1/accounts/dab40d27bb62fa8dc40c1bf8194f6d04bff293ac65dd08ace075a8d6d5cec5be | jq -c '{account, pulse, balances, coins: [.coins[] | {id: .id[0:8], version, data}]}'
{"account":"dab40d27bb62fa8dc40c1bf8194f6d04bff293ac65dd08ace075a8d6d5cec5be","pulse":222,"balances":[["1436ac5e7c42d6cc93304083c0c6c8c964e939aa391c2e0a7304810b7e368f3c",29488900]],"coins":[{"id":"f9157ec4","version":0,"data":{"Coin":{"amount":29488900,"asset":"1436ac5e7c42d6cc93304083c0c6c8c964e939aa391c2e0a7304810b7e368f3c"}}}]}
```

The account object holds the authorization policy (both public keys, about
1.3 KB), the number of key rotations and the optional viewing key. This
query scans every object, which is slow on a large state (an open issue).

## `GET /v1/objects/{id}`

`{pulse, proof: {id, object, proof}}`: the object (or `null`) and a
sparse-Merkle proof against the state root of `pulse`. For a missing object
the proof proves absence:

```text
$ curl -s http://127.0.0.1:8730/v1/objects/0000000000000000000000000000000000000000000000000000000000000007
{"proof":{"id":"0000000000000000000000000000000000000000000000000000000000000007","object":null,"proof":{"leaf":"Empty","siblings":["137f47b7470d3d9953b74a4f30671395bbb71ae841b1739e0e8159e50596b370","a2cf08fb99e584e306497ea9d6f83fd7d0b2159867227d8cacf897cb272cd700","19c861267c203da37e86cee5d9e3ad47cdb131ebdc97aedd3c675ce5900cae8a","0ad16893e8a24f2fd88b490081192c1d2fd019171260e4ec7fbb9004d9f14010","62b44337253cda9d32a8ebd52eb48b6aca91c1d7934ad0353ecfe99e810b23b0"]}},"pulse":222}
```

An object is `{id, owner, version, data}`. `owner` is `{"Account": …}` or
`{"Shared": <domain>}`; `data` is one of `Coin`, `Account`, `Charter`,
`Allotment`, `Bounded`, `Market`, `Assurance`, `Treasury`, `Schedule`,
`Validators`, `Forced`, `Stream`, `MemoryLog`, `Task`, `Delegations`,
`Delegation`, `Deal`, `Custody`, `Beacon`, `AgentOverlay`, `Cohort` or
`Tombstone`. Protocol objects have fixed
identifiers derived from their names (the validator set, the
forced-inclusion obligations, …).

## `GET /v1/agents/{id}`

`{pulse, agent, status, depth, lineage, charter, overlays, bucket}` for an
[agent identity](../concepts/agent-identities.md) or any charter
(specification v1.14). `charter` is the charter with a state proof, as
`GET /v1/objects/{id}` gives it. `overlays` has the overlay of the agent and
of each of its ancestors, from the agent up to the root, each with its
state proof: an overlay that does not exist (nothing ever happened to that
agent) comes with a proof of its absence. A client that checks every proof
against the state root of a certified header for `pulse` knows the agent's
standing without trusting the node.

`status` is the first that applies of `revoked`, `suspended`, `stopped by
an ancestor`, `expired`, `active, narrowed` and `active`, as of the next
Pulse, in which a payment would execute. `bucket` is the agent's
allotment: `remaining` (its budget), `can_spend_next_pulse` (what its
bucket lets it pay in the next Pulse), `burst` and `rate_per_pulse` (what
it keeps after its children's shares) and `spawned` (the lineage indices
it has used). Amounts are in micro-USD.

For a keyless member of a cohort, at depth 1 under an orchestrator whose
overlay exists (it was narrowed, then lifted), the answer cut down with
`jq`:

```text
$ curl -s $SION_NODE/v1/agents/01fc0eac…5aa2 | jq '{pulse, status, depth, bucket, overlays: [.overlays[] | {agent, present: (.overlay.object != null)}]}'
{
  "pulse": 98,
  "status": "active",
  "depth": 1,
  "bucket": {
    "allotment": "3b828b4f52ec474c3ce31d94d35c0c49970f1d0c82094665cd17934ca4026d46",
    "burst": 1000,
    "can_spend_next_pulse": 6,
    "rate_per_pulse": 1,
    "remaining": 49000,
    "spawned": 0
  },
  "overlays": [
    {
      "agent": "01fc0eac8f1b1b0ca007fa20a0037f814f3d6dcf9224d6aa00d78bee51a35aa2",
      "present": false
    },
    {
      "agent": "0df519abdf18a35258c4047d5a6cbc36f60edf0b137b8c96236e60dbe0ac2b98",
      "present": true
    }
  ]
}
```

## `GET /v1/claims/{id}`

`{claim, atoms, status, coc_standard_policy, proof, pulse}`: the stored
claim (body, issuer, issue Pulse, position in the log), every atom it rests
on with its bond, its status, its cost of corruption under the standard
policy, and its inclusion proof in the knowledge log. `sion claim show`
prints it; an example is in [Issuing and assessing claims](../guides/claims.md#look-at-it).
A falsified claim's status reads like
`"Falsified { at: Pulse(31), causes: [AtomId(94d5e302)] }"`.

## `POST /v1/claims/{id}/decide`

Body: `{"value": <micro-units>, "policy": <policy>}`; `policy` is optional
and defaults to the standard policy (trust the `Cryptographic` and
`Specification` classes, κ = 2).

```text
$ curl -s -X POST -H 'Content-Type: application/json' http://127.0.0.1:8730/v1/claims/e825c122db0c61842f067b444583c2b3593a75e1c5f551f4c0e3162079239412/decide -d '{"value": 10000000, "policy": {"trusted_atoms": [], "trusted_classes": ["Cryptographic", "Specification"], "distrusted_atoms": [], "assumed_broken": [], "kappa": {"num": 2, "den": 1}}}'
{"accept":true,"adjudicator_cap":"Infinite","coc":{"exact":true,"high":"Infinite","low":"Infinite"},"detection_adjusted":"Infinite","effective":"Infinite","reasons":["cost of corruption ∞ covers κ·V = 20"],"required":20000000,"reservable":"Infinite"}
```

With an empty `trusted_classes`, the same claim, which rests only on its
issuer's signatures, is worth nothing:

```text
{"accept":false,"adjudicator_cap":"Infinite","coc":{"exact":true,"high":{"Finite":0},"low":{"Finite":0}},"detection_adjusted":{"Finite":0},"effective":{"Finite":0},"reasons":["cost of corruption 0 (after detection discount 1000/1000) is below κ·V = 20","only 0 of backing is unreserved; κ·V = 20 is needed"],"required":20000000,"reservable":{"Finite":0}}
```

A policy is `{trusted_atoms, trusted_classes, distrusted_atoms,
assumed_broken, kappa: {num, den}}`. The fields of the answer are explained
in [Issuing and assessing claims](../guides/claims.md#price-it-before-relying-on-it).

## `POST /v1/transactions`

Body: `{"tx": "<hex of the Borsh-encoded SignedTransaction>"}`.

```text
$ curl -s -X POST -H 'Content-Type: application/json' http://127.0.0.1:8730/v1/transactions -d '{"tx": "a2229501a66be2aa4963…"}'
{"digest":"7f62fb33e2139d059996c96ba27b7e357ab437b54a9926ff6dd6101306fddc4f","route":"lane","shard":0}
```

The answer comes at once: the digest, the route (`lane`, or `consensus` for
a contended kind), and the sender's shard. Track the transaction with
`GET /v1/transactions/{digest}`. Errors:

```text
{"error":"bad JSON body: invalid type: integer `5`, expected a string at line 1 column 8"}
{"error":"tx is not a Borsh SignedTransaction"}
```

A transaction over 256 KiB is refused with `transaction too large`. A
batch member (spec v1.13) can be submitted here like any transaction, with
the batch's signatures, its position and its path; a whole batch is
cheaper through `POST /v1/batches`.

## `POST /v1/batches`

Body: `{"batch": "<hex of the Borsh-encoded BatchEnvelope>"}`: the batch
(`TxBatch { signer, size, root }`), its one authorization, and every
member's body with, for a keyless sender, its charter, in the batch's
order (`sion_exec::tx::BatchEnvelope`). The node checks that the members
are the whole batch (as many as its size, their leaves giving its root),
builds each member's inclusion path, and hands the members to its gateway
together, which asks each shard's committee for theirs in one message. The
answer has the batch's root and a receipt per member, as `POST
/v1/transactions` gives one:

```text
$ sion agent pay-batch orchestrator carol 1 --agents "$W1,$W2,$W3" --print > batch.hex
$ curl -s -X POST -H 'Content-Type: application/json' http://127.0.0.1:8793/v1/batches -d "{\"batch\": \"$(cat batch.hex)\"}"
{"batch":"554e1cd7c8a30a339e7ae0633991d701364bb8fff8ccad0b0d657047ad0a1cb9","members":[{"digest":"fca975f57370c8e7f660cbcd546744378272d90b2f74d1c3fabd3f625e556181","route":"lane","shard":0},{"digest":"669e636ef5d24f7545c6a931d70b45b2b526d3d2465b867a4ab5c3e6fcd2053a","route":"lane","shard":1},{"digest":"30ecd86632a443215659305ffdf3d18065c9381bd88995e95e83bb9c82bf2e34","route":"lane","shard":1}]}
```

Here three keyless subagents' payments, from a devnet on port 8793, went
to two shards. The members' signatures are not checked here: the lane
committees check them, once for the batch. Errors: a body that is not an
envelope, a size that is not the number of members (here the size field
edited to 4), and members whose leaves do not give the root (one bit of
the root flipped):

```text
{"error":"batch is not a Borsh BatchEnvelope"}
{"error":"the batch says it holds 4, the envelope has 3"}
{"error":"the members do not give the batch's root"}
```

## `GET /v1/transactions/{digest}`

| `state` | Other fields | Meaning |
|---|---|---|
| `certifying` | `shard`, `approvals` | Collecting lane approvals |
| `sequencing` | none | Certified (or submitted, if contended), waiting for a Pulse |
| `rejected` | `reason` | Certification failed |
| `executed` | `record` | Executed: `{effects, status, pulse, index, receipt: {effects, index, count, path}}` |

```text
$ curl -s http://127.0.0.1:8730/v1/transactions/0b4b87c8ed9db1d35d2548e9dfa2d7c529c680a2cd264d9cfcd683f85b897d39 | jq -c '{state, record: {status: .record.status, pulse: .record.pulse, index: .record.index, receipt: {index: .record.receipt.index, count: .record.receipt.count, path: .record.receipt.path}, events: .record.effects.events, fee: .record.effects.fee}}'
{"state":"executed","record":{"status":"Success","pulse":5,"index":0,"receipt":{"index":0,"count":1,"path":[]},"events":[{"Transferred":{"asset":"1436ac5e7c42d6cc93304083c0c6c8c964e939aa391c2e0a7304810b7e368f3c","outputs":[["98252ec61e5fe25d4e2cf6560119730fda49b549ae2b64e1acc04ce3e3633147",12500000]]}}],"fee":1200}}
```

`effects` also lists the objects created, mutated and consumed, and the
deferred items. `receipt.path` is the Merkle path from the effects leaf to
the Pulse's receipts root (empty for a Pulse with one transaction).

```text
$ curl -s http://127.0.0.1:8730/v1/transactions/0000000000000000000000000000000000000000000000000000000000000009
{"error":"unknown transaction"}
```

The node keeps records for the last 256 Pulses. For older transactions the
gateway's last status, usually `sequencing`, is all that is left.

## `POST /v1/faucet`

Devnet only. Body: `{"account": "<hex>", "amount": <micro-units>,
"asset": "<hex, optional>"}`; the default asset is USD.

```text
$ curl -s -X POST -H 'Content-Type: application/json' http://127.0.0.1:8730/v1/faucet -d '{"account": "d4a27f08fe31e101a2bce2845d3ae4c00a16405db06b569c56a34732641cb971", "amount": 2500000}'
{"queued":true}
$ curl -s -X POST -H 'Content-Type: application/json' http://127.0.0.1:8730/v1/faucet -d '{"account": "d4a27f08fe31e101a2bce2845d3ae4c00a16405db06b569c56a34732641cb971", "amount": 0}'
{"error":"amount must be between 1 micro-unit and one million units"}
```

Grants are queued (at most 10,000) and minted in batches.

## `GET /v1/pulses/latest`, `GET /v1/pulses/{n}`

`{header, certificate, counts, data_bytes, execution_ms, anchors,
revealed, new_claims, deferred_events, transactions}`. `latest` is the last
executed Pulse; it may not be certified yet.

```text
$ curl -s http://127.0.0.1:8730/v1/pulses/223 | jq -c 'del(.certificate, .transactions)'
{"anchors":2,"counts":[0,0,0],"data_bytes":48,"deferred_events":[],"execution_ms":115,"header":{"after":{"config":"2d86efe308c60641d10f022ce94b57596644f923d10c7fbcf7aa3dc029e45592","knowledge_root":"99e059bfe40288f5b83abfef816535bec4166266d30f638f11438847cb5860b6","knowledge_size":8,"pulse":223,"state_root":"fd80b0d5aa73b4c1820cc6146ff605bf1cf41a473e5ee3ce17f886189d6f275f"},"beacon":"0b91712df24f0487bd65a570d2d12e7b5c4c8670527b4cf394e8c4807714fe5f","before":{"config":"2d86efe308c60641d10f022ce94b57596644f923d10c7fbcf7aa3dc029e45592","knowledge_root":"99e059bfe40288f5b83abfef816535bec4166266d30f638f11438847cb5860b6","knowledge_size":8,"pulse":222,"state_root":"fd80b0d5aa73b4c1820cc6146ff605bf1cf41a473e5ee3ce17f886189d6f275f"},"chain":"6345655cfd2a714ed1dab69c0660f2f48f429ce2981e25629d0467d7b7ee97b7","data_k":4,"data_root":"5f36558759fd1f80adcc899ea7acfd3508e0bd2c83bbafbf38930c06aab72bf2","epoch":0,"forced":{"overdue":0,"overdue_root":"0000000000000000000000000000000000000000000000000000000000000000","pending":0},"order_digest":"9b89dcc0235fd2afd59fb2a81fc22ed9c3cf9fd765bdf7bc50d9a8f935ed5a0d","pulse":223,"receipts_root":"b3950ae788d193123558660817bb152c629460ad232a154ae961493db2ce2542"},"new_claims":[],"revealed":1}
```

| Field | Meaning |
|---|---|
| `header` | The Pulse header ([Pulses](../concepts/pulses.md#the-pulse-header)) |
| `certificate` | `{header, signatures: [[validator, bytes], …]}` once certified, else `null`. Signatures are arrays of bytes, so this field is large |
| `counts` | Transactions that succeeded, failed, were rejected |
| `data_bytes` | Size of the published data |
| `execution_ms` | How long execution took on the answering validator |
| `anchors`, `revealed` | Anchors committed in the Pulse; beacon reveals included |
| `new_claims`, `deferred_events` | Claims that joined the log; events of the deferred phase |
| `transactions` | The executed transactions with their effects |

An unknown or pruned Pulse: `{"error":"unknown or pruned Pulse"}`.

## `GET /v1/pulses/{n}/data`

The data-availability header: `{data_root, k, row_roots, col_roots}`, with
`2k` row and column roots of the extended square:

```text
$ curl -s http://127.0.0.1:8730/v1/pulses/223/data | jq -c '{data_root, k, rows: (.row_roots | length), cols: (.col_roots | length)}'
{"data_root":"5f36558759fd1f80adcc899ea7acfd3508e0bd2c83bbafbf38930c06aab72bf2","k":4,"rows":8,"cols":8}
```

## `GET /v1/pulses/{n}/data/{row}/{column}`

One share of the extended square (a Goldilocks field element) with its
Merkle proof against the row root:

```text
$ curl -s http://127.0.0.1:8730/v1/pulses/223/data/0/1
{"proof":["5230ce22dc83884424387ed1f7edda3ce215865a881654475f52aae873736103","d7f6e986a245132f878912c5b259a773546e4fa0dd1c205776ffe9f15cc9861f","1085d0690ea24f763b99f5984f88265dd8eba23a45f21d1d21e1fbc426cc8fe1"],"value":62768919806476288}
```

Shares are served for the last 64 Pulses.

## `GET /v1/pulses/{n}/world`

The World Proof's summary, with the proof itself as hex in `proof`:

```text
$ curl -s http://127.0.0.1:8730/v1/pulses/522/world | jq -c 'del(.proof)'
{"bytes":108046,"certificates":1,"claim":"f889f014054b69cf3ac04f79dc9c0a361c9b2d3ffd8c5dd39b58c307b7a67c4e","data_root":"7c73b1f6eda69069675d4c4caa378976681f099df75f111dbcd6cda88c89a529","header":"139198369965c3973bc8d52623f3396bfa5f6969f1452796bee167790e54c0fd","leaves":2,"proving_ms":15,"pulse":522,"simulated":true,"stacks":["A","B"],"statements_root":"e65e171946953e110a25724e018cb353771f9687a4b982d940ed720c2ff4a58d","transactions":1}
```

`proof` is the Borsh encoding of `sion_prover::WorldProof`
([The World Proof format](world-proof.md)). A proof that failed is
published as `{pulse, error}`. The prover keeps the last 32; otherwise:
`{"error":"no World Proof for that Pulse (yet)"}`.

## `GET /v1/events`

Query parameters: `from` (a Pulse, default 0) and `limit` (default 200, at
most 5,000). Events in order, each with its Pulse and, for transaction
events, the transaction's digest:

```text
$ curl -s 'http://127.0.0.1:8730/v1/events?from=440&limit=3'
[{"event":{"Transferred":{"asset":"1436ac5e7c42d6cc93304083c0c6c8c964e939aa391c2e0a7304810b7e368f3c","outputs":[["a08c0fc672bb5875d68526c0e9b1a3bbe5f57652e6af18a174dbb39c0ad93670",250000]]}},"pulse":440,"tx":"eaf9a464697e9d067693978761d7df6645f4be530bdca4950afd49b9d0241a31"},{"event":{"Transferred":{"asset":"1436ac5e7c42d6cc93304083c0c6c8c964e939aa391c2e0a7304810b7e368f3c","outputs":[["d4a27f08fe31e101a2bce2845d3ae4c00a16405db06b569c56a34732641cb971",1000000]]}},"pulse":444,"tx":"f3cd1539f794842c857efb12e094e2ee81ae3763d1884cad99f1f93c04bdd0e8"},{"event":{"Minted":{"amount":2500000,"asset":"1436ac5e7c42d6cc93304083c0c6c8c964e939aa391c2e0a7304810b7e368f3c","to":"d4a27f08fe31e101a2bce2845d3ae4c00a16405db06b569c56a34732641cb971"}},"pulse":522,"tx":"0734529a0a70dd73e4a8ff9deb686aeb49ade95bad83d3fe13ab065ab2504561"}]
```

Deferred-phase events have `"tx": null`. Every event kind is listed in
[Events](events.md).

## `POST /v1/forced`

Body: `{"tx": "<hex of the Borsh-encoded SignedTransaction>"}`. The node
hands the intent to every validator, bypassing the gateway:

```text
{"digest":"30c4f19181d0a2ddaee54f3ee102f3547f66c263bac9193ca0d2015d159716d3","route":"forced","shard":0}
```

See [Forced inclusion](../guides/forced-inclusion.md).

## `GET /v1/forced`

`{pulse, window, summary, pending, rotations}`: the obligations after the
latest Pulse, the summary its header commits to, and this epoch's shard
rotations. Each pending entry is `{digest, shard, posted_at, deadline,
missed}`. An example is in [Forced inclusion](../guides/forced-inclusion.md#censoring-validators).

## `GET /v1/deals`

`{pulse, deals}`: every storage deal, each `{id, deal}` with the deal's
terms and state (`Open`, `Active { provider, custody, start, bond, paid }`
or `Closed { at, proven, missed, refund, forfeited }`), amounts in
micro-units. With `?open=1`, only deals still open for acceptance (state
`Open`, `accept_by` after the latest Pulse): what providers poll. The node
answers from an index of deal ids it keeps, not a scan of the state. A provider's custody records are in its account view
(`GET /v1/accounts/{id}`, field `custody`). See
[Storage deals](../concepts/storage.md).

## `POST /v1/devnet/censor`

**Adversary simulation**, devnet only. Body: `{"accounts": ["<hex>", …],
"gateway": <bool>, "validators": <n>}`. The gateway (if `gateway`) and the
first `n` validators drop the accounts' transactions. An empty list stops
censorship.

```text
$ curl -s -X POST -H 'Content-Type: application/json' http://127.0.0.1:8730/v1/devnet/censor -d '{"accounts": ["dab40d27bb62fa8dc40c1bf8194f6d04bff293ac65dd08ace075a8d6d5cec5be"], "gateway": true, "validators": 0}'
{"censoring":1,"gateway":true,"validators":0}
$ curl -s -X POST -H 'Content-Type: application/json' http://127.0.0.1:8730/v1/devnet/censor -d '{"accounts": []}'
{"censoring":0,"gateway":false,"validators":0}
```
