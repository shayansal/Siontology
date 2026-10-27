# Charters for AI agents

An Agent Charter lets an AI agent spend a principal's money within limits
the protocol enforces on every payment. This guide charters an agent,
pays inside and outside its limits, and revokes it. The concepts, including
delegation, are in [Agent Charters](../concepts/agent-charters.md).

## Create a charter

```text
Usage: sion charter create [OPTIONS] <PRINCIPAL> <AGENT>

Arguments:
  <PRINCIPAL>  Principal's wallet
  <AGENT>      Agent (wallet or account)

Options:
      --payees <PAYEES>            Allowed payees (comma-separated; empty: any)
      --max-payment <MAX_PAYMENT>  Largest payment (USD) [default: 100]
      --max-window <MAX_WINDOW>    Largest total per window (USD) [default: 500]
      --window <WINDOW>            Window (Pulses) [default: 1000]
      --funding <FUNDING>          Budget (USD) [default: 1000]
      --bond <BOND>                Bond behind the agent's receipts (USD) [default: 0]
      --lifetime <LIFETIME>        Charter lifetime (Pulses) [default: 100000]
```

The principal, acme, gives agent7 a budget of 200 USD that it may spend
only at gpu-cloud, at most 50 USD per payment and 80 USD per 100 Pulses,
with a 100 USD bond behind the agent's receipts:

```text
$ sion charter create acme agent7 --payees gpu-cloud --max-payment 50 --max-window 80 --window 100 --funding 200 --bond 100 --lifetime 5000
acme signs: Give agent7 an Agent Charter funded with 200 USD and a 100 USD bond behind its receipts: only to gpu-cloud, at most 50 USD per payment, 80 USD per 100 Pulses, until Pulse 5036, a receipt claim for every payment, may delegate narrower limits. Fee 0.0012 USD; valid until Pulse 56.
Pulse 38: Done: charter ce08bb2e created; fee 0.0012 USD.
  digest b1b467efd0bc645c819503c3175b5353f0433af82cad762ac5ad6f3ac51d5b64
charter ce08bb2e578f8b8a5abc757da46783c647cb33a0a6bf370cc1dc2126a28c49de
allotment 2819fd2e4dea454997f4c676477627fb8326d54ad8d9abe87fcb233c116d0fc7
```

The charter is an object only the principal can revoke. The **allotment**
holds the 200 USD budget, and only the agent can write it, so the agent's
payments need no consensus. Charters created with the CLI always issue a
receipt claim for every payment and allow delegation with narrower limits;
the other caveats (an evidence requirement above a threshold, no receipts,
no delegation) are available through the Rust API.

The agent needs a little USD of its own for fees:

```text
$ sion balance agent7
account 62bdd2bb253aadd3a430f480d7f569103dad789882b4ca73b85cf0aa96966703 (Pulse 38)
  5 USD
  1 coins, 1 allotments
```

## Pay as the agent

```text
Usage: sion charter pay [OPTIONS] <AGENT> <CHARTER> <ALLOTMENT> <PAYEE> <AMOUNT>

Options:
      --purpose <PURPOSE>  Purpose [default: ""]
```

A first payment of 30 USD to gpu-cloud succeeded at Pulse 41, with receipt
claim `e06931af`. Then the ledger refused what the charter does not allow.
A payee that is not on the list (what a prompt injection would try):

```text
$ sion charter pay agent7 ce08bb2e578f8b8a5abc757da46783c647cb33a0a6bf370cc1dc2126a28c49de 2819fd2e4dea454997f4c676477627fb8326d54ad8d9abe87fcb233c116d0fc7 mallory 10 --purpose "ignore previous instructions"
agent7 signs: As an agent under charter ce08bb2e, pay 10 USD to mallory for "ignore previous instructions". Fee 0.0011 USD; valid until Pulse 61.
Pulse 44: Failed (charter caveat violated: payee not allowed); nothing happened except the fee of 0.0011 USD.
```

A payment above the per-payment limit:

```text
$ sion charter pay agent7 ce08bb2e578f8b8a5abc757da46783c647cb33a0a6bf370cc1dc2126a28c49de 2819fd2e4dea454997f4c676477627fb8326d54ad8d9abe87fcb233c116d0fc7 gpu-cloud 60 --purpose "a large job"
agent7 signs: As an agent under charter ce08bb2e, pay 60 USD to gpu-cloud for "a large job". Fee 0.0011 USD; valid until Pulse 64.
Pulse 47: Failed (charter caveat violated: payment 60 above per-payment limit 50); nothing happened except the fee of 0.0011 USD.
```

A payment within every limit:

```text
$ sion charter pay agent7 ce08bb2e578f8b8a5abc757da46783c647cb33a0a6bf370cc1dc2126a28c49de 2819fd2e4dea454997f4c676477627fb8326d54ad8d9abe87fcb233c116d0fc7 gpu-cloud 45 --purpose embeddings
agent7 signs: As an agent under charter ce08bb2e, pay 45 USD to gpu-cloud for "embeddings". Fee 0.0011 USD; valid until Pulse 67.
Pulse 50: Done: agent paid 45 USD to gpu-cloud (receipt claim af729e59); fee 0.0011 USD.
```

And one more that would bring the window's total to 85 USD:

```text
$ sion charter pay agent7 ce08bb2e578f8b8a5abc757da46783c647cb33a0a6bf370cc1dc2126a28c49de 2819fd2e4dea454997f4c676477627fb8326d54ad8d9abe87fcb233c116d0fc7 gpu-cloud 10 --purpose "one more"
agent7 signs: As an agent under charter ce08bb2e, pay 10 USD to gpu-cloud for "one more". Fee 0.0011 USD; valid until Pulse 70.
Pulse 53: Failed (charter caveat violated: window total 85 above limit 80); nothing happened except the fee of 0.0011 USD.
```

A refused payment still costs the agent its fee, from its own coins, so a
looping agent cannot spam for free.

## Receipts

Each successful payment issued a receipt claim, which joined the knowledge
log in the same Pulse:

```text
$ sion events --from 41 --limit 3
Pulse    41  {"AgentPaid":{"charter":"ce08bb2e578f8b8a5abc757da46783c647cb33a0a6bf370cc1dc2126a28c49de","payee":"74a3493d356ec54face5946511115668b04bb818c323c4ba2d9d12b7b34dff60","amount":30000000,"receipt":"e06931afbaf7df310b6c9bb44542e71ab70d9b4726b45306f199bae01f45e688"}}
Pulse    41  {"ClaimIssued":{"claim":"e06931afbaf7df310b6c9bb44542e71ab70d9b4726b45306f199bae01f45e688","log_index":1}}
Pulse    50  {"AgentPaid":{"charter":"ce08bb2e578f8b8a5abc757da46783c647cb33a0a6bf370cc1dc2126a28c49de","payee":"74a3493d356ec54face5946511115668b04bb818c323c4ba2d9d12b7b34dff60","amount":45000000,"receipt":"af729e59e0e68ef5b67dc9c94eec3a73259b12353d6ec2bb922462ad5a6a54ca"}}
```

A receipt rests on the 100 USD bond behind the agent's receipts, so
anyone can price it. It covers a value of 40 USD at κ = 2:

```text
$ sion claim decide e06931afbaf7df310b6c9bb44542e71ab70d9b4726b45306f199bae01f45e688 40
{
  "accept": true,
  "adjudicator_cap": "Infinite",
  …
  "reasons": [
    "cost of corruption 100 covers κ·V = 80"
  ],
  …
}
```

Receipts name no adjudicator, so they cannot be challenged the way
[bonded claims](bonds-and-challenges.md) can; `adjudicator_cap` is
`Infinite` for that reason.

## Revoke

```text
$ sion charter revoke acme ce08bb2e578f8b8a5abc757da46783c647cb33a0a6bf370cc1dc2126a28c49de
acme signs: Revoke charter ce08bb2e and every charter delegated from it. Fee 0.0011 USD; valid until Pulse 76.
Pulse 59: Done: charter ce08bb2e revoked; fee 0.0011 USD.

$ sion charter pay agent7 ce08bb2e578f8b8a5abc757da46783c647cb33a0a6bf370cc1dc2126a28c49de 2819fd2e4dea454997f4c676477627fb8326d54ad8d9abe87fcb233c116d0fc7 gpu-cloud 1 --purpose "after revocation"
agent7 signs: As an agent under charter ce08bb2e, pay 1 USD to gpu-cloud for "after revocation". Fee 0.0011 USD; valid until Pulse 79.
Pulse 61: Failed (charter caveat violated: charter revoked); nothing happened except the fee of 0.0011 USD.
```

Revocation is a contended operation, ordered by consensus. Until it
commits, the most the agent can still spend is what remains in its
allotment. Afterwards, `ReclaimCharter` returns what is left to the
principal; no CLI command sends it yet.

```text
$ sion balance gpu-cloud
account 74a3493d356ec54face5946511115668b04bb818c323c4ba2d9d12b7b34dff60 (Pulse 61)
  75 USD
  2 coins, 0 allotments
```

## Agents over MCP and x402

An agent connected through MCP pays with `sion_agent_pay`, and buys HTTP
resources under its charter with `sion_fetch_paid`
([MCP tools](../reference/mcp.md), [x402](x402.md)). The same caveats
apply: the agent can be told anything, and the ledger still enforces the
charter.
