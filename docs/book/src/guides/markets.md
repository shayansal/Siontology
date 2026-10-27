# Markets and clearing

A market collects orders in batches and settles each batch at one price,
only with a certificate that no better allocation exists. This guide opens
a GPU-hour market, enters four orders, clears the batch and checks the
balances. The concepts are in
[Verifiable Clearing](../concepts/verifiable-clearing.md).

## Open a market

```text
Usage: sion market create [OPTIONS] <OPERATOR> <LABEL>

Options:
      --good <GOOD>          The good (asset name or ID) [default: gpu-hour]
      --batch <BATCH>        Pulses per batch [default: 6]
      --deadline <DEADLINE>  Pulses allowed for clearing [default: 4]
```

```text
$ sion market create operator "gpu-hours before 18:00" --batch 20 --deadline 30
operator signs: Open the market "gpu-hours before 18:00" for GPU-hours priced in USD, clearing every 20 Pulses. Fee 0.0012 USD; valid until Pulse 102.
Pulse 84: Done: market 45544ef8 opened; fee 0.0012 USD.
  digest d2f1bb4c2941019c1c4eeeb69961658433f4f95ba1ae793af43a9f28b7f0acf1

$ sion events --from 84 --limit 2
Pulse    84  {"MarketCreated":{"market":"45544ef8d600ad29d7830a62685a5b3e68e3e82c2bd1a513dc0ab5e766bc25ab"}}
```

The receipt shows the market's short identifier; the full one is in the
`MarketCreated` event. Batches are numbered by Pulse: batch `i` holds the
orders that arrive in Pulses `20·i` to `20·i + 19`, seals at the end of
the last of them, and must be cleared within 30 Pulses of sealing, or every
order is refunded.

The goods themselves are an asset. On a devnet the faucet mints any asset
by name:

```text
$ sion faucet seller1 5 --asset gpu-hour
funded seller1 with 5
$ sion balance seller1
account e795b0225f780c09959eecf67c8aea8be2e2996545436426edf92f92d4beb626 (Pulse 82)
  100 USD
  5 7075a799
  2 coins, 0 allotments
```

`7075a799` is the GPU-hour asset; the balance shows assets other than USD
by their short identifier.

## Enter orders

```text
Usage: sion market order [OPTIONS] <FROM> <MARKET> <SIDE> <QUANTITY> <PRICE>

Arguments:
  <SIDE>      buy or sell
  <QUANTITY>  Units
  <PRICE>     Limit price per unit (USD)
```

An order escrows what it could pay: a buy order escrows quantity × limit
price in USD, a sell order the units it offers. Two buyers and two
sellers, all in batch 5:

```text
$ sion market order buyer1 45544ef8d600ad29d7830a62685a5b3e68e3e82c2bd1a513dc0ab5e766bc25ab buy 5 2.10
buyer1 signs: Bid for 5 units at at most 2.1 each in market 45544ef8. Fee 0.0012 USD; valid until Pulse 119.
Pulse 101: Done: order entered market 45544ef8; fee 0.0012 USD.
$ sion market order buyer2 45544ef8d600ad29d7830a62685a5b3e68e3e82c2bd1a513dc0ab5e766bc25ab buy 3 1.90
buyer2 signs: Bid for 3 units at at most 1.9 each in market 45544ef8. Fee 0.0012 USD; valid until Pulse 122.
Pulse 104: Done: order entered market 45544ef8; fee 0.0012 USD.
$ sion market order seller1 45544ef8d600ad29d7830a62685a5b3e68e3e82c2bd1a513dc0ab5e766bc25ab sell 5 1.80
seller1 signs: Offer 5 units at at least 1.8 each in market 45544ef8. Fee 0.0012 USD; valid until Pulse 124.
Pulse 107: Done: order entered market 45544ef8; fee 0.0012 USD.
$ sion market order seller2 45544ef8d600ad29d7830a62685a5b3e68e3e82c2bd1a513dc0ab5e766bc25ab sell 6 2.00
seller2 signs: Offer 6 units at at least 2 each in market 45544ef8. Fee 0.0012 USD; valid until Pulse 128.
Pulse 111: Done: order entered market 45544ef8; fee 0.0012 USD.
```

(The doubled "at" in these sentences is a wording bug in this version.)

`sion market show` prints the market object. An excerpt of batch 5, with
amounts in micro-units:

```text
$ sion market show 45544ef8d600ad29d7830a62685a5b3e68e3e82c2bd1a513dc0ab5e766bc25ab
{
  "data": {
    "Market": {
      "batch_length": 20,
      "batches": {
        "5": {
          "index": 5,
          "orders": [
            {
              "escrow": 5000000,
              "escrow_asset": "7075a79953ddce076ed864e815ea2357b811264839d50b3288d003a324f21614",
              "id": "1578c8865802bdad5e06319ea92faeb024582dc515db534a61e59ddd5f680b53",
              "owner": "e795b0225f780c09959eecf67c8aea8be2e2996545436426edf92f92d4beb626",
              "sealed": null,
              "terms": {
                "limit_price": 1800000,
                "quantity": 5,
                "side": "Sell"
              }
            },
            {
              "escrow": 10500000,
              "escrow_asset": "1436ac5e7c42d6cc93304083c0c6c8c964e939aa391c2e0a7304810b7e368f3c",
              …
              "terms": {
                "limit_price": 2100000,
                "quantity": 5,
                "side": "Buy"
              }
            },
            …
          ],
          "status": "Open"
        }
      },
      …
```

