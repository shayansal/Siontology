# Storage

With `--data-dir`, every validator keeps a write-ahead log of executed
Pulses, periodic snapshots, a lane log and its epoch-boundary proofs
(`sion_node::storage`). Without it, a devnet keeps everything in memory and
starts from genesis every time. The [devnet guide](../guides/devnet.md#persistence)
shows a real directory.

## Layout

```text
<data-dir>/
  genesis                 the genesis digest; one chain per directory
  generation              the restart generation (0, 1, 2, …)
  validator-<i>/
    wal-<sequence>.log    write-ahead log segments
    snapshot-<pulse>.bin  the two newest snapshots
    lanes.log             lane approvals not yet executed or expired
    epochs/epoch-<e>.bin  proofs that link epoch e − 1 to e
```

Starting a devnet whose genesis differs from the directory's `genesis` file
fails with `<dir> holds another chain (genesis …); use a fresh directory`.

## The write-ahead log

Each executed Pulse is one record: the Pulse's input (the certified owned
and contended transactions, the forced intents, the Pulse number and the
beacon), the resulting world commitment, and what the Pulse builder carried
past the Pulse's close: the beacon state and the beacon reveals consensus
already committed for later Pulses. Validators may reveal up to 8 Pulses
ahead, so those reveals decide later beacons; a validator restarting from
its log without them would compute a different beacon from one that never
stopped. Records are framed:

| Bytes | Content |
|---|---|
| 8 | Magic tag `SIONWAL4` |
| 8 | Sequence number, big-endian |
| 4 | Payload length, big-endian |
| 8 | Checksum: the first 8 bytes of a domain-separated SHA3-256 of the payload |
| n | Payload: the Borsh-encoded record |

Segments rotate at 8 MiB. A record is appended before the validator signs
the Pulse's header. `fsync` per record is off on the devnet (a
`DevnetConfig` field), so a power failure can lose the last records; a
crash of the process cannot, since writes reach the operating system first.

## Snapshots

Every 50 Pulses, and when a validator stops cleanly, it writes a snapshot:
the full state, the Pulse builder's carry as in a record (the beacon state
and the reveals waiting for later Pulses), the restart generation, and the
signed forced intents the state still holds obligations for. A snapshot is
written to a temporary file, synced and renamed into place, so it is either
whole or absent. The two newest are kept. Framing: magic `SIONSNP4`, an
8-byte length, an 8-byte checksum, the payload. The sparse Merkle tree is
never trusted from disk: it is rebuilt from the objects.

A directory written by an older version (magic `SIONWAL3` or `SIONSNP3`)
is refused with `<file> was written in an older storage format; start from
a fresh data directory`, rather than read as a torn tail and truncated.

## The lane log

Lane approvals are appended to `lanes.log` (magic `SIONLAN1`, length,
checksum, payload) **before** they leave the validator, every batch
interval. After a restart, each validator restores the locks of
transactions that have neither executed nor expired, and compacts the log
to them; snapshots compact it too. So no conflicting spend of a locked
input can be certified after a restart, and the original transaction can
be certified again.

## Epoch proofs

At every epoch boundary a validator stores the certificate of the epoch's
last Pulse and the validator set after it, with its state proof, as
`epochs/epoch-<e>.bin` (write and rename). They are loaded at startup, so
`GET /v1/epochs/{e}` answers for epochs that began before a restart.

## Recovery

On start, for each validator:

1. Open the log, truncating a torn record at the tail.
2. Load the newest snapshot that decodes and checks, or start from genesis.
3. Re-execute every logged Pulse after it and check that each reproduces
   the logged world commitment. A difference stops the start-up with
   `replay of Pulse <p> diverged`: corruption or a nondeterminism bug
   cannot pass silently.
4. If another validator logged more Pulses, adopt its state and its
   Pulse-builder carry (state sync) and snapshot them. Validators stop
   together, but one may have logged a Pulse more than another.
5. Resume the Pulse builder from the carry: the beacon, re-synced with the
   active set, and the reveals already committed for the Pulses ahead.
6. Restore live lane locks from the lane log.

Each start also increments `generation`, and the validators begin a new
consensus session numbered from it, so round numbers never repeat within
a session.

## What is kept only in memory

Pulse outputs, receipts and summaries for the last 256 Pulses; published
data for the last 64; World Proofs for the last 32; the gateway's
transaction statuses. After a restart, they exist only for Pulses executed
since. There is no archival tier.
