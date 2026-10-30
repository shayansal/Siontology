# Memory from the command line

This guide gives a robot a memory log, records what it sees, commits the
log's head to the ledger, and recalls with proofs. It assumes a devnet is
running (`sion devnet`). For background, see
[Memory for agents and robots](../concepts/memory.md).

## 1. Open a log

```bash
sion keys new robot-7
sion faucet robot-7 10
sion memory open robot-7
```

```text
robot-7 signs: Open a memory log that only the sender can append to. Fee 0.0012 USD; valid until Pulse 22.
Pulse 5: Done: memory log 9755b633 opened; fee 0.0012 USD.
log 9755b63385ef638c61d106b5131d17c506364600f91c96dd93f413aabe096491
```

The log's object on the ledger holds its head; its atoms live in a local
store, `$SION_HOME/memory/<log id>` (default `~/.sion/memory`). With
`--agent <id>`, the log belongs to an agent identity whose agent is the
wallet, and it cannot grow while that agent, or any agent above it, is
suspended or revoked.

## 2. Record atoms

```bash
T=1790812800   # seconds since the Unix epoch
sion memory remember robot-7 9755b633 pallet-17 at @aisle-4 \
  --frame warehouse-3 --at 41000,11000,500 --size 600,400,500 --from $T --seconds 30 --confidence 900
sion memory remember robot-7 9755b633 pallet-18 at @aisle-9 \
  --frame warehouse-3 --at 90000,11000,500 --size 600,400,500 --from $((T+5)) --seconds 30
sion memory remember robot-7 9755b633 dock-door-2 open true --kind world-state --from $((T+8))
```

```text
recorded atom 0 (d6ca870a) in log 9755b633; the log holds 1 atoms, 1 not yet committed
recorded atom 1 (083662ea) in log 9755b633; the log holds 2 atoms, 2 not yet committed
recorded atom 2 (7bf542e1) in log 9755b633; the log holds 3 atoms, 3 not yet committed
```

Each atom is a statement (subject, predicate, value), when it held, and,
for robots, where: a box in a frame, in millimetres. `@name` is an entity,
`true` and `false` booleans, numbers integers, anything else text.
Recording costs nothing and sends nothing: the atoms stay on the machine.

## 3. Commit

```bash
sion memory commit robot-7 9755b633
```

```text
robot-7 signs: Commit memory log 9755b633 at 3 atoms, proving it only grew. Fee 0.0011 USD; valid until Pulse 25.
Pulse 8: Done: memory log 9755b633 now holds 3 atoms (head 1); fee 0.0011 USD.
```

The commit carries the log's size, its Merkle root, its index root and a
proof that the log extends the head the ledger held. Pulse 8 is now the
transaction time of these three atoms: the robot cannot backdate them, and
it can never change or remove them, because a later head that did would
not extend this one.

## 4. Recall, with proof

```bash
sion memory recall 9755b633 warehouse-3 --min 40000,10000,0 --max 42000,12000,3000 --from $T --until $((T+60))
```

```json
{
  "as_of": { "index": "2b4ff11f…", "pulse": 8, "size": 3 },
  "atoms": [
    {
      "confidence_permille": 900,
      "kind": "observation",
      "seq": 0,
      "statement": "pallet-17 at aisle-4",
      "valid_from_s": 1790812800,
      "valid_until_s": 1790812830
    }
  ],
  "proof_bytes": 424,
  "verified": "complete against the index root the ledger holds"
}
```

The answer is everything the log had recorded in that box during that
minute, as of its committed head, and the proof checks it against the
index root the ledger holds: an atom left out would make it fail. An empty
answer is proved too:

```bash
sion memory recall 9755b633 warehouse-3 --min 60000,10000,0 --max 62000,12000,3000 --from $T --until $((T+60))
```

```json
{ "atoms": [], "proof_bytes": 520, "verified": "complete against the index root the ledger holds" }
```

Here the writer proves its own memory to itself. Between parties, the
writer serves the answer and its proof, and the reader checks them against
the head it reads from the ledger: the `robots` demo does exactly that.

## 5. Keep going

```bash
sion memory remember robot-7 9755b633 pallet-17 at @aisle-5 \
  --frame warehouse-3 --at 51000,11000,500 --size 600,400,500 --from $((T+40)) --seconds 30
sion memory commit robot-7 9755b633
sion memory show 9755b633
```

```text
Pulse 11: Done: memory log 9755b633 now holds 4 atoms (head 2); fee 0.0011 USD.
```

`sion memory show` prints the head on the ledger (size, roots, number of
commits, the Pulse of the last one, whether the log is sealed) and how many
atoms the local store holds. `--seal` on a commit closes the log for good.

