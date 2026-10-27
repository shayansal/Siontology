# Running a devnet

`sion devnet` runs a whole network in one process and serves its HTTP API.
This guide covers its flags, persistence and restarts, and the modes meant
for experiments.

## Flags

```text
$ sion devnet --help
Run a devnet and serve its API

Usage: sion devnet [OPTIONS]

Options:
      --node <NODE>
          The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --validators <VALIDATORS>
          Genesis validators (the first active set) [default: 4]
      --standby <STANDBY>
          Standby validators: they run from the start, follow the chain without a seat, and join the active set when their operator stakes for them (`operator-<index>`) [default: 0]
      --epoch-length <EPOCH_LENGTH>
          Pulses per epoch: committees change at epoch boundaries (0: one epoch that never ends) [default: 0]
      --unbonding <UNBONDING>
          Pulses between leaving the active set and withdrawing stake [default: 20]
      --forced-window <FORCED_WINDOW>
          Pulses a shard has to execute a forced intent before its committee is replaced [default: 8]
      --shards <SHARDS>
          Lane shards [default: 2]
      --seats <SEATS>
          Seats per shard committee [default: 4]
      --rounds-per-pulse <ROUNDS_PER_PULSE>
          Consensus rounds per Pulse [default: 4]
      --data-dir <DATA_DIR>
          Keep logs and snapshots here (restarts resume from them)
      --api <API>
          Where to serve the API [default: 127.0.0.1:8730]
      --insecure-fast-signatures
          INSECURE: replace dual post-quantum validator signatures with a fast simulation MAC (load experiments only)
      --no-prove
          Do not run the prover
      --chunk <CHUNK>
          Transactions per proof chunk [default: 16]
      --accounts <ACCOUNTS>
          Named accounts funded at genesis [default: alice,bob,carol]
      --duration <DURATION>
          Stop after this many seconds (0: run until interrupted) [default: 0]
      --report <REPORT>
          Print status every this many seconds (0: silent) [default: 5]
  -h, --help
          Print help
```

