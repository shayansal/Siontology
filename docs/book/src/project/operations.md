# Operations

Siontology runs today as a devnet: every validator in one process on one
machine. This chapter covers running one for a team or a test
environment. It secures nothing of value; do not hold anything real on it.

## Running a devnet as a service

```bash
sion devnet --data-dir /var/lib/sion-devnet --api 127.0.0.1:8730
```

- **Keep a data directory.** With `--data-dir`, the chain survives
  restarts ([Running a devnet](../guides/devnet.md#persistence)). Without
  it, every start is a new chain from genesis.
- **Stop cleanly when you can.** Ctrl-C or SIGTERM ends the process at
  once; recovery replays the write-ahead log from the newest snapshot, which
  is safe but slower. `--duration <seconds>` stops cleanly and writes final
  snapshots.
- **One devnet per machine is plenty.** All validators run at full speed in
  one process. On the 2-core machine used for this book, one idle devnet
  closed about 0.3 Pulses a second; two at once slow each other down.
- **Pin the identity.** Publish the chain identity the devnet prints at
  startup to your users through a channel they trust, and have them set
  `SION_CHAIN`.

## Exposure

The HTTP API has no authentication. Anyone who can reach it can:

- mint coins with `POST /v1/faucet`;
- make the gateway and validators censor accounts with
  `POST /v1/devnet/censor` (an adversary simulation);
- submit transactions and forced intents.

The default listen address is `127.0.0.1:8730`, reachable only from the
same machine. If you put a devnet behind a proxy for others, restrict
those routes, and remember that devnet validator keys and the deterministic
devnet wallets (`alice`, `faucet`, `operator-0`, …) can be derived by
anyone (`impl:devnet-keys`).

## What to watch

`sion status` (or `GET /v1/status`) every few seconds:

| Signal | Healthy | Worry when |
|---|---|---|
| `pulse` | Increases | It stops: consensus has stalled |
| `pulse − certified_pulse` | 0 to 2 | It grows: header signatures are not reaching a quorum |
| `pulse − proven_pulse` | Small | It grows past 16: the prover is behind and Pulses go unproven |
| `forced_overdue` | 0 | Above 0: some shard is censoring a forced intent |
| `executed[2]` (rejected) | Low | It climbs: clients are sending stale or invalid transactions |
| `epoch`, `active_validators` | Change only at boundaries | The active set shrinks unexpectedly |

The devnet's standard error carries the events an operator should see:

| Line | Meaning |
|---|---|
| `validator: Pulse 24: epoch 1 begins with 5 validators (session 1)` | An epoch boundary |
| `validator: Pulse 176: forced intent d4a76c8d overdue in shard 1: censorship detected` | A shard missed a forced-inclusion deadline |
| `validator: Pulse 176: shard 1's committee rotated (rotation 1)` | The shard's committee was replaced |
| `validator <i> detected Byzantine behavior by …` | Equivocation seen in consensus; consider reporting it |
| `validator <i>: Pulse <p> has no data-availability commitment; not signing it` | Data encoding failed for a Pulse |
| `validator <i>: log append failed: …`, `snapshot failed: …` | Disk trouble: the chain may not survive a restart |

## Disk and memory

- The write-ahead log grows with traffic. A devnet with light traffic wrote
  1.25 MB for 228 Pulses. Segments rotate at 8 MiB and are never deleted;
  plan disk accordingly or start fresh periodically.
- Each validator keeps two snapshots (each the full state: 50 to 60 KB on
  that devnet) and a lane log compacted at every snapshot.
- In memory, each validator keeps the last 256 Pulses of outputs, the
  published data of the last 64 and extended squares of the last 4; the
  prover keeps 32 World Proofs. Large Pulses make these large.

## Backups

Stop the devnet, then copy the data directory. A copy taken while it runs
may catch a torn log record; recovery truncates it, so the copy loses at
most the last Pulse. Back up wallet seeds (`$SION_HOME/wallets/*.seed`)
separately: anyone who reads a seed controls the account.

## Upgrades

There is no migration between versions. Log and snapshot formats carry a
version tag and change when the state does; a data directory from another
version may not load, and a different genesis is refused. After upgrading,
start a fresh data directory.

## What is not supported

- Validators on separate machines: there is no networking between
  processes (`impl:in-process-network`).
- Joining an existing devnet from outside: a devnet's validators are
  threads of one process.
- Production use of any kind.