## 6. Archive with a storage provider

`sion memory archive` packs every atom the ledger holds (committing
first what the local store holds beyond it), encrypts them for the
writer, names the archive's storage root as the log's manifest on the
ledger, and posts a storage deal for it, on the same terms as
`sion storage post`:

```text
$ sion memory archive alice 52f798dc…cc92 --price 0.05 --rounds 20 --round-length 8 --provider bob --local
alice signs: Commit memory log 52f798dc at 3 atoms, proving it only grew. Fee 0.0011 USD; valid until Pulse 52.
alice signs: Commit memory log 52f798dc at 3 atoms, proving it only grew, its manifest naming archive 83913807. Fee 0.0011 USD; valid until Pulse 54.
Pulse 34: Done: memory log 52f798dc now holds 3 atoms (head 1); fee 0.0011 USD.
Pulse 36: Done: memory log 52f798dc now holds 3 atoms (head 2); fee 0.0011 USD.
archive $SION_HOME/memory/archives/52f798dc…-3.bin (530 bytes, encrypted for alice): 3 atoms at root d96b7379; storage root 83913807, now the log's manifest
alice signs: Escrow 1 USD to have data 83913807 (1 leaves) kept by bob for 20 rounds of 8 Pulses: 0.05 USD per round proven by 4 random leaves, with 0 USD of collateral at stake. Fee 0.0011 USD; valid until Pulse 56.
Pulse 38: Done: storage deal dcc80610 posted: 1 leaves, 20 rounds at 0.05 USD each, 0 USD of collateral, for bob; fee 0.0011 USD.
deal dcc80610…a00f
```

The archive is encrypted with ChaCha20-Poly1305 under a key derived from
the writer's wallet seed and the log, so bob keeps 530 bytes he cannot
read; its header, in the clear, names the log and its size and root. The
second commit is at the same size: it keeps the root and moves only the
manifest, which later commits carry forward:

```text
$ sion memory show 52f798dc…cc92
{
  "committed": {
    "at_pulse": 36,
    "commits": 2,
    "index": "7af1af08…94f8",
    "manifest": "83913807…53ea",
    "root": "d96b7379…9350",
    "sealed": false,
    "size": 3
  },
  "local_atoms": 3
}
```

The provider, started with `--listen`, fetches the archive, accepts the
deal and proves every round; it also serves the data of the deals it
keeps to their clients:

```text
$ sion storage serve bob --allow-local --listen 127.0.0.1:8741 --pulses 40
bob provides storage from Pulse 38; copies under $SION_HOME/storage/bob
retrievals for clients on http://127.0.0.1:8741 (POST /v1/deals/<deal>/retrieve)
fetching deal dcc80610 from file://$SION_HOME/memory/archives/52f798dc287f0dc85881d7f342295b5567e28d6e70ad…
bob signs: Accept storage deal dcc80610, posting its collateral; prove each round or lose it. Fee 0.0011 USD; valid until Pulse 58.
Pulse 41: Done: bob accepted storage deal dcc80610 (custody record f8769f9a); rounds start at Pulse 48; fee 0.0011 USD.
```

## 7. Restore

When the local store is lost, `sion memory restore` brings it back from
the provider: it finds the deal whose root is the log's manifest, asks
the provider for the data with a request the deal's client signs, and
checks the archive before it writes anything: its storage root is the
manifest the head names on the ledger, it opens with the wallet's key,
and its atoms rebuild the root of a head the ledger committed.

```text
$ sion memory restore alice 52f798dc…cc92 --from http://127.0.0.1:8741
{
  "atoms_on_ledger": 3,
  "checked": "the archive is the one the log's head names as its manifest, opens with this wallet's key, and rebuilds the head the ledger committed at Pulse 36",
  "log": "52f798dc…cc92",
  "restored_atoms": 3,
  "root": "d96b7379…9350"
}
```

```text
$ sion memory restore mallory 52f798dc…cc92 --from http://127.0.0.1:8741
error: log 52f798dc is written by another account
```

If the log grew after its latest archive, the restore says how many
atoms the archive does not hold, and the log takes no new atoms until
they are back: archive as often as atoms matter (a scheduled job can run
`sion memory archive`). An archive never holds the store's epoch keys, so
payloads sealed under them stay sealed, forgetting holds for archives
too, and a restore keeps the key file it finds.

## For agents

The MCP server offers `sion_memory_open`, `sion_memory_remember`,
`sion_memory_commit` and `sion_memory_recall`, with the same behaviour.

## What the command line does not do yet

Revisions, conflicts, belief, sealed payloads, forgetting and omission
proofs are in the `sion-memory` library and in the `robots` demo
(`sion demo robots --local`), not yet in commands.
