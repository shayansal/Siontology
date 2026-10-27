# Staking and epochs

This guide shows the validator set changing: a validator joins by staking,
the committee changes at an epoch boundary, rewards are paid and claimed,
and an equivocating validator is slashed and loses its seat. A light client
follows every step from the genesis it pinned. The concepts are in
[Validators, staking and epochs](../concepts/committees.md).

The default devnet runs one epoch that never ends, so its validator set
never changes. You need a devnet with a **standby validator** (one that
runs from the start without a seat) and **short epochs**.

## The quick way

`sion demo staking --local` starts its own in-process devnet with one
standby validator and 24-Pulse epochs, and runs the whole cycle. It took
67 seconds on the machine used for this book:

```text
$ sion demo staking --local
== Staking: committees come from the state, epoch by epoch ==
…
           epochs last 24 Pulses; this one ends after Pulse 24
Pulse   6  operator-4 stakes 50,000 USD for validator a6f32863, with a proof that it holds the validator's keys
           Done: validator a6f32863 registered by operator-4 with 50,000 USD at stake; fee 0.0032 USD.
Pulse  24  epoch 1 begins: 5 validators hold seats, the newcomer among them; 0.00224 USD of fees paid as rewards, split by stake
           every validator derived the same committees from the state: consensus (weighted by stake) and 2 lane committees sampled with the beacon
           light client: followed the committee from genesis through 1 boundary proof(s); Pulse 25's certificate has 4 of 5 validators, 350000 of 450000 stake (quorum 300001) ✓
Pulse  29  operator-2 collects: Done: 0.00056 USD of rewards claimed for validator 04dde303; fee 0.0012 USD.
           ADVERSARY SIMULATION: validator a6f32863 signs two different consensus headers for the same round
Pulse  31  watchdog reports both signed headers: Done: validator a6f32863 slashed: 50,000 USD forfeited, 2,500 USD of it to watchdog for the report; fee 0.005 USD.
           epoch 2 begins with 4 validators: the slashed validator lost its seat
           light client: still following, 2 boundary proof(s) from the pinned genesis ✓
Result: who certifies is part of the state transition, proven like any balance; stake buys seats, misbehaviour is evidence anyone can submit, and a client needs nothing but the genesis it pinned.
```

The line elided after the title names the standby validator and the
genesis validators' stake (100,000 USD each).

Step by step:

1. `operator-4`, the devnet wallet that operates standby validator 4,
   stakes 50,000 USD for it with a proof of key possession. The validator
   is Joining.
2. At Pulse 24, the last of epoch 0, the deferred phase pays 70 % of the
   epoch's fees to the four validators, makes the newcomer Active and picks
   the next active set: five validators weighing 450,000.
3. The light client crosses the boundary with one proof and checks a
   certificate signed under the new committee.
4. `operator-2` claims its validator's share of the rewards.
5. The demo makes the newcomer sign two headers for one round (an
   adversary simulation, possible because devnet keys are public), and an
   account called `watchdog` reports them. The stake is forfeited: 2,500
   USD (5 %) to the watchdog, the rest to the treasury.
6. At the next boundary the slashed validator is not Active, so it leaves
   the active set. The light client follows that boundary too.

## On a devnet you keep

Start a devnet with one standby validator, 24-Pulse epochs and a short
unbonding period:

```text
$ sion devnet --standby 1 --epoch-length 24 --unbonding 8
Siontology devnet: 4 validators, 2 shards, API on http://127.0.0.1:8730
chain identity (pin it: sion verify --chain …): e2fdac57652ce347915b7fc92c08f327ac785407b2bd956ddff374fd5f335416
accounts funded at genesis: alice, bob, carol
```

The epoch parameters are part of the genesis, so this chain identity
differs from a default devnet's. The banner counts the genesis validators;
`sion status` counts the validator threads, standby included, and says
which epoch it is:

```text
$ sion status
{
  "active": true,
  "active_validators": 4,
  ...
  "epoch": 0,
  "epoch_end": 24,
  ...
  "session": 0,
  "shards": 2,
  "validator": "cdc0cc1da885c87ebdb67a8c05e027449f8cc15e4743959f8c0f3fa469b3ab38",
  "validators": 5
}
```

`active` says whether the validator answering the query holds a seat.

### The validator set

`GET /v1/validators` returns the validator set and the committees derived
from it (the [HTTP API reference](../reference/http-api.md) lists every
field). The parameters, with amounts in micro-units:

