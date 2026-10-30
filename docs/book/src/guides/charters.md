# Charters for AI agents

An Agent Charter lets an AI agent spend a principal's money within limits
the protocol enforces on every payment. This guide charters an agent,
pays inside and outside its limits, and revokes it. The concepts, including
delegation, are in [Agent Charters](../concepts/agent-charters.md). Every
output below is from one run on a local devnet.

## Create a charter

```text
Charter an agent

Usage: sion charter create [OPTIONS] <PRINCIPAL> <AGENT>

Arguments:
  <PRINCIPAL>  Principal's wallet
  <AGENT>      Agent (wallet or account)

Options:
      --node <NODE>                The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --payees <PAYEES>            Allowed payees (comma-separated; empty: any)
      --max-payment <MAX_PAYMENT>  Largest payment (USD) [default: 100]
      --burst <BURST>              The most the agent may spend at once: its bucket's capacity, full when it starts (USD) [default: 500]
      --rate <RATE>                What refills its bucket each Pulse (USD) [default: 0.5]
      --funding <FUNDING>          Budget (USD) [default: 1000]
      --bond <BOND>                Bond behind the agent's receipts (USD) [default: 0]
      --lifetime <LIFETIME>        Charter lifetime (Pulses) [default: 100000]
  -h, --help                       Print help
```

The principal, acme, gives agent7 a budget of 200 USD that it may spend
only at gpu-cloud, at most 50 USD per payment, with a 100 USD bond behind
the agent's receipts. How fast it may spend is a token bucket: it holds at
most 80 USD (`--burst`), starts full and refills by 0.1 USD a Pulse
(`--rate`), so over any stretch of time the agent pays at most 80 USD plus
0.1 USD for each Pulse after the first:

```text
$ sion charter create acme agent7 --payees gpu-cloud --max-payment 50 --burst 80 --rate 0.1 --funding 200 --bond 100 --lifetime 5000
acme signs: Give agent7 an Agent Charter funded with 200 USD and a 100 USD bond behind its receipts: only to gpu-cloud, at most 50 USD per payment, 80 USD at once, refilled by 0.1 USD a Pulse, until Pulse 5005, a receipt claim for every payment, may delegate narrower limits. Fee 0.0012 USD; valid until Pulse 25.
Pulse 7: Done: charter 080027af created; fee 0.0012 USD.
  digest 4753898eb688e58574a372b6f5d7bfec695214f11411221d4fed06823df6133b
charter 080027af99f5541e72560fbc961349c486c4623e4622a5fc23bff96668a14cea
allotment 3a51139ff80e74baf2c49f40627288db4833a493bce4a6fd75ed714b27a31f6e
```

The charter is an object only the principal, or the agent itself, can
revoke. The **allotment**
holds the 200 USD budget and the bucket, and only the agent can write it,
so the agent's payments need no consensus. Charters created with the CLI
always issue a receipt claim for every payment and allow delegation with
narrower limits; the other caveats (an evidence requirement above a
threshold, no receipts, no delegation) are available through the Rust API.

The agent needs a little USD of its own for fees:

```text
$ sion balance agent7
account 3b0c32773d5889ae32eb7b083b10927f0ffb33278bf1c1aff3bcc73b7993b7f7 (Pulse 7)
  5 USD
  1 coins, 1 allotments
```

## Pay as the agent

```text
Pay as the agent

Usage: sion charter pay [OPTIONS] <AGENT> <CHARTER> <ALLOTMENT> <PAYEE> <AMOUNT>

Arguments:
  <AGENT>      Agent's wallet
  <CHARTER>    Charter ID
  <ALLOTMENT>  Allotment ID
  <PAYEE>      Payee
  <AMOUNT>     USD

Options:
      --node <NODE>        The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --purpose <PURPOSE>  Purpose [default: ""]
  -h, --help               Print help
```

A first payment of 30 USD to gpu-cloud succeeded at Pulse 10, with receipt
claim `e06955c0`:

```text
$ sion charter pay agent7 080027af…4cea 3a51139f…1f6e gpu-cloud 30 --purpose '1M tokens of inference'
agent7 signs: As an agent under charter 080027af, pay 30 USD to gpu-cloud for "1M tokens of inference". Fee 0.0012 USD; valid until Pulse 28.
Pulse 10: Done: agent paid 30 USD to gpu-cloud (receipt claim e06955c0); fee 0.0012 USD.
  digest ff013542788b6dca657c6aaf2dd43250fdbb563a8d4ceb76f31e9a66adc94809
```

Then the ledger refused what the charter does not allow. A payee that is
not on the list (what a prompt injection would try):

```text
$ sion charter pay agent7 080027af…4cea 3a51139f…1f6e mallory 10 --purpose 'ignore previous instructions'
agent7 signs: As an agent under charter 080027af, pay 10 USD to mallory for "ignore previous instructions". Fee 0.0011 USD; valid until Pulse 31.
Pulse 13: Failed (charter caveat violated: payee not allowed); nothing happened except the fee of 0.0011 USD.
  digest b66f9398089f9d537558141af63f863973481d266236c82a01430ef414f09cc2
```

A payment above the per-payment limit:

