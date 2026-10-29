# The validator thread

A devnet (`sion_node::Devnet`) runs every validator on its own thread with
its own copy of the state, connected to the others by in-memory channels.
Genesis validators and standby validators run the same code. This chapter
describes one validator thread (`sion_node::validator`).

## What a validator holds

| Part | Type | Purpose |
|---|---|---|
| State replica | `sion_exec::State` | Objects, the knowledge log, the validator set, obligations |
| Committees | `sion_node::committees::Committees` | Derived from the state after every Pulse |
| Lane validators | one `sion_consensus::LaneValidator` per shard whose committee it sits on | Locks, approvals, batches |
| Consensus core | `sion_consensus::Core` | The certified DAG of the current session |
| Pulse builder | `sion_node::pulses::PulseBuilder` | Cuts the commit sequence into Pulses; collects beacon reveals |
| Beacon chain | `sion_consensus::HashChain` | Its private hash chain for reveals |
| Write-ahead log, lane log | `sion_node::storage` | Durability (with `--data-dir`) |
| History | the last 256 Pulses | Outputs, receipts and summaries for queries; published data for the last 64; extended squares for the last 4 |
| Epoch boundaries | one proof per epoch | For light clients (`GET /v1/epochs/{e}`) |

Each thread has a 64 MiB stack: decoding and checking nested structures
recurses, protocol limits keep the depth small, and a generous stack is a
second line of defence.

## What it receives

| Message | From | What the validator does |
|---|---|---|
| `Consensus` | other validators | Feeds the consensus core |
| `Submit` | the gateway, the API (forced intents) | Proposes the payload item in its next header |
| `Lane` | the gateway | Checks and locks a transaction's inputs for one shard |
| `LaneBatch` | the gateway | The lane requests of a batch's members in one shard: each checked and locked as a `Lane` request, the batch's signatures checked once and remembered |
| `PulseSignature` | other validators | Collects signatures on a Pulse header until it is certified |
| `ForcedApproval` | other validators | Collects approvals of a forced intent, so a certificate forms without the gateway |
| `Adversary` | `POST /v1/devnet/censor` | Adversary simulation: censors accounts |
| `Query` | the API, the gateway | Answers status, objects, receipts, proofs |
| `Shutdown` | the devnet | Writes a final snapshot and stops |

Timers drive the rest: consensus timeouts and resends, and the lane batch
timer, which every 25 ms makes the lane log durable and then signs and
sends the approvals of the batch.

## After every Pulse

When the Pulse builder closes a Pulse, the validator:

1. checks every lane certificate in it, executes it, and appends it to the
   write-ahead log ([A transaction, from wallet to receipt](transaction-flow.md));
2. builds and signs the header, if the data has an availability
   commitment, and broadcasts the signature;
3. releases the lane locks of transactions that executed or expired, and
   retries lane requests it parked while it was behind;
4. derives the committees from the new state. A shard whose lane committee
   changed gets a new lane validator; the validator keeps its own locks if
   it stays on the committee. At an epoch boundary it starts a new
   consensus session and records the boundary for light clients;
5. approves the forced intents its shards owe;
6. reveals its beacon value for the next Pulse.

Validator 0 also traces execution for the prover and tells the gateway
which transactions executed.

## Sessions

At an epoch boundary (and at every restart), the validator replaces its
consensus core and Pulse builder:

- the new core has the new active set as its committee and the new session
  number as its epoch, so messages of the old session are ignored, and
  messages of a future session wait;
- the old builder closes no Pulse after the epoch's last Pulse; what the old
  session committed after it, and what was still in the old core's
  mempool, is proposed again in the new session (beacon reveals excepted);
- the new builder carries over the beacon and the reveals already committed
  for later Pulses. After a restart they come from the log: each record and
  snapshot holds them as of its Pulse's close (`pulses::Carry`);
- beacon participants follow the active set.

A validator outside the active set runs the core as an **observer**: it
receives every message, commits what the committee commits, and executes
every Pulse, but does not propose or vote, and its header signatures do not
count.

## Queries

The HTTP API asks validator 0. A query is a message to its thread, answered
between Pulses, so an answer always reflects one consistent state. A slow
query delays that validator, which is one reason an account query that scans
every object is listed as an open issue in the implementation status.

## Evidence

When the consensus core detects equivocation (two headers for one round from
one author), the validator logs `validator <i> detected Byzantine behavior
by [<validators>]`. It does not submit a `ReportEquivocation` transaction
itself.
