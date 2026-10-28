# Storage from the command line

This guide has alice pay to have a file kept and bob keep it, on a local
devnet: post a deal, accept it, prove rounds by hand, then let a provider
loop run to the end. [Storage deals](../concepts/storage.md) explains the
rules. Every output below is from a real run (long paths and digests
shortened with `…`).

Start a devnet in one terminal:

```bash
sion devnet --no-prove
```

## Prepare and post

A deal commits to the file's Merkle root. `prepare` shows it without
posting anything:

```text
$ sion storage prepare notes.txt
notes.txt: 70929 bytes in 18 leaves, root 44057875c2c4475f257879ae3597177b9da0b148b962566eea127e2d131e78c8
```

Alice posts an open deal: 4 rounds of 10 Pulses at 0.25 USD a proven
round, 4 leaves challenged per round, 2 USD of collateral. She escrows
1 USD:

```text
$ sion faucet alice 50
$ sion storage post alice notes.txt --price 0.25 --rounds 4 --round-length 10 --collateral 2
alice signs: Escrow 1 USD to have data 44057875 (18 leaves) kept by any provider for 4 rounds of 10 Pulses: 0.25 USD per round proven by 4 random leaves, with 2 USD of collateral at stake. Fee 0.0011 USD; valid until Pulse 22.
Pulse 5: Done: storage deal 83827abe posted: 18 leaves, 4 rounds at 0.25 USD each, 2 USD of collateral, open to any provider; fee 0.0011 USD.
deal 83827abe5c24d434418fcbd66c17eb1fe69040df5a91460d00cf3eac9089fc68
```

Options: `--provider <name>` names the only account that may accept;
`--challenges` (1 to 16, default 4); `--accept-within` Pulses (default
200); `--locator` where providers fetch the data (by default `file://` and
the file's absolute path, which works on one machine; serve the file over
HTTP for providers elsewhere, with any static web server).

```text
$ sion storage deals --open
83827abe  18 leaves, 0.25 USD × 4 rounds of 10 Pulses, collateral 2 USD, for anyone: open
```

## Accept

Bob accepts: the command fetches the data from the locator (or `--file`),
checks that it hashes to the deal's root, keeps a copy under
`$SION_HOME/storage/bob`, and posts the collateral:

```text
$ sion faucet bob 10
$ sion storage accept bob 83827abe
bob signs: Accept storage deal 83827abe, posting its collateral; prove each round or lose it. Fee 0.0011 USD; valid until Pulse 27.
Pulse 10: Done: bob accepted storage deal 83827abe (custody record 75bda7eb); rounds start at Pulse 11; fee 0.0011 USD.
```

`show` gives the deal and, once accepted, the provider's progress:

```text
$ sion storage show 83827abe
{
  "challenges": 4,
  "collateral": "2",
  "escrow": "1",
  "leaves": 18,
  "price": "0.25",
  "progress": { "end": 51, "next": 0, "proven": 0, "round_now": null },
  "rounds": 4,
  "round_length": 10,
  "state": {
    "bond": "2",
    "custody": "75bda7eb…",
    "paid_rounds": 0,
    "phase": "active",
    "provider": "a27e5e28…",
    "start": 11
  },
  …
}
```

## Prove, collect

`prove` answers the round under way for every deal bob keeps, in as few
transactions as the 64-leaf limit allows. A round is proven once; asking
again in the same round does nothing:

```text
$ sion storage prove bob
bob signs: Prove custody of 1 storage deal(s) for the round under way with 4 challenged leaves. Fee 0.0027 USD; valid until Pulse 31.
Pulse 13: Done: round 0 of storage deal 83827abe proven; fee 0.0027 USD.
$ sion storage prove bob
nothing to prove: no round under way that is not proven yet
```

Bob collects for the rounds he proved whenever he likes:

```text
$ sion storage settle bob 83827abe
Pulse 21: Done: 0.25 USD paid for 1 proven round(s) of storage deal 83827abe; fee 0.0011 USD.
```

## Run a provider

`serve` does all of this every second: it accepts open deals whose data it
can fetch (within `--min-price`, `--max-collateral` and `--max-mib`),
proves every round, collects once four proven rounds are unpaid, and closes
deals that ended. Each proof it sends expires at the end of its round, so
a proof that would arrive late is refused at no cost instead of failing
for a fee:

```text
$ sion storage serve bob --pulses 40
bob provides storage from Pulse 21; copies under …/storage/bob
Pulse 24: Done: round 1 of storage deal 83827abe proven; fee 0.0027 USD.
Pulse 33: Done: round 2 of storage deal 83827abe proven; fee 0.0027 USD.
Pulse 44: Done: round 3 of storage deal 83827abe proven; fee 0.0027 USD.
Pulse 54: Done: storage deal 83827abe closed: 0 USD refunded, 0.75 USD paid, 2 USD of collateral returned, 0 USD forfeited, 0 round(s) missed; fee 0.0011 USD.
```

Bob proved all four rounds: 1 USD in all (0.25 collected earlier, 0.75 at
the close), and his collateral back. Each proof cost him a fee of 0.0027
USD: a proof carries the four challenged leaves, 4 KiB each, and fees grow
with size.

## Refunds and refusals

An open deal nobody accepts is refunded by its client; nobody else can
close it:

```text
$ sion storage post alice notes.txt --price 0.1 --rounds 2 --accept-within 30
Pulse 64: Done: storage deal 56ba4507 posted: 18 leaves, 2 rounds at 0.1 USD each, 0 USD of collateral, open to any provider; fee 0.0011 USD.
$ sion storage close bob 56ba4507
Pulse 66: Failed (unauthorized: only the client closes an open deal); nothing happened except the fee of 0.0011 USD.
$ sion storage close alice 56ba4507
Pulse 68: Done: storage deal 56ba4507 closed: 0.2 USD refunded, 0 USD paid, 0 USD of collateral returned, 0 USD forfeited, 0 round(s) missed; fee 0.0011 USD.
```

A client may also close an accepted deal before its end once its provider
has proven none of three rounds in a row; the provider then forfeits all
of its collateral to the treasury. The `storage` demo shows it:

```bash
sion demo storage --local
```

## From an agent

The MCP server has `sion_storage_post`, `sion_storage_deals` and
`sion_storage_close` ([MCP tools](../reference/mcp.md)): an agent can have
a file kept and watch its deals; providers run `sion storage serve`.