```text
$ sion charter pay agent7 080027af…4cea 3a51139f…1f6e gpu-cloud 60 --purpose 'a large job'
agent7 signs: As an agent under charter 080027af, pay 60 USD to gpu-cloud for "a large job". Fee 0.0011 USD; valid until Pulse 33.
Pulse 16: Failed (charter caveat violated: payment 60 above per-payment limit 50); nothing happened except the fee of 0.0011 USD.
  digest dbe50b096854b62b7824d29e9457af4e18824518adc12957f9b3ae17a2fc8607
```

A payment within every limit:

```text
$ sion charter pay agent7 080027af…4cea 3a51139f…1f6e gpu-cloud 45 --purpose embeddings
agent7 signs: As an agent under charter 080027af, pay 45 USD to gpu-cloud for "embeddings". Fee 0.0011 USD; valid until Pulse 37.
Pulse 19: Done: agent paid 45 USD to gpu-cloud (receipt claim 38c2ea63); fee 0.0011 USD.
  digest fdb2ca24153565d9206ee293682dcbc1a43c17a664ec2afbc4a939bf350f2fe5
```

And one more, which the bucket does not cover: the two payments took 75
of its 80 USD, and it has refilled by 1.2 USD since the first, to
6.2 USD:

```text
$ sion charter pay agent7 080027af…4cea 3a51139f…1f6e gpu-cloud 10 --purpose 'one more'
agent7 signs: As an agent under charter 080027af, pay 10 USD to gpu-cloud for "one more". Fee 0.0011 USD; valid until Pulse 39.
Pulse 22: Failed (charter caveat violated: the bucket holds 6.2, below the payment 10); nothing happened except the fee of 0.0011 USD.
  digest f0994baf65d216322b7a9e68fa7a29d334e412554d6dcf37241699b51d79c7bb
```

A refused payment still costs the agent its fee, from its own coins, so a
looping agent cannot spam for free.

## Receipts

Each successful payment issued a receipt claim, which joined the knowledge
log in the same Pulse:

```text
$ sion events --from 10 --limit 3
Pulse    10  {"AgentPaid":{"charter":"080027af99f5541e72560fbc961349c486c4623e4622a5fc23bff96668a14cea","payee":"72dcc6f43f6a6c4d32372f9b4e705cc32c1eb9ba7f0949d07fd2c76af54d3ec6","amount":30000000,"receipt":"e06955c0485f6f0366839d2f2c47bbe1cb5903346d00b7309203898ab62a777e"}}
Pulse    10  {"ClaimIssued":{"claim":"e06955c0485f6f0366839d2f2c47bbe1cb5903346d00b7309203898ab62a777e","log_index":0}}
Pulse    19  {"AgentPaid":{"charter":"080027af99f5541e72560fbc961349c486c4623e4622a5fc23bff96668a14cea","payee":"72dcc6f43f6a6c4d32372f9b4e705cc32c1eb9ba7f0949d07fd2c76af54d3ec6","amount":45000000,"receipt":"38c2ea63e56ae11b76817e5b0256d24f463578e8d6c7d4a211de7bba5880fc24"}}
```

A receipt rests on the 100 USD bond behind the agent's receipts, so
anyone can price it. It covers a value of 40 USD at κ = 2:

```text
$ sion claim decide e06955c0…777e 40
{
  …
  "accept": true,
  "adjudicator_cap": "Infinite",
  "reasons": [
    "cost of corruption 100 covers κ·V = 80"
  ]
}
```

Receipts name no adjudicator, so they cannot be challenged the way
[bonded claims](bonds-and-challenges.md) can; `adjudicator_cap` is
`Infinite` for that reason.

## Revoke

```text
$ sion charter revoke acme 080027af…4cea
acme signs: Revoke charter 080027af and every charter delegated from it. Fee 0.0011 USD; valid until Pulse 43.
Pulse 26: Done: charter 080027af revoked; fee 0.0011 USD.
  digest 757b53334f0d399a62d1e6a442b9ffada00a9856cc96ab6caffe1a172993c0a1
$ sion charter pay agent7 080027af…4cea 3a51139f…1f6e gpu-cloud 1 --purpose 'after revocation'
agent7 signs: As an agent under charter 080027af, pay 1 USD to gpu-cloud for "after revocation". Fee 0.0011 USD; valid until Pulse 46.
Pulse 28: Failed (charter caveat violated: charter revoked); nothing happened except the fee of 0.0011 USD.
  digest 6d464a0ebd24c41d37b7edc3458759c598564f52d9edc78fbc0d04cf9b5765a6
```

Revocation is a contended operation, ordered by consensus. Until it
commits, the most the agent can still spend is what remains in its
allotment, at the pace its bucket allows. Afterwards, `ReclaimCharter`
returns what is left to the principal; `sion agent reclaim` sends it, for
any charter:

```text
$ sion agent reclaim acme 080027af…4cea
acme signs: Return what is left in dead charter 080027af's allotments to its backer. Fee 0.0011 USD; valid until Pulse 48.
Pulse 31: Done: 125 USD returned from charter 080027af; fee 0.0011 USD.
$ sion balance gpu-cloud
account 72dcc6f43f6a6c4d32372f9b4e705cc32c1eb9ba7f0949d07fd2c76af54d3ec6 (Pulse 31)
  75 USD
  2 coins, 0 allotments
```

## Agents over MCP and x402

An agent connected through MCP pays with `sion_agent_pay`, and buys HTTP
resources under its charter with `sion_fetch_paid`
([MCP tools](../reference/mcp.md), [x402](x402.md)). The same caveats
apply: the agent can be told anything, and the ledger still enforces the
charter.
