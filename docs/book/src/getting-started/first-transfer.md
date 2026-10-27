# Your first transfer

With a devnet running (see [Run a devnet](devnet.md)), create a wallet,
fund it from the faucet and pay someone. Every command below was run
against a real devnet; your identifiers and Pulse numbers will differ.

## 1. Create a wallet

```text
$ sion keys new dana
created dana: account dab40d27bb62fa8dc40c1bf8194f6d04bff293ac65dd08ace075a8d6d5cec5be
seed stored in $HOME/.sion/wallets/dana.seed
```

The CLI prints the seed file's absolute path. The seed is 32 random bytes,
hex-encoded. Anyone who reads it controls the account.

```text
$ sion keys show dana
account dab40d27bb62fa8dc40c1bf8194f6d04bff293ac65dd08ace075a8d6d5cec5be
ML-DSA-44 key   621fafde6a1a72503c71b2a1e9c5da1212d5e3eb4560f4e13c21485196269d79
SLH-DSA key     412112d70e98c1a97c2dffa3b396fc99caae90e6c1fccef6ecedbd5b14eb0333
```

The account identifier is the hash of the wallet's genesis policy: one
ML-DSA-44 key and one SLH-DSA-SHA2-128f key, both required. The two lines
below it are fingerprints of the public keys.

## 2. Fund it

```text
$ sion balance dana
account dab40d27bb62fa8dc40c1bf8194f6d04bff293ac65dd08ace075a8d6d5cec5be (Pulse 0)
  0 coins, 0 allotments

$ sion faucet dana 100
funded dana with 100

$ sion balance dana
account dab40d27bb62fa8dc40c1bf8194f6d04bff293ac65dd08ace075a8d6d5cec5be (Pulse 2)
  100 USD
  1 coins, 0 allotments
```

`sion faucet` asks the devnet to mint coins; the gateway batches waiting
grants (up to 256) into one faucet transaction at a time. The command
waits until the coins arrive (about 5 seconds here).

## 3. Pay

```text
$ sion send dana alice 12.5
dana signs: Pay 12.5 USD to alice. Fee 0.0012 USD; valid until Pulse 22.
Pulse 5: Done: paid 12.5 USD to alice; fee 0.0012 USD.
  digest 0b4b87c8ed9db1d35d2548e9dfa2d7c529c680a2cd264d9cfcd683f85b897d39
```

The first line is what the wallet signed, as a sentence. The second is what
happened, also as a sentence, and the Pulse it executed in. Behind it:

1. The wallet picked a coin, computed the minimum fee for the
   transaction's size, set it to expire 20 Pulses from now, and signed it
   with both keys for this chain.
2. Dana's shard committee locked the coin to this transaction; three of
   the four validators' approvals formed a lane certificate. From here the
   payment is final: no conflicting spend of that coin can be certified.
3. Consensus put the certificate into a Pulse, every validator executed the
   Pulse and signed its header, and the prover built its World Proof.

The fee was 0.0012 USD because this was dana's first transaction: it
carries her genesis policy (including a 1,312-byte public key), so the
body is just over 1 KiB. Later transfers pay 0.0011 USD.

```text
$ sion balance dana
account dab40d27bb62fa8dc40c1bf8194f6d04bff293ac65dd08ace075a8d6d5cec5be (Pulse 5)
  87.4988 USD
  1 coins, 0 allotments

$ sion balance alice
account 98252ec61e5fe25d4e2cf6560119730fda49b549ae2b64e1acc04ce3e3633147 (Pulse 5)
  100,012.5 USD
  5 coins, 0 allotments
```

Dana's coin was consumed (it is a tombstone now) and two coins were
created: 12.5 USD for alice and the change for dana.

## 4. Look it up

```text
$ sion tx 0b4b87c8ed9db1d35d2548e9dfa2d7c529c680a2cd264d9cfcd683f85b897d39
Pulse 5: Done: paid 12.5 USD to alice; fee 0.0012 USD.
  digest 0b4b87c8ed9db1d35d2548e9dfa2d7c529c680a2cd264d9cfcd683f85b897d39

$ sion events --limit 8
Pulse     2  {"Minted":{"to":"dab40d27bb62fa8dc40c1bf8194f6d04bff293ac65dd08ace075a8d6d5cec5be","asset":"1436ac5e7c42d6cc93304083c0c6c8c964e939aa391c2e0a7304810b7e368f3c","amount":100000000}}
Pulse     5  {"Transferred":{"asset":"1436ac5e7c42d6cc93304083c0c6c8c964e939aa391c2e0a7304810b7e368f3c","outputs":[["98252ec61e5fe25d4e2cf6560119730fda49b549ae2b64e1acc04ce3e3633147",12500000]]}}
```

Amounts in events and in JSON are micro-units: `12500000` is 12.5 USD.
`1436ac5e…` is the USD asset.

## 5. Check the receipt as a light client

```text
$ sion receipt 0b4b87c8ed9db1d35d2548e9dfa2d7c529c680a2cd264d9cfcd683f85b897d39 --chain 6345655cfd2a714ed1dab69c0660f2f48f429ce2981e25629d0467d7b7ee97b7
Pulse 5: Done: paid 12.5 USD to alice; fee 0.0012 USD.
  certified by 300000 of the committee's stake (quorum 266667), dual post-quantum signatures ✓
  receipt proven against the certified header's receipts root ✓
  chain 6345655c (pinned)
```

`sion receipt` does not take the node's word: it checks that the node
serves the chain you pinned, verifies the committee's signatures on the
Pulse header (each genesis validator stakes 100,000 USD, so its weight is
100,000 and three of the four make the quorum of 266,667), checks that the transaction sits at its position in the
certified order, and proves the receipt against the header's receipts
root. Without `--chain` it says `NOT pinned: pass --chain to pin it`.

The node keeps transaction records for the last 256 Pulses. After that it
no longer has the receipt. A known issue: `sion tx` then prints
`sequencing` (the gateway's last status for the transaction) instead of
saying that the record was pruned, and `sion receipt` fails with
`not executed yet (sequencing)`.

## Next

- [Verify as a light client](light-client.md): check a whole Pulse.
- [Transfers](../guides/transfers.md): other assets, forced sends, what can
  go wrong.
