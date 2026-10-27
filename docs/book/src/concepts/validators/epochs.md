# Epochs and committees

An epoch is a run of Pulses with one validator set. Committees change only
at epoch boundaries, and when a shard misses a forced-inclusion deadline.
Both changes happen inside the state transition, so a light client can
follow them from the genesis it pinned.

## The boundary

At genesis the validator set is in epoch 0, which starts at Pulse 1 and
ends at `epoch_end = epoch_start + epoch_length − 1`. The genesis state
schedules an `EpochEnd` event at that Pulse. In the deferred phase of the
epoch's last Pulse (specification §6.3), after the Pulse's fees have
reached the treasury:

1. **Rewards.** A share of the fees collected during the epoch is paid to
   the epoch's validators by weight. See [Rewards](rewards.md).
2. **Joins and exits.** Every Joining record becomes Active; every Leaving
   record becomes Exited, withdrawable `unbonding` Pulses later.
3. **The next active set.** The Active records with the most stake, at
   most `max_active`. Slashed validators are not Active, so they lose their
   seats here.
4. **A new epoch.** The epoch number goes up by one; the new epoch starts
   at the next Pulse; the lane-sampling seed becomes this Pulse's beacon;
   shard rotations are cleared; the fee mark moves to the current fee
   total.
5. **The next boundary** is scheduled, and the event `EpochChanged { epoch,
   active, rewards }` is emitted.

The devnet's `--epoch-length 0` (the default) sets the length to 2⁶³ − 1
Pulses, so the boundary never comes and the set never changes.

## Committees after a boundary

After every Pulse, each validator derives the committees from its state:

| Committee | Members | Weights | Certifies |
|---|---|---|---|
| Consensus | The active set | Stake in whole units | Consensus headers and votes; Pulse headers |
| Lane committee of shard *s* | `seats` validators sampled from the active set with the epoch's seed, or the shard's rotated committee | One seat each | Locks on owned inputs (lane certificates) |

Lane approvals and certificates name a **lane epoch** that combines the
validator-set epoch and the shard's rotation count. An approval made under
one lane committee never counts for the next.

Sampling uses the beacon value of the previous epoch's last Pulse, so
nobody knows the next epoch's lane committees before that Pulse's beacon
is revealed.
The beacon is a hash-chain commit-reveal among the active validators, and
the last validator to reveal can bias it by withholding
(`impl:hash-chain-beacon`).

## Consensus sessions

Consensus runs one DAG per **session**. A new session starts at every
epoch boundary, with the new active set as its committee, and at every
restart of the devnet. Round numbers start again in each session, and
messages name their session, so a vote from one session never counts in
another.

Nothing is lost at the hand-over. What the old session committed after the
epoch's last Pulse, and what it never committed, is proposed again in the
new one. The gateway follows the new committees too: certificates from a
replaced lane committee no longer count, so it collects approvals again
from the new one.

Standby validators run as observers: they execute every Pulse and keep the
state, but propose and vote only once their stake makes them active. A
validator that leaves the active set goes back to observing.

This hand-over assumes that a validator still finishing the old session
receives every message already sent. The in-process network always
delivers them; a real network needs a drain period
(`impl:reliable-handover`).

## How a light client follows

A light client pins one value, the [chain identity](../../reference/chain-identity.md).
It fixes the genesis validators, their keys and stake, and so the
committee of epoch 0. Every later committee is in the state, and the client
follows it one boundary at a time (`sion_light::EpochChain`):

1. It fetches the proof for epoch *e* from `GET /v1/epochs/{e}`: the
   certificate of epoch *e − 1*'s last Pulse, and the validator set after
   that Pulse with a state proof against the certified header.
2. It checks the certificate against epoch *e − 1*'s committee, which it
   already trusts, and the state proof against the header.
3. It checks that the set is in epoch *e* and that the epoch starts right
   after the certified Pulse. From the set it derives epoch *e*'s committee
   and keys.

The links never need checking again, and they are short: one certificate
and one state proof per epoch. Validators keep them in memory and, with a
data directory, on disk
(`<data-dir>/validator-<i>/epochs/epoch-<e>.bin`), so they survive
restarts.

World Proofs carry the validator set the Pulse began with, proven against
the Pulse's starting state root. Lane certificates are checked against the
lane committees derived from that set, not against anything a node
serves.

## On the devnet

The staking demo prints the change as the light client sees it
(`sion demo staking --local`):

```text
Pulse  24  epoch 1 begins: 5 validators hold seats, the newcomer among them; 0.00224 USD of fees paid as rewards, split by stake
           every validator derived the same committees from the state: consensus (weighted by stake) and 2 lane committees sampled with the beacon
           light client: followed the committee from genesis through 1 boundary proof(s); Pulse 25's certificate has 4 of 5 validators, 350000 of 450000 stake (quorum 300001) ✓
```

The four genesis validators weigh 100,000 each and the newcomer 50,000, so
the total is 450,000 and a quorum is ⌊2·450,000/3⌋ + 1 = 300,001.

On a devnet started with `sion devnet --standby 1 --epoch-length 24`,
`sion verify` reports the boundaries it crossed:

```text
$ sion verify --chain e2fdac57652ce347915b7fc92c08f327ac785407b2bd956ddff374fd5f335416
chain: e2fdac57 matches the pinned identity; 4 validators, keys derive their identifiers ✓
Pulse 34 header 69502475 (state root df555d15, knowledge root b3950ae7, 0 claims)
committees: followed from the pinned epoch 0 to epoch 1 through 1 certified boundaries and validator-set proofs ✓
certificate: 350000 of 450000 stake signed (quorum 300001), each signature ML-DSA-44 + SLH-DSA-SHA2-128f ✓
```

The rest of that report is in [Light-client verification](../../guides/light-client.md).
The validator log shows each boundary:

```text
validator: Pulse 24: epoch 1 begins with 5 validators (session 1)
validator: Pulse 48: epoch 2 begins with 4 validators (session 2)
```