```text
$ curl -s http://127.0.0.1:8730/v1/validators | jq '{epoch, epoch_start, epoch_end, session, params}'
{
  "epoch": 0,
  "epoch_start": 1,
  "epoch_end": 24,
  "session": 0,
  "params": {
    "epoch_length": 24,
    "max_active": 100,
    "min_stake": 10000000000,
    "reporter_permille": 50,
    "reward_permille": 700,
    "seats": 4,
    "stake_asset": "1436ac5e7c42d6cc93304083c0c6c8c964e939aa391c2e0a7304810b7e368f3c",
    "unbonding": 8
  }
}
$ curl -s http://127.0.0.1:8730/v1/validators | jq -c '.validators[] | {validator: .validator[0:8], stake, rewards, status, active}'
{"validator":"04dde303","stake":100000000000,"rewards":0,"status":"Active","active":true}
{"validator":"2ed647f1","stake":100000000000,"rewards":0,"status":"Active","active":true}
{"validator":"92e02694","stake":100000000000,"rewards":0,"status":"Active","active":true}
{"validator":"cdc0cc1d","stake":100000000000,"rewards":0,"status":"Active","active":true}
```

The standby validator is not in the set: it is registered only when its
operator stakes.

### Run the demo against it

```text
$ sion demo staking
== Staking: committees come from the state, epoch by epoch ==
…
           epochs last 24 Pulses; this one ends after Pulse 24
Pulse   9  operator-4 stakes 50,000 USD for validator a6f32863, with a proof that it holds the validator's keys
           Done: validator a6f32863 registered by operator-4 with 50,000 USD at stake; fee 0.0032 USD.
Pulse  24  epoch 1 begins: 5 validators hold seats, the newcomer among them; 0.00224 USD of fees paid as rewards, split by stake
           every validator derived the same committees from the state: consensus (weighted by stake) and 2 lane committees sampled with the beacon
           light client: followed the committee from genesis through 1 boundary proof(s); Pulse 25's certificate has 4 of 5 validators, 350000 of 450000 stake (quorum 300001) ✓
Pulse  30  operator-2 collects: Done: 0.00056 USD of rewards claimed for validator 04dde303; fee 0.0012 USD.
           ADVERSARY SIMULATION: validator a6f32863 signs two different consensus headers for the same round
Pulse  34  watchdog reports both signed headers: Done: validator a6f32863 slashed: 50,000 USD forfeited, 2,500 USD of it to watchdog for the report; fee 0.005 USD.
           epoch 2 begins with 4 validators: the slashed validator lost its seat
error: the epoch's boundary Pulse is not certified yet
```

In this run the last step failed: the demo asked for the proof of epoch 2
as soon as the epoch began, before the boundary Pulse's certificate had
formed. A few seconds later the proof was there (below). The demo stakes
for the first standby validator that is not registered yet, so it runs
once per standby validator.

The validator log shows each boundary:

```text
validator: Pulse 24: epoch 1 begins with 5 validators (session 1)
validator: Pulse 48: epoch 2 begins with 4 validators (session 2)
validator: Pulse 72: epoch 3 begins with 4 validators (session 3)
```

### What changed

The events, with amounts in micro-USD:

```text
$ sion events --from 1 --limit 400
Pulse     3  {"Minted":{"to":"e5a85e84a139843f9305b91529032c49a807851bac8507c81adcbe5de2e4cb4c","asset":"1436ac5e7c42d6cc93304083c0c6c8c964e939aa391c2e0a7304810b7e368f3c","amount":50010000000}}
Pulse     6  {"Minted":{"to":"d21900a62be1498b812964f0700dbccb5cbe651a00e4cce640ab415c38153e80","asset":"1436ac5e7c42d6cc93304083c0c6c8c964e939aa391c2e0a7304810b7e368f3c","amount":10000000}}
Pulse     9  {"ValidatorRegistered":{"validator":"a6f3286364638f893e56073c6fcc85cf29b68bf65eb411cb41a613b5929dab6a","operator":"e5a85e84a139843f9305b91529032c49a807851bac8507c81adcbe5de2e4cb4c","stake":50000000000}}
Pulse    24  {"EpochChanged":{"epoch":1,"active":5,"rewards":2240}}
Pulse    27  {"Minted":{"to":"944da10c2cf91dd4d67baab256f8ae4742cf25c72a1938b45d1f47e81eb1cd4a","asset":"1436ac5e7c42d6cc93304083c0c6c8c964e939aa391c2e0a7304810b7e368f3c","amount":1000000}}
Pulse    30  {"RewardsClaimed":{"validator":"04dde303fa8297520d5ab13c037c1aa125e3eb3fef5e36762e0ceb654ca729af","amount":560}}
Pulse    34  {"ValidatorSlashed":{"validator":"a6f3286364638f893e56073c6fcc85cf29b68bf65eb411cb41a613b5929dab6a","amount":50000000000,"reporter":"d21900a62be1498b812964f0700dbccb5cbe651a00e4cce640ab415c38153e80","reward":2500000000}}
…
```