Orders are kept sorted by order identifier, so their position says nothing
about who came first.

## Clear the batch

When the batch has sealed, anyone can clear it, by submitting a clearing
with its optimality certificate. `sion market clear` computes both with
the exact solver and submits them:

```text
$ sion market clear solver 45544ef8d600ad29d7830a62685a5b3e68e3e82c2bd1a513dc0ab5e766bc25ab
clearing batch 5: volume 5, price Some(Amount(1.95))
solver signs: Settle batch 5 of market 45544ef8 with a certified-optimal clearing. Fee 0.0012 USD; valid until Pulse 139.
Pulse 121: Done: batch 5 cleared: 5 units at one price 1.95; fee 0.0012 USD.
```

The ledger accepted the clearing only after checking its certificate in
linear time. Five units traded: buyer1's bid at 2.10 met seller1's offer at
1.80. Any price from 1.90 to 2.00 clears the same allocation (buyer2 would
buy above 1.90, seller2 would sell from 2.00), and the canonical price is
the middle of that band, 1.95. The batch now records the result:

```text
          "status": {
            "Cleared": {
              "price": 1950000,
              "solver": "e47206583632d3b320f970883d7b1b0481e50b34d88c05ff4bf650da55d4b412",
              "volume": 5,
              "welfare": 1500000
            }
          }
```

Welfare is the gains from trade: (2.10 − 1.80) × 5 = 1.50 USD.

## Balances

Each side traded at 1.95, and unused escrow came back:

```text
$ sion balance buyer1
account 1cc68e2d9a6321cf44ce4aeb0abacc8a8e5a81f3069f54e5a893258be87ee6c9 (Pulse 121)
  90.2488 USD
  5 7075a799
  3 coins, 0 allotments

$ sion balance seller1
account e795b0225f780c09959eecf67c8aea8be2e2996545436426edf92f92d4beb626 (Pulse 121)
  109.7488 USD
  2 coins, 0 allotments

$ sion balance seller2
account 6c44a52208d9e2acd6081dbad45b5208182a7b5330ca6fc013cee922415712b3 (Pulse 121)
  99.9988 USD
  6 7075a799
  2 coins, 0 allotments
```

buyer1 paid 9.75 USD for five GPU-hours (and its fee); seller1 received
9.75 USD; buyer2 and seller2 did not trade and got their escrow back.

## A batch nobody clears

If no valid clearing arrives within the deadline, the batch is refunded in
the deferred phase:

```text
$ sion market create hank "nobody clears" --batch 2 --deadline 2
hank signs: Open the market "nobody clears" for GPU-hours priced in USD, clearing every 2 Pulses. Fee 0.0012 USD; valid until Pulse 32.
Pulse 14: Done: market ac79de53 opened; fee 0.0012 USD.
$ sion market order hank ac79de5308342817475e508016ca5b1466b5e27079fa462e7c65d22d061f1816 buy 1 0.5
hank signs: Bid for 1 units at at most 0.5 each in market ac79de53. Fee 0.0011 USD; valid until Pulse 34.
Pulse 16: Done: order entered market ac79de53; fee 0.0011 USD.
$ sion events --from 14 --limit 20
Pulse    14  {"MarketCreated":{"market":"ac79de5308342817475e508016ca5b1466b5e27079fa462e7c65d22d061f1816"}}
Pulse    16  {"OrderSubmitted":{"market":"ac79de5308342817475e508016ca5b1466b5e27079fa462e7c65d22d061f1816","order":"7973a25008565c43462da37bbcf03d35d4ab55e90262bdbb4fce142534d4c887"}}
Pulse    17  {"BatchSealed":{"market":"ac79de5308342817475e508016ca5b1466b5e27079fa462e7c65d22d061f1816","batch":8,"orders":1}}
Pulse    19  {"BatchRefunded":{"market":"ac79de5308342817475e508016ca5b1466b5e27079fa462e7c65d22d061f1816","batch":8}}
$ sion market clear hank ac79de5308342817475e508016ca5b1466b5e27079fa462e7c65d22d061f1816
error: no sealed batch to clear
```

## Sealed orders

Markets created with a committee accept sealed orders: encrypted to the
committee with ML-KEM and Shamir sharing, opened only after the batch
seals (`RevealBatch`), so nobody sees an order before the batch closes.
The CLI creates markets without a committee and enters plain orders;
sealed orders are available through the Rust API and exercised by the
scenario and differential tests (`sion-exec/tests/scenarios.rs`,
`sion-spec/tests/differential.rs`).
