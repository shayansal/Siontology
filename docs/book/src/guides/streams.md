# Streams from the command line

This guide opens a [payment stream](../concepts/payment-streams.md) from
alice to bob on a devnet, pays from it by hand, checks payments as a third
party would, settles by walking and by a jump to a checkpoint, tops the
stream up and closes it. For streams that pay an HTTP API automatically, see
[x402](x402.md#the-stream-scheme-one-hash-per-request).

Start a devnet in another terminal first ([Running a devnet](devnet.md)).
The outputs below are from a real run; identifiers and Pulses will differ on
yours.

## Open a stream

```text
$ sion stream open alice bob --unit 0.001 --steps 10000 --deposit 5 --pulses 2000
alice signs: Open a payment stream to bob: escrow 5 USD, paid off the ledger at 0.001 USD a step for up to 10,000 steps, settled by bob until Pulse 2111, with a checkpoint every 64 steps. Fee 0.0011 USD; valid until Pulse 132.
Pulse 114: Done: alice opened stream e21f72c8 to bob with 5 USD in escrow; fee 0.0011 USD.
  digest 6743fe4feb09570ed950390b11688e9013694cec9b9eae03cfdfbe6e63efef70
stream e21f72c8ce111466ed0d66f259c88adbb2981ae5aabb1b6bfcaa6990ac1e1938
its chain's secret seed is in /home/you/.sion/streams/e21f72c8ce111466ed0d66f259c88adbb2981ae5aabb1b6bfcaa6990ac1e1938.json
```

- `--unit` is the value of one step, `--steps` the length of the chain: the
  most the stream can ever pay is their product, 10 USD here. The escrow,
  5 USD, is what it can pay now.
- `--pulses` is how long bob has to settle. After that, alice may close the
  stream and take back whatever bob has not settled.
- The chain gets a checkpoint every 64 steps by default (`--spacing`), so
  bob can settle any number of payments for a bounded number of hashes.
  `--spacing 0` opens a stream without checkpoints.

The command builds the chain from a random secret, one hash per step, and
keeps the secret and the position revealed so far in
`$SION_HOME/streams/<stream>.json`:

```json
{
  "stream": "e21f72c8ce111466ed0d66f259c88adbb2981ae5aabb1b6bfcaa6990ac1e1938",
  "wallet": "alice",
  "payee": "a27e5e28be5f6fcef56c7adbd682ceb0bc6966f666e87696998c743dc6f88b37",
  "unit": 1000,
  "length": 10000,
  "seed": "…",
  "revealed": 0,
  "spacing": 64,
  "announced": null
}
```

**Guard that file like a wallet seed.** Anyone who reads `seed` can pay
from the stream for the rest of its chain.

## Pay

Each `sion stream pay` reveals the next element and prints the payment, to
hand to bob however you like:

```text
$ sion stream pay e21f72c8ce111466ed0d66f259c88adbb2981ae5aabb1b6bfcaa6990ac1e1938
e21f72c8ce111466ed0d66f259c88adbb2981ae5aabb1b6bfcaa6990ac1e1938:1:e6f8a8b433d8f490c575fda4c02fb1dc98530eb2aeb5684d5f83a5a424ba00ed

$ sion stream pay e21f72c8ce111466ed0d66f259c88adbb2981ae5aabb1b6bfcaa6990ac1e1938
e21f72c8ce111466ed0d66f259c88adbb2981ae5aabb1b6bfcaa6990ac1e1938:2:72caf7a8e940a9b55dc9c5fdfba11174b6251a21db319eeade5062c475bfb165
```

A payment is `<stream>:<position>:<element>`. Position 2 pays two steps in
all, 0.002 USD. The command writes the new position to disk before it
prints, so it never reveals the same position for two payments.

`--steps` pays several steps at once. The first payment past a checkpoint
also prints the checkpoint's **jump**, `<index>:<path>`, which bob needs to
settle by checkpoint:

```text
$ sion stream pay e21f72c8ce111466ed0d66f259c88adbb2981ae5aabb1b6bfcaa6990ac1e1938 --steps 98
e21f72c8ce111466ed0d66f259c88adbb2981ae5aabb1b6bfcaa6990ac1e1938:100:e20c5862b67585a59249956075e89c28f35d626805f1bb169d85239647733801
jump 1:7671447d0bb012a0f8fb4ac1403d3f3eca842797825ede27ec81070b4ea78717,2702da845ec3957c73a6e531ce7c2d2359e16e38f2cfd31adb6b21e5be98fec7,…
```

`sion stream jump <stream> --position <p>` prints the jump for any position
again.

## Check a payment

Anyone can check a payment against the ledger: bob before he accepts it,
or an auditor years later.

```text
$ sion stream check e21f72c8…1938:100:e20c5862…3801
genuine, not yet settled: 100 steps above the settled position, worth 0.1 USD to the payee

$ sion stream check e21f72c8…1938:100:e20c5862…3801 --jump 1:7671447d…,…
genuine, not yet settled: 100 steps above the settled position, worth 0.1 USD to the payee

$ sion stream check e21f72c8…1938:100:abababababababababababababababababababababababababababababababab
error: not an element of this stream's chain
```

Without `--jump` the check walks from the payment down to the stream's
head, one hash per step; with it, at most the spacing plus 21 hashes,
however far apart they are. (Identifiers are shortened here; the commands
take them in full.)

In practice a payee checks each payment against the last one it accepted,
not against the ledger: one hash per step and no network round trip. That
is what an [x402 server](x402.md#the-stream-scheme-one-hash-per-request)
does.

## Settle

Bob settles the steps he holds. A settlement without `--jump` walks from
the head, one hash per step and at most 65,536 steps:

```text
$ sion stream settle bob e21f72c8…1938:1:e6f8a8b4…00ed
bob signs: Settle stream e21f72c8 up to step 1. Fee 0.001102 USD; valid until Pulse 144.
Pulse 127: Done: stream e21f72c8 settled to step 1: 0.001 USD paid; fee 0.001102 USD.
  digest 1b598957d713ed70a8862db2bbb12648bf72d536babe1d9066ad76fed03006f3
```

With the jump alice handed over, a settlement walks only to the checkpoint
below it and checks the checkpoint's Merkle path. Here that is 36 steps to
checkpoint 1 (position 64), the leaf and an 8-hash path: 45 hashes, 90
micro-USD on top of the minimum fee, whether it settles 99 steps or 99
million:

```text
$ sion stream settle bob e21f72c8…1938:100:e20c5862…3801 --jump 1:7671447d…,…
bob signs: Settle stream e21f72c8 up to step 100, proven by checkpoint 1. Fee 0.00119 USD; valid until Pulse 147.
Pulse 130: Done: stream e21f72c8 settled to step 100: 0.099 USD paid; fee 0.00119 USD.
  digest 47e7f6e02a82f409a93d0c4eaeabe45bc062db87538418262ab599701dcea1d1
```

Once settled, a payment checks as settled:

```text
$ sion stream check e21f72c8…1938:1:e6f8a8b4…00ed
genuine and settled: the ledger has settled the stream up to position 100 (checked with 99 hashes)
```

## Top up, close and look

```text
$ sion stream show e21f72c8ce111466ed0d66f259c88adbb2981ae5aabb1b6bfcaa6990ac1e1938
stream e21f72c8ce111466ed0d66f259c88adbb2981ae5aabb1b6bfcaa6990ac1e1938
  alice pays bob 0.001 USD per step, 100 of 10,000 steps settled, a checkpoint every 64 steps
  escrow 4.9 USD, settle by Pulse 2111, open

$ sion stream top-up alice e21f72c8ce111466ed0d66f259c88adbb2981ae5aabb1b6bfcaa6990ac1e1938 --deposit 1 --pulses 100
alice signs: Add 1 USD to stream e21f72c8 and keep it open until Pulse 2211. Fee 0.0011 USD; valid until Pulse 217.
Pulse 200: Done: stream e21f72c8 topped up by 1 USD, open until Pulse 2211; fee 0.0011 USD.
  digest 986d759194379c6fda592e859ee3ce925b4f121e69337f178f0f9cddcc7597d9

$ sion stream close bob e21f72c8ce111466ed0d66f259c88adbb2981ae5aabb1b6bfcaa6990ac1e1938
bob signs: Close stream e21f72c8 and return what is left in escrow to its payer. Fee 0.0011 USD; valid until Pulse 220.
Pulse 202: Done: stream e21f72c8 closed: 5.9 USD returned to its payer; fee 0.0011 USD.
  digest 65d718627351855f09a79d844734c52f5acd903bd0238fa58f123879029f1951
```

Alice had revealed position 101 before bob closed. Bob never settled it,
so it was not paid: closing ends the stream. A payee settles before it
closes, as the x402 server does when it stops.

## What can go wrong

| You see | Because | Do |
|---|---|---|
| `stream … has 0 of 10000 steps left` | The chain is used up | Open a new stream |
| `Failed (… is more than the … left in escrow)` | The settlement asks for more than the escrow holds | The payer tops up; the payee settles only what is covered |
| `Failed (the stream has expired)` | The last Pulse passed | The payer closes the stream to take the escrow back |
| `Failed (at most 65536 steps per settlement without a jump)` | A walk covers at most 65,536 steps | Settle with `--jump`, or in several settlements |
| `Failed (the jump does not apply: …)` | No checkpoints, or the checkpoint is not the one just below the position | Use the jump for checkpoint `position / spacing` |
| `error: no chain for stream …` | This machine's `$SION_HOME/streams` does not hold the stream's chain | Pay from the machine that opened it |
