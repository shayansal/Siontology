# Storage from the command line

This guide has alice pay to have a file kept and bob keep it, on a local
devnet: post a deal, accept it, prove a round by hand, then let a provider
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

Alice posts an open deal: 4 rounds of 16 Pulses at 0.25 USD a proven
round, 4 leaves challenged per round, 2 USD of collateral. She escrows
1 USD. Bob runs on her machine in this guide, so `--local` points
providers at the file itself:

```text
$ sion faucet alice 50
$ sion storage post alice notes.txt --price 0.25 --rounds 4 --round-length 16 --collateral 2 --local
alice signs: Escrow 1 USD to have data 44057875 (18 leaves) kept by any provider for 4 rounds of 16 Pulses: 0.25 USD per round proven by 4 random leaves, with 2 USD of collateral at stake. Fee 0.0011 USD; valid until Pulse 22.
Pulse 5: Done: storage deal 7254c0de posted: 18 leaves, 4 rounds at 0.25 USD each, 2 USD of collateral, open to any provider; fee 0.0011 USD.
deal 7254c0de4012cf90ee81326b3b17b30adbe313819b03bace9c5b84fc52ad66cd
```

Where providers fetch the data is the deal's **locator**: `--locator`
gives an `https://` URL serving the file (any static web server), for
providers anywhere; `--local` gives `file://` and the file's absolute
path, which only a provider on this machine can read, and which the deal
then publishes. One of the two is required. Other options: `--provider
<name>` names the only account that may accept (and then collateral
defaults to none); `--collateral` (an open deal must ask for some, and by
default asks for its escrow); `--challenges` (1 to 16, default 4);
`--round-length` (a multiple of 8, from 8 to 100,000, default 16);
`--accept-within` Pulses (default 200).

Proofs publish the challenged leaves: encrypt data that must stay private
before you prepare it.

```text
$ sion storage deals --open
7254c0de  18 leaves, 0.25 USD × 4 rounds of 16 Pulses, collateral 2 USD, for anyone: open
```

## Accept

Bob accepts: the command fetches the data from the locator (or `--file`),
checks that it hashes to the deal's root, posts the collateral, and keeps
a copy under `$SION_HOME/storage/bob` once the acceptance went through:

```text
$ sion faucet bob 10
$ sion storage accept bob 7254c0de
bob signs: Accept storage deal 7254c0de, posting its collateral; prove each round or lose it. Fee 0.0011 USD; valid until Pulse 28.
Pulse 11: Done: bob accepted storage deal 7254c0de (custody record 5b2cd3b9); rounds start at Pulse 16; fee 0.0011 USD.
```

Rounds start on the next multiple of 8 Pulses. `show` gives the deal and,
once accepted, the provider's progress:

```text
$ sion storage show 7254c0de
{
  "challenges": 4,
  "collateral": "2",
  "escrow": "1",
  "leaves": 18,
  "price": "0.25",
  "progress": { "end": 80, "next": 0, "proven": 0, "round_now": null },
  "round_length": 16,
  "rounds": 4,
  "state": {
    "bond": "2",
    "custody": "5b2cd3b9…",
    "paid_rounds": 0,
    "phase": "active",
    "provider": "a27e5e28…",
    "start": 16
  },
  …
}
```

## Prove, collect

A round's challenge comes from the beacon of its first Pulse, so there is
nothing to prove until that Pulse has executed:

```text
$ sion storage prove bob
nothing to prove: no round under way that is not proven yet
```

From Pulse 16, `prove` answers the round under way for every deal bob
keeps, in as few transactions as the limits allow (64 leaves and 192 KiB
of proofs each). A round is proven once; asking again in the same round
does nothing:

```text
$ sion storage prove bob
bob signs: Prove custody of 1 storage deal(s) for the round under way with 4 challenged leaves. Fee 0.0027 USD; valid until Pulse 32.
Pulse 19: Done: round 0 of storage deal 7254c0de proven; fee 0.0027 USD.
$ sion storage prove bob
nothing to prove: no round under way that is not proven yet
```

Bob collects for the rounds he proved whenever he likes:

```text
$ sion storage settle bob 7254c0de
Pulse 22: Done: 0.25 USD paid for 1 proven round(s) of storage deal 7254c0de; fee 0.0011 USD.
```

## Run a provider

`serve` does all of this every second. Each step proves first, then
collects and closes deals that ended, then accepts deals whose data
arrived, then starts fetching new ones in the background, so a slow
locator never delays a proof. Each proof expires at the end of its round,
so one that would arrive late is refused at no cost instead of failing
for a fee. Bob's provider is on alice's machine, so it is told it may read
local files:

```text
$ sion storage serve bob --allow-local --pulses 70
bob provides storage from Pulse 22; copies under …/storage/bob
Pulse 36: Done: round 1 of storage deal 7254c0de proven; fee 0.0027 USD.
Pulse 52: Done: round 2 of storage deal 7254c0de proven; fee 0.0027 USD.
Pulse 67: Done: round 3 of storage deal 7254c0de proven; fee 0.0027 USD.
Pulse 83: Done: storage deal 7254c0de closed: 0 USD refunded, 0.75 USD paid, 2 USD of collateral returned, 0 USD forfeited, 0 round(s) missed; fee 0.0011 USD.
```

Bob proved all four rounds: 1 USD in all (0.25 collected earlier, 0.75 at
the close), and his collateral back; his copy was deleted at the close.
Each proof cost him a fee of 0.0027 USD: a proof carries the four
challenged leaves, 4 KiB each, and fees grow with size.

A provider taking strangers' deals keeps its terms modest. The defaults:
at least 0.01 USD a round (`--min-price`) and twice what proving a round
costs in fees; at most 10 USD of collateral a deal (`--max-collateral`)
and 100 USD in all (`--max-total-collateral`); 64 MiB a deal
(`--max-mib`) and 1 GiB in all (`--max-total-mib`); deals of at most
100,000 Pulses (`--max-duration`). Without `--allow-local` it refuses
`file://` locators, and without `--allow-private` it does not fetch from
loopback, private or other local network addresses.

## Refunds and refusals

An open deal nobody accepts is refunded by its client; nobody else can
close it:

```text
$ sion storage post alice notes.txt --price 0.1 --rounds 2 --accept-within 30 --local
Pulse 96: Done: storage deal 6dff254d posted: 18 leaves, 2 rounds at 0.1 USD each, 0.2 USD of collateral, open to any provider; fee 0.0011 USD.
$ sion storage close bob 6dff254d
Pulse 98: Failed (unauthorized: only the client closes an open deal); nothing happened except the fee of 0.0011 USD.
$ sion storage close alice 6dff254d
Pulse 100: Done: storage deal 6dff254d closed: 0.2 USD refunded, 0 USD paid, 0 USD of collateral returned, 0 USD forfeited, 0 round(s) missed; fee 0.0011 USD.
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
