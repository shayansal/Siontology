# Run a devnet

A devnet runs every validator of a committee in one process, each on its
own thread with its own copy of the state, exchanging real signed messages
over in-memory channels. Everything else is the real protocol: lane
certificates, consensus, Pulse execution, header certificates, data
availability and the World Proof.

Start one in a terminal:

```bash
sion devnet
```

```text
Siontology devnet: 4 validators, 2 shards, API on http://127.0.0.1:8730
chain identity (pin it: sion verify --chain …): 6345655cfd2a714ed1dab69c0660f2f48f429ce2981e25629d0467d7b7ee97b7
accounts funded at genesis: alice, bob, carol
Pulse 1  certified 1  round 8  executed [0,0,0]  claims 0  objects 22  proven 1
Pulse 2  certified 2  round 15  executed [1,0,0]  claims 0  objects 23  proven 2
Pulse 4  certified 3  round 22  executed [1,0,0]  claims 0  objects 23  proven 4
```

- The **chain identity** is the value a light client pins. Write it down;
  [Verify as a light client](light-client.md) uses it.
- `alice`, `bob` and `carol` start with 100,000 USD each, in four coins, so
  each can send several transactions in one Pulse. A `faucet` account holds
  the devnet's supply.
- Every five seconds the devnet prints the latest executed Pulse, the
  latest certified Pulse, the consensus round, the transactions executed
  (successful, failed, rejected) since the process started, claims in the
  knowledge log, objects in the state, and the latest proven Pulse.

How often Pulses close depends on the machine. On the 2-core machine used
for this book, shared with other work, an idle devnet closed about 0.3
Pulses a second (17 Pulses in its first minute).

The devnet runs until you press Ctrl-C (or for `--duration` seconds).
Without `--data-dir` it keeps
everything in memory and starts from genesis every time; see
[Running a devnet](../guides/devnet.md) for persistence, restarts and every
flag.

## Talk to it

In another terminal:

```bash
sion status
```

```json
{
  "active": true,
  "active_validators": 4,
  "anchors": 2,
  "beacon": "30de983de39a060dc990e95512f3f51adc2c5570006ed775c9dc81fab667921b",
  "certified_pulse": 0,
  "chain": "6345655cfd2a714ed1dab69c0660f2f48f429ce2981e25629d0467d7b7ee97b7",
  "chain_id": "sion-devnet-1",
  "claims": 0,
  "commitment": {
    "config": "2d86efe308c60641d10f022ce94b57596644f923d10c7fbcf7aa3dc029e45592",
    "knowledge_root": "b3950ae788d193123558660817bb152c629460ad232a154ae961493db2ce2542",
    "knowledge_size": 0,
    "pulse": 0,
    "state_root": "a7f345b0d0b0b6cb03d58a264886d44a4501392686d38e0346d3632292e414cd"
  },
  "epoch": 0,
  "epoch_end": 9223372036854775807,
  "executed": [
    0,
    0,
    0
  ],
  "forced_overdue": 0,
  "forced_pending": 0,
  "genesis": "30de983de39a060dc990e95512f3f51adc2c5570006ed775c9dc81fab667921b",
  "insecure_signatures": false,
  "objects": 22,
  "proven_pulse": null,
  "pulse": 0,
  "round": 6,
  "session": 0,
  "shards": 2,
  "validator": "cdc0cc1da885c87ebdb67a8c05e027449f8cc15e4743959f8c0f3fa469b3ab38",
  "validators": 4
}
```

This was taken right after start: no Pulse has executed yet, so
`certified_pulse` is 0 and `proven_pulse` is `null`. The default devnet
runs one epoch that never ends, hence the large `epoch_end`. Every field is
described in the [HTTP API reference](../reference/http-api.md#get-v1status).
Note the two names: `chain_id` is the chain's configured name, and `chain`
is the chain identity.

The CLI talks to `http://127.0.0.1:8730` unless you pass `--node` or set
`SION_NODE`. The node serves plain JSON over HTTP, so `curl` works too:

```bash
curl -s http://127.0.0.1:8730/v1/status
```

## Next

[Your first transfer](first-transfer.md).
