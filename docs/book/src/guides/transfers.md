# Transfers

How to move money with the CLI, what each command prints, and what can go
wrong. [Your first transfer](../getting-started/first-transfer.md) walks
through the basics; this guide covers the rest.

## Funding a wallet

On a devnet, `sion faucet` mints coins:

```text
$ sion faucet dana 100
funded dana with 100
```

```text
Usage: sion faucet [OPTIONS] <ACCOUNT> <AMOUNT>

Arguments:
  <ACCOUNT>  Wallet name or account ID
  <AMOUNT>   Amount (decimal)

Options:
      --asset <ASSET>  Asset: usd, a name such as gpu-hour, or a hex ID [default: usd]
```

The faucet accepts 1 micro-unit to 1,000,000 units per request:

```text
$ sion faucet alice 2000000
error: amount must be between 1 micro-unit and one million units
```

Grants are queued and minted in batches of up to 256 per faucet
transaction, one transaction at a time, so a busy faucet can take a few
Pulses. Faucet transactions pay no fee. A devnet's faucet is a genesis
account; a production chain would have none.

## Sending

```text
Usage: sion send [OPTIONS] <FROM> <TO> <AMOUNT>

Arguments:
  <FROM>    Sending wallet
  <TO>      Recipient (wallet name or account ID)
  <AMOUNT>  Amount (decimal)

Options:
      --asset <ASSET>  Asset [default: usd]
      --forced         Post it as a forced intent, straight to consensus, bypassing the gateway (for when your transactions are being ignored)
```

```text
$ sion send dana alice 12.5
dana signs: Pay 12.5 USD to alice. Fee 0.0012 USD; valid until Pulse 22.
Pulse 5: Done: paid 12.5 USD to alice; fee 0.0012 USD.
  digest 0b4b87c8ed9db1d35d2548e9dfa2d7c529c680a2cd264d9cfcd683f85b897d39
```

- Amounts have at most six decimals; one unit is 10⁶ micro-units.
- The fee is always paid in USD, from the first input. Other assets need a
  USD coin for the fee as well.
- The command returns when the transaction has executed, typically a few
  seconds on the devnet (8.3 seconds for the transfer above, on a busy
  2-core machine). It waits up to about a minute and a half.
- A transfer is final when its lane certificate forms, before it
  executes: no conflicting spend of the same coin can be certified after
  that.

### Fees

The minimum fee is `0.001 USD + 0.0001 USD × ⌈body / 1024⌉`, where the body
is the encoded transaction without signatures. A sender's first
transaction carries its genesis policy, about 1.3 KB of public keys, so it
pays 0.0012 USD; later transfers fit in 1 KiB and pay 0.0011 USD:

```text
$ sion send dana alice 1
dana signs: Pay 1 USD to alice. Fee 0.0011 USD; valid until Pulse 248.
Pulse 230: Done: paid 1 USD to alice; fee 0.0011 USD.
```

Fees go to the treasury. 70 % of each epoch's fees is paid to that epoch's
validators ([Rewards](../concepts/validators/rewards.md)).

### Coins and parallel transactions

Balances are coins, and a transaction consumes whole coins and creates
change. Dana's 100 USD from the faucet was one coin; after the transfer she
again had one coin (her change) and alice had one more:

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

A coin can be locked to one transaction at a time. An account with one coin
can therefore have one transaction in flight; the genesis accounts start
with four coins each. The wallet does not merge coins: it selects the
largest first.

## Double spends

Two transactions that spend the same coin version cannot both be
certified. To show this, two transfers from the same wallet, to different
recipients, were signed by separate processes (which do not share the
wallet's memory of pending coins) and submitted at the same time:

```text
$ curl -s http://127.0.0.1:8730/v1/transactions/93d6ac50b1dcaa95d2c9115eff5e357276f9b356d01637901beeb9c2955ee8b5 | jq -c '{state, status: .record.status, pulse: .record.pulse}'
{"state":"executed","status":"Success","pulse":9}
$ curl -s http://127.0.0.1:8730/v1/transactions/8f26f159c0723de6c19a5e1f2fa4f8ea5a2e2ec2eace19d8ffe823db99df69e5
{"reason":"object ObjectRef { id: ObjectId(27657e3b), version: Version(0) } is locked to transaction TxDigest(93d6ac50)","state":"rejected"}
$ sion tx 8f26f159c0723de6c19a5e1f2fa4f8ea5a2e2ec2eace19d8ffe823db99df69e5
rejected: object ObjectRef { id: ObjectId(27657e3b), version: Version(0) } is locked to transaction TxDigest(93d6ac50)
$ sion receipt 8f26f159c0723de6c19a5e1f2fa4f8ea5a2e2ec2eace19d8ffe823db99df69e5
error: verification failed: never certified: object ObjectRef { id: ObjectId(27657e3b), version: Version(0) } is locked to transaction TxDigest(93d6ac50)
```

The validators locked the coin to whichever transaction reached them
first; the other could not gather a quorum and was rejected without a fee.
Within one process, the CLI's wallet never builds two transactions on the
same coin.

## Looking a transaction up

| Command | What it shows |
|---|---|
| `sion tx <digest>` | The node's record: the outcome sentence, or `certifying`, `sequencing`, `rejected: …` |
| `sion receipt <digest> [--chain <id>]` | The same, verified as a light client: the Pulse certificate, the certified order and the receipt's Merkle proof |
| `GET /v1/transactions/{digest}` | The raw record ([HTTP API](../reference/http-api.md#get-v1transactionsdigest)) |

The node keeps records for the last 256 Pulses. For an older transaction,
`sion tx` prints `sequencing` and `sion receipt` fails: the record is gone,
and the gateway's last status is all that is left.

## When it goes wrong

```text
$ sion send nobody alice 5
error: insufficient funds: need 5.001, have 0
$ sion send alice bob 1.1234567
error: "1.1234567" is not an amount with at most 6 decimals
$ sion send alice bob abc
error: "abc" is not an amount
$ sion send alice bob 99999999
error: insufficient funds: need 99,999,999.001, have 100,000
```

`need` includes the fee the wallet would pay. More messages are in
[Error messages](../reference/errors.md).

If a transfer does not execute (`error: <digest> did not execute in time`),
the gateway may be ignoring it. Post it as a forced intent with `--forced`;
see [Forced inclusion and the censorship demo](forced-inclusion.md).
