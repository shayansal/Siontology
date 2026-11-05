# Selling and buying: ACP and AP2

An agent's work can be sold to other agents through the checkout they
already speak, and paid for in USD on the chain. Two open protocols do
the talking:

- The **Agentic Commerce Protocol** (ACP, from OpenAI and Stripe) is the
  REST contract a merchant serves so that an agent can buy from it: a
  checkout session made from items, priced, paid and turned into an order.
  Siontology serves it at API version `2026-04-17`.
- The **Agent Payments Protocol** (AP2, from Google) secures what the
  agent buys: *mandates*, signed by the user's trusted surface, that say
  what the agent may buy and pay. Siontology checks and makes them at
  version 0.2 (of 2026-04-28).

The outputs below are from a run on a local devnet. Nothing here was run
against another implementation of either protocol.

## Selling

A catalog lists what is sold, at fixed prices in whole cents:

```json
{"offers": [
  {"id": "report", "name": "A research report", "description": "a cited report on one question", "price_usd": "4.50"},
  {"id": "review", "name": "A code review", "description": "a review of one pull request", "price_usd": "2.00"}
]}
```

`sion agent sell serve` serves ACP's checkout for it, paid to a wallet.
Buyers are given bearer tokens, read from an environment variable so they
stay out of files:

```text
$ SION_ACP_TOKENS=buyer-token-0123456789 sion agent sell serve catalog.json \
    --pay-to carol --name "Research desk" --chain 94425d72…
selling 2 offer(s) through ACP 2026-04-17 at http://127.0.0.1:8796, paid to 31338b14 on sion:94425d7211b21492c4a9147ba3b67dc0
  report: A research report (4.5 USD)
  review: A code review (2 USD)
orders are written to /root/.sion/agents/routes/acp-orders.jsonl
```

It serves the discovery document without a token, as ACP has it:

```text
$ curl -s http://127.0.0.1:8796/.well-known/acp.json
{"api_base_url":"http://127.0.0.1:8796","capabilities":{"services":["checkout"],"supported_currencies":["usd"]},"protocol":{"documentation_url":"https://siontology.org/docs/agent/commerce.html","name":"acp","supported_versions":["2026-04-17"],"version":"2026-04-17"},"transports":["rest"]}
```

and the five checkout endpoints with a token: `POST /checkout_sessions`,
`POST /checkout_sessions/{id}` (change the items or the buyer), `GET
/checkout_sessions/{id}`, `POST …/complete` and `POST …/cancel`. Every
request names `API-Version: 2026-04-17`; every `POST` carries an
`Idempotency-Key`, and the same key with the same body is answered with
the stored answer (`Idempotent-Replayed: true`), with another body is
refused (`422 idempotency_conflict`). Amounts are integer cents, errors
are ACP's flat `{type, code, message, param}`. Each order also has a page
at `/orders/{id}`, linked from the order as its `permalink_url`.

## How a session is paid

ACP leaves the payment method open: a session declares *payment handlers*,
each a named specification, and the buyer's instrument names one. The
merchant declares one handler of Siontology's own:

| Field | Value |
| --- | --- |
| `name` | `org.siontology.ledger` |
| `version` | `2026-11-05` |
| `requires_delegate_payment` | `false` |
| `psp` | `siontology` |
| `config_schema` | [`config.schema.json`](commerce/config.schema.json) |
| `instrument_schemas` | [`instrument.schema.json`](commerce/instrument.schema.json) |

Its `config.requirements` is an [x402](../guides/x402.md) quote for
the session's total (in micro-USD: the cents times 10,000), with a fresh
nonce, payable to the merchant on the chain named. The buyer signs a
transaction against it and does not submit it, and puts it in the
instrument:

```json
{"payment_data": {
  "handler_id": "sion_ledger",
  "instrument": {
    "handler_id": "sion_ledger",
    "type": "sion_transaction",
    "credential": {"type": "signed_transaction", "transaction": "<hex of the signed transaction>"}
  }
}}
```

Completing the session settles it: the merchant submits the transaction,
waits for it to execute, and verifies its receipt as a light client does
against the chain it pinned. Only then is the order made. A payment that
does not settle leaves the session ready for payment, with an error
message (`payment_declined`) and a fresh quote to pay against.