The `Minted` events are the faucet funding the operator, the watchdog and
`operator-2`. At Pulse 48, the end of epoch 1, an `EpochChanged` event
began epoch 2. Its rewards are 70 % of the fees collected during epoch 1;
the 47,500 USD the slash brought the treasury is not a fee and is not paid
out ([Rewards](../concepts/validators/rewards.md)).

After epoch 2 began, the slashed validator's record:

```text
$ curl -s http://127.0.0.1:8730/v1/validators | jq -c '.validators[] | select(.validator | startswith("a6f3")) | {validator: .validator[0:8], stake, rewards, status, active}'
{"validator":"a6f32863","stake":0,"rewards":0,"status":{"Slashed":{"at":34}},"active":false}
```

The committees of epoch 2, derived from the set:

```text
$ curl -s http://127.0.0.1:8730/v1/validators | jq -c '{epoch, session, consensus: [.committee.members[] | [.[0][0:8], .[1]]], quorum_of: .committee.total, lanes: (.shards | map_values([.members[][0][0:8]]))}'
{"epoch":2,"session":2,"consensus":[["04dde303",100000],["2ed647f1",100000],["92e02694",100000],["cdc0cc1d",100000]],"quorum_of":400000,"lanes":{"0":["04dde303","2ed647f1","92e02694","cdc0cc1d"],"1":["04dde303","2ed647f1","92e02694","cdc0cc1d"]}}
```

### Epoch proofs

`GET /v1/epochs/{e}` returns what links epoch *e − 1* to epoch *e*: the
certificate of epoch *e − 1*'s last Pulse and the validator set after it,
with a state proof. The response is large (signatures are byte arrays); a
summary:

```text
$ curl -s http://127.0.0.1:8730/v1/epochs/1 | jq -c '{epoch, boundary_pulse: .boundary.header.pulse, boundary_epoch: .boundary.header.epoch, signers: [.boundary.signatures[][0][0:8]], next_epoch_start: .validators.object.data.Validators.epoch_start, active: [.validators.object.data.Validators.active[] | [.[0][0:8], .[1]]]}'
{"epoch":1,"boundary_pulse":24,"boundary_epoch":0,"signers":["04dde303","2ed647f1","cdc0cc1d"],"next_epoch_start":25,"active":[["04dde303",100000],["2ed647f1",100000],["92e02694",100000],["a6f32863",50000],["cdc0cc1d",100000]]}
```

Three of the four genesis validators signed Pulse 24, enough for epoch 0's
quorum of 266,667. Epoch 0 needs no proof: the chain identity fixes it.

```text
$ curl -s http://127.0.0.1:8730/v1/epochs/0
{"error":"no proof for that epoch (it has not begun, or began before this validator kept proofs)"}
$ curl -s http://127.0.0.1:8730/v1/epochs/x
{"error":"the epoch must be a number"}
```

### Verify across epochs

`sion verify` follows the committee from the pinned identity to the epoch
of the Pulse it checks:

```text
$ sion verify --chain e2fdac57652ce347915b7fc92c08f327ac785407b2bd956ddff374fd5f335416
chain: e2fdac57 matches the pinned identity; 4 validators, keys derive their identifiers ✓
Pulse 34 header 69502475 (state root df555d15, knowledge root b3950ae7, 0 claims)
committees: followed from the pinned epoch 0 to epoch 1 through 1 certified boundaries and validator-set proofs ✓
certificate: 350000 of 450000 stake signed (quorum 300001), each signature ML-DSA-44 + SLH-DSA-SHA2-128f ✓
forced inclusion: no overdue intents (0 pending within their deadline) ✓
receipts: 1 of 1 transactions proven against the receipts root, in the certified order ✓
state: proof skipped (node already at Pulse 48)
availability: 16 of 16 random shares of the 256×256 square verified; a withholding would go unnoticed with probability < 9.2e-3 ✓
world proof: 2 leaves re-verified by stack ["A", "B"]; the published data is what executed and encodes to the certified data root (245 KB; replay proofs simulate succinct ones) ✓
lane certificates: all 1 executed transactions locked their inputs with a quorum of their shard's committee, taken from the proven validator set ✓
```

"4 validators" on the first line are the genesis validators the identity
fixes. Pulse 34 is in epoch 1, so the client crossed one boundary.

## Leaving and withdrawing

No CLI command builds `Unstake` or `WithdrawStake` yet, and no demo
unstakes; the staking demo is the only command that builds `ClaimRewards`.
All three are transaction kinds in the Rust API (`sion_exec::TxKind`): build
and sign one there, and submit it with
[`POST /v1/transactions`](../reference/http-api.md#post-v1transactions)
like any other transaction. The
[Staking](../concepts/validators/staking.md) chapter describes their rules
and refusals.
