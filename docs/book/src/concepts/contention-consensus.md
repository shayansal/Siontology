# Contention Consensus

Some objects have many writers: a market batch, the assurance registry, a
charter that its principal may revoke. Updates to them conflict, so they
need a total order. Contention Consensus provides it, and only for them.

## In the architecture

- **Contention domains.** Each contended object belongs to a domain: one
  market, one registry, one charter.
- **One DAG, many commits.** Validators run a DAG-based BFT protocol of the
  Mysticeti class, in which every validator proposes in every round. A
  domain's batch executes as soon as its own part of the DAG commits.
- **Atomic cross-domain operations.** An operation touching several domains
  is included in one round and executes when every domain it touches
  commits.
- **Fair order.** Market domains clear by
  [Verifiable Clearing](verifiable-clearing.md), at one price, so order
  inside a batch carries no value. Other domains order each batch by a hash
  salted with a beacon value revealed only after the batch seals.

## In this implementation

The consensus core (`sion_consensus::core`) is a **certified-DAG BFT
protocol in the Bullshark family**, partially synchronous:

1. **The DAG.** Every round, every validator proposes a header that
   references certificates from more than two thirds of the previous
   round's stake. Because every validator proposes every round, one honest
   validator is enough to get an item included.
2. **The commit rule.** Even rounds have a leader, the anchor. An anchor
   commits once round *r* + 1 certificates holding more than one third of
   the stake reference it. Committing an anchor orders its not yet
   delivered causal history deterministically, by round and then author.
   Payload items are deduplicated, because a client may hand the same item
   to several validators.
3. **Liveness.** In an even round a validator waits for the anchor or a
   timeout; in an odd round it waits for enough votes for the anchor, or a
   timeout. Headers and certificates are re-broadcast, and missing parents
   are fetched on demand.

Equivocation (two headers for one round from one validator) is detected and
produces signed evidence. The node logs it but does not report it; anyone
holding the two signed headers can submit them in a `ReportEquivocation`
transaction, which slashes the author's whole stake
([Slashing for equivocation](validators/equivocation-slashing.md)).

## Sessions

Consensus runs one DAG per **session**, with the active set as its
committee, each validator weighted by its stake. A new session starts at
every epoch boundary and at every restart of the devnet; round numbers
start again, and every message names its session, so nothing from one
session counts in another. What the old session committed after the
epoch's last Pulse, and what it never committed, is proposed again in the
new one. See [Epochs and committees](validators/epochs.md).

What consensus orders in a Pulse:

- groups of lane-certified transactions (owned-path and contended alike);
- forced intents posted straight to consensus
  (see [Censorship resistance](censorship-resistance.md));
- validators' beacon reveals.

Contended transactions execute in the Pulse's second phase, in commit
order (see [Pulses](pulses.md)).

## Parameters

The devnet uses these consensus parameters (`CoreConfig`):

| Parameter | Devnet value | Meaning |
|---|---|---|
| `leader_timeout_ms` | 400 | How long to wait for an anchor or its votes before advancing |
| `resend_interval_ms` | 600 | How often to re-broadcast and re-request missing parents |
| `gc_depth` | 50 | Rounds of history kept below the last committed anchor |
| `max_items_per_header` | 2,000 | Payload items per header |
| `max_payload_bytes` | 512 KiB | Payload bytes per header; honest validators vote for no larger header |
| `max_rounds_ahead` | 64 | Headers for rounds further ahead than this are ignored |
| `max_pending_headers` | 4,096 | Headers waiting for parents |
| `repropose_after_rounds` | 8 | Re-propose an item not delivered this many rounds after it was proposed |
| `leader_schedule` | `RoundRobin` | Anchors in committee order |

## Testing

`sion-consensus` includes a deterministic network simulator with Byzantine
behaviours (crashes, equivocating proposers, silent voters, invalid
signatures, colluders beyond the fault threshold) and an adversarial phase
before the global stabilization time, in which messages are dropped. Its
tests check safety (no two honest validators commit different sequences)
and liveness. To print a few scenarios:

```bash
cargo run --release -p sion-consensus --example simulate
```

## Differences from the architecture

| Architecture | This implementation |
|---|---|
| Uncertified DAG (Mysticeti class), fewer message delays | Certified DAG (Bullshark class) |
| A domain's batch executes when its part of the DAG commits | Everything committed in a Pulse executes when the Pulse closes |
| Non-market domains ordered by a beacon-salted hash after the batch seals | Contended transactions execute in commit order |
| Committees sampled by stake, rotated, with value in flight capped at slashable stake | The consensus committee is the whole active set (at most 100 validators), weighted by stake, and changes at epoch boundaries; no value-in-flight cap |