No card token is delegated, because what bounds the agent is on the
ledger: paying under its Agent Charter, it cannot pay anyone or any amount
the charter does not allow. The trust atom is `agent:acp-checkout`. An
order says the merchant was paid, not that it delivered.

Cards are not taken: the handler is the only one served, so an agent that
knows only ACP's card handlers (`dev.acp.tokenized.card` through a PSP)
cannot pay this merchant.

## Buying

`sion agent buy` is the buyer: it makes a session, checks that the quote
is the session's total, that the chain is the one it pinned and that the
total is within its cap, pays, and completes:

```text
$ SION_ACP_TOKEN=buyer-token-0123456789 sion agent buy http://127.0.0.1:8796 report review \
    --max-usd 10 --wallet alice --chain 94425d72…
bought report, review for 6.50 USD: order ord_1_676e7c51440b (http://127.0.0.1:8796/orders/ord_1_676e7c51440b)
$ SION_ACP_TOKEN=buyer-token-0123456789 sion agent buy http://127.0.0.1:8796 report report report \
    --max-usd 10 --wallet alice --chain 94425d72…
error: the checkout costs 13.5 USD, more than the 10 USD allowed
```

The merchant's balance went from 100,001 to 100,007.5 USD: one payment,
for the order made. With `--charter` and `--allotment` it pays as an agent
under its charter. A checkout over the cap is canceled, not paid.

## AP2 mandates

With `--ap2-trust <jwks.json>`, the merchant also takes AP2's mandates,
from the trusted surfaces whose keys the file lists by `kid` (AP2's
Trusted Agent Provider model). With `--require-mandate`, a session
completes only with one. They travel in an ACP extension of Siontology's
own, `org.siontology.ap2` ([schema](commerce/ap2.schema.json)):

- Each session carries `ap2.checkout_jwt`, the checkout the merchant
  signed (in UCP's shape: its id, the merchant, the line items and the
  totals), an `audience` and a `nonce`. The nonce changes whenever the
  session is priced again.
- The completion carries `ap2.checkout_mandate` and, if the buyer has one,
  `ap2.payment_mandate`: Delegate SD-JWT chains, the user's open mandate
  and the agent's closing of it for this checkout.
- The merchant checks them before any money moves, and answers with
  signed receipts in `ap2.checkout_receipt` and `ap2.payment_receipt`,
  each naming the mandate it answers by its hash.

What is checked: every signature (ES256), every disclosure (RFC 9901), and
each hop's binding to the one before; the closing hop's `aud` and `nonce`;
`iat` and `exp`; that the mandate carries exactly the checkout this
merchant signed, with its hash; and that the open mandate's constraints
hold: `checkout.allowed_merchants`, `checkout.line_items` (matched as a
flow, as AP2 describes), `payment.amount_range`, `payment.allowed_payees`,
`payment.allowed_payment_instruments`, `payment.allowed_pisps`,
`payment.reference`, `payment.execution_date`, and `payment.budget`
against what was paid under the same open mandate. A constraint not known
here fails, as AP2 requires, and so does `payment.agent_recurrence`,
which is not tracked. An open checkout mandate makes one order: AP2's line
items cannot be split across checkouts, so the merchant refuses a second
order under it. The trust atom is `agent:ap2-mandate`.

A refused mandate leaves the session ready for payment with an error
message, and its error receipt in the session; nothing was paid.

The shopping agent's side is in the library (`sion_cli::ap2`): `issue`
makes a root mandate as a trusted surface would, `close` is the agent's
closing hop over it, `chain` joins them, and `verify_checkout`,
`verify_payment` and `receipt` are the merchant's and the payment side's.
They are checked against the encoded tokens of AP2's own specification,
whose signatures and hash bindings verify here.

Not built: an `x5c` certificate chain as a root's key, OpenID4VP
delegation from a wallet's user credential, and ACP's request signing,
webhooks, delegated payment and authentication, 3-D Secure, and the feed
and cart APIs. Sessions are kept in memory and go when the merchant stops;
orders are written down.