| Flag | Notes |
|---|---|
| `--validators` | Genesis validators, each staking 100,000 USD, operated by the devnet wallets `operator-0`, `operator-1`, … Every validator runs on its own thread with its own state. Validator keys are derived from the chain name and the validator's index, so anyone can derive them (`impl:devnet-keys`) |
| `--standby` | Extra validators that execute every Pulse without a seat. They join the active set at an epoch boundary once their operator stakes for them; see [Staking and epochs](staking.md) |
| `--epoch-length` | With the default 0, the validator set never changes. Set it to see committees change |
| `--unbonding` | Pulses an exited validator's stake stays locked |
| `--forced-window` | Pulses a shard has to execute a forced intent; see [Forced inclusion](forced-inclusion.md) |
| `--shards` | Accounts are assigned to shards by their identifier; each shard has its own lane committee |
| `--seats` | Seats per lane committee, capped at the active set's size. With 4 validators and 4 seats every committee is every validator |
| `--rounds-per-pulse` | Consensus rounds per Pulse. More rounds per Pulse mean fewer, larger Pulses |
| `--data-dir` | Persist the chain (below). Without it nothing is written to disk |
| `--api` | Listen address of the HTTP API. `--node` is the address the CLI talks to; it is accepted by every command and unused by `devnet` |
| `--accounts` | Comma-separated names of deterministic devnet wallets funded at genesis with 100,000 USD each, as four coins of 25,000 USD. A `faucet` wallet holding 1,000,000,000 USD is always added |
| `--chunk` | Transactions per proof chunk, for the prover |
| `--no-prove` | No prover thread: no World Proofs, and `proven_pulse` stays `null` |
| `--insecure-fast-signatures` | See [Experiment modes](#experiment-modes) |
| `--duration` | Stop after this many seconds, cleanly |
| `--report` | Status line interval |

The chain identity depends on the genesis validators, the shards, the
seats, the funded accounts, the epoch length, the unbonding period, the
forced-inclusion window and `--insecure-fast-signatures`. `--standby` does
not change it: standby validators are not in the genesis. Two devnets
started with the same flags have the same identity. Other settings (minimum stake, reward shares, snapshot interval,
history) are fields of `sion_node::DevnetConfig` without a flag; see
[Configuration](../reference/configuration.md).

## Persistence

With `--data-dir`, every validator keeps its own write-ahead log, snapshots
and lane log:

```text
$ ls -la devnet-data devnet-data/validator-0
devnet-data:
-rw-r--r-- 1 root root    1 Oct 28 04:07 generation
-rw-r--r-- 1 root root   64 Oct 28 04:07 genesis
drwxr-xr-x 2 root root 4096 Oct 28 04:17 validator-0
drwxr-xr-x 2 root root 4096 Oct 28 04:17 validator-1
drwxr-xr-x 2 root root 4096 Oct 28 04:17 validator-2
drwxr-xr-x 2 root root 4096 Oct 28 04:17 validator-3

devnet-data/validator-0:
-rw-r--r-- 1 root root  178780 Oct 28 04:19 lanes.log
-rw-r--r-- 1 root root   50025 Oct 28 04:15 snapshot-0000000000000150.bin
-rw-r--r-- 1 root root   56652 Oct 28 04:17 snapshot-0000000000000200.bin
-rw-r--r-- 1 root root 1251013 Oct 28 04:19 wal-0000000000000000.log
```

| File | Contents |
|---|---|
| `genesis` | The genesis digest. A directory holds one chain; starting a different configuration on it is refused |
| `generation` | How many times this directory has been restarted (0 on the first run). Each start begins a new consensus session |
| `validator-N/wal-*.log` | Every executed Pulse: its input (transactions and forced intents), the resulting world commitment and the beacon state. Segments rotate at 8 MiB |
| `validator-N/snapshot-*.bin` | A full snapshot every 50 Pulses and at a clean shutdown; the two newest are kept |
| `validator-N/lanes.log` | Lane approvals, written before they leave the validator, so locks survive a restart |
| `validator-N/epochs/epoch-*.bin` | With epochs that end: the proof linking each epoch to the previous one, for light clients |

This log of 228 Pulses was 1.25 MB. See [Storage](../architecture/storage.md)
for the record formats and recovery.

## Stopping and restarting

Ctrl-C (SIGINT) ends the process at once; the validators do not write a
final snapshot. `--duration` stops cleanly: every validator writes a final
snapshot. Either way, a restart recovers:

1. Each validator loads its newest valid snapshot and re-executes every
   logged Pulse after it, checking that each one reproduces the logged
   world commitment. A torn record at the end of the log is truncated.
2. A validator that logged fewer Pulses than another adopts the most
   advanced state (state sync) and snapshots it.
3. Lane locks of transactions that have neither executed nor expired are
   restored, and the lane log is compacted to them.
4. `generation` goes up by one, and consensus starts a new session.

A real restart after Ctrl-C at Pulse 228:

```text
$ sion devnet --data-dir devnet-data --duration 45
Siontology devnet: 4 validators, 2 shards, API on http://127.0.0.1:8730
chain identity (pin it: sion verify --chain …): 6345655cfd2a714ed1dab69c0660f2f48f429ce2981e25629d0467d7b7ee97b7
accounts funded at genesis: alice, bob, carol
Pulse 228  certified 0  round 7  executed [0,0,0]  claims 8  objects 95  proven null
Pulse 230  certified 230  round 14  executed [1,0,0]  claims 8  objects 97  proven 230
```

The API answered 2 seconds after start. The chain identity is unchanged,
and balances and claims are where they were:

```text
$ sion balance dana
account dab40d27bb62fa8dc40c1bf8194f6d04bff293ac65dd08ace075a8d6d5cec5be (Pulse 228)
  29.4889 USD
  1 coins, 0 allotments
```

Right after a restart, `certified_pulse` is 0 until the first header of the
new session is certified, and the `executed` counts start again from zero:
they count transactions executed by this process, not by the chain. The
status reports the new session as `"session": 4294967296`, which is the
restart generation (1) times 2³² plus the epoch (0).

When the restarted devnet stopped cleanly with `--duration`, each validator
wrote a final snapshot, and the lane log had been compacted from 178,780 to
19,820 bytes:

```text
-rw-r--r-- 1 root root   19820 Oct 28 04:19 lanes.log
-rw-r--r-- 1 root root   56652 Oct 28 04:17 snapshot-0000000000000200.bin
-rw-r--r-- 1 root root   61615 Oct 28 04:20 snapshot-0000000000000239.bin
-rw-r--r-- 1 root root 1276891 Oct 28 04:20 wal-0000000000000000.log
```

Starting another configuration on the same directory fails:

```text
$ sion devnet --data-dir devnet-data --accounts alice
error: configuration: devnet-data holds another chain (genesis 30de983de39a060dc990e95512f3f51adc2c5570006ed775c9dc81fab667921b); use a fresh directory
```

What a restart does not keep: Pulse summaries, receipts, data shares and
World Proofs are held in memory, so they are only available for Pulses
executed since the restart.

## Experiment modes

### `--no-prove`

The prover is off, so there are no World Proofs. `sion verify` without
`--no-world` then fails:

```text
$ sion verify
error: verification failed: no certified Pulse yet
```

Use `sion verify --no-world`, which checks everything except the World
Proof.

### `--insecure-fast-signatures`

Validators sign with a keyed hash whose verifier can forge, instead of dual
post-quantum signatures. The devnet warns at startup, the chain identity
records the mode, and every report says so:

```text
WARNING: validators use an INSECURE simulation MAC (--insecure-fast-signatures)
```

Without the cost of post-quantum signatures, an idle devnet on the machine
used for this book closed about 300 Pulses a second, against about 0.3 with
real signatures. That breaks the CLI workflow:

- a transaction built by the CLI is valid for 20 Pulses, which pass before
  it can be certified:
  `error: not certified: the transaction has expired`;
- the node keeps 256 Pulses of history, which passes in under a second, so
  `sion verify` and `sion receipt` find their Pulse already pruned
  (`unknown or pruned Pulse`).

Use this mode for load experiments with your own tooling, not for the
walkthroughs in this book. It secures nothing (`impl:insecure-sim-signatures`).

## Several devnets on one machine

Give each its own `--api` port and `--data-dir`, and point the CLI at it
with `--node` or `SION_NODE`:

```bash
sion devnet --api 127.0.0.1:8731 --data-dir devnet-b
SION_NODE=http://127.0.0.1:8731 sion status
```

Every devnet runs all its validators at full speed. On a small machine,
run one at a time.
