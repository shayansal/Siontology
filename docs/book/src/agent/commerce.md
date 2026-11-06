# Selling and buying: ACP and AP2

An agent's work can be sold to other agents through the checkout they
already speak, and paid for in USD on the chain or by card. Two open
protocols do the talking:

- The **Agentic Commerce Protocol** (ACP, from OpenAI and Stripe) is the
  REST contract a merchant serves so that an agent can buy from it: a
  checkout session made from items, priced, paid and turned into an order.
  Siontology serves it at API version `2026-04-17`: checkout, carts,
  orders and their webhooks, request signing, delegated card payments,
  3-D Secure and product feeds.
- The **Agent Payments Protocol** (AP2, from Google) secures what the
  agent buys: *mandates*, signed by the user's trusted surface, that say
  what the agent may buy and pay. Siontology checks and makes them at
  version 0.2 (of 2026-04-28).

The outputs below are from a run on a local devnet. Card payments there
are simulated: the PSP's acquirer and the 3-D Secure issuer are stand-ins
that move no money and authenticate no one, and say so. What was checked
against other people's implementations is under
[Interoperability](#interoperability).

## Selling

A catalog lists what is sold, at fixed prices in whole cents:

```json
{"offers": [
  {"id": "report", "name": "A research report", "description": "a cited report on one question", "price_usd": "4.50"},
  {"id": "review", "name": "A code review", "description": "a review of one pull request", "price_usd": "2.00"}
]}
```

`sion agent sell serve` serves ACP for it, paid to a wallet. Secrets come
from environment variables, so they stay out of files: buyers' bearer
tokens (`SION_ACP_TOKENS`, `name=token`, comma-separated), the admin
token, the webhook secret and the merchant's token at its PSP. This
merchant also asks buyers to sign their requests, sends webhooks, takes
cards through a PSP with 3-D Secure, and refunds ledger payments from
carol's wallet:

```text
$ sion agent sell serve catalog.json --pay-to carol --name "Research desk" --chain 94425d72… \
    --client-keys client-keys.json --require-signatures \
    --webhook-url http://127.0.0.1:8800/agentic_checkout/webhooks/order_events \
    --card-psp http://127.0.0.1:8797 --card-merchant-id acct_research_desk --three-ds three-ds.json \
    --refunds-from carol --state state.json
selling 2 offer(s) through ACP 2026-04-17 at http://127.0.0.1:8796, paid to 31338b14 on sion:94425d7211b21492c4a9147ba3b67dc0
  report: A research report (4.5 USD)
  review: A code review (2 USD)
cards delegated to the PSP at http://127.0.0.1:8797
3-D Secure asked for
order webhooks to http://127.0.0.1:8800/agentic_checkout/webhooks/order_events
admin API on
ledger refunds from carol
state is kept in state.json; orders are logged to /root/.sion/agents/routes/acp-orders.jsonl
```

`--chain` must be the chain the node serves (`sion status` names it as
`chain`): a merchant pinned to another chain is refused at the start,
since it would be paid on a ledger whose receipts it could never verify.

It serves the discovery document without a token, as ACP has it:

```text
$ curl -s http://127.0.0.1:8796/.well-known/acp.json
{"api_base_url":"http://127.0.0.1:8796","capabilities":{"services":["checkout","orders","carts"],"supported_currencies":["usd"]},"protocol":{"documentation_url":"https://siontology.org/docs/agent/commerce.html","name":"acp","supported_versions":["2026-04-17"],"version":"2026-04-17"},"transports":["rest"]}
```

With a token, it serves:

| Endpoints | What they do |
| --- | --- |
| `POST /checkout_sessions`, `POST /checkout_sessions/{id}`, `GET /checkout_sessions/{id}` | Make a session from items, change its items or buyer, read it |
| `POST /checkout_sessions/{id}/complete`, `…/cancel` | Pay it and make the order, or end it |
| `POST /carts`, `GET /carts/{id}`, `PUT /carts/{id}`, `POST /carts/{id}/cancel` | A cart: items and estimated totals before a checkout, replaced whole |
| `GET /orders/{id}` | An order's page, public as a buyer's link is (its `permalink_url`) |
| `GET /admin/orders`, `POST /admin/orders/{id}/fulfill`, `…/refund`, `GET /admin/outbox` | The operator's, with the admin token: orders, fulfilling and refunding them, and the webhooks waiting or given up |

Every request names `API-Version: 2026-04-17`. Every `POST` carries an
`Idempotency-Key`: the same key with the same body is answered from what
was stored (`Idempotent-Replayed: true`), another body is refused (`422
idempotency_conflict`), a request still running is answered `409` with
`Retry-After`, a `5xx` answer is not kept, and keys are kept 24 hours.
Bodies are compared by their RFC 8785 canonical form. Amounts are integer
cents; errors are ACP's flat `{type, code, message, param}`. A cart names
an item it does not sell in a message rather than dropping it.

### Signed requests

ACP has a client sign its request in `Signature`, with the time in
`Timestamp`, but does not say what is signed. Two profiles are checked:

- **ES256 or Ed25519** over the body's RFC 8785 canonical JSON (so key
  order and spacing do not matter), the signature base64url, the time RFC
  3339 and only checked for freshness;
- **HMAC-SHA256** over `timestamp.body` in hex, the time in Unix seconds:
  what deployed ACP handlers verify (Vercel's `acp-handler`, for one).

Either way the time must be within 300 seconds. `--client-keys` names each
buyer's key, as `{"<buyer's name>": <JWK>}` (EC P-256, OKP Ed25519, or
`oct` for a shared HMAC secret), and `--require-signatures` refuses an
unsigned request from a buyer with a key. `sion agent commerce keygen`
makes a key and prints its public JWK:

```text
$ sion agent commerce keygen buyer.p8
{"crv":"P-256","kty":"EC","x":"9YuSYFMkeJ_ITZrQQZUlECfX6pjH4TSeDkryfAuitCo","y":"UqkhZODtBKANAJy98GthMV_YkQl0685y-hifu4zelug"}
```

The trust atom is `agent:acp-signing`: another implementation's
signatures verify only if it signs the same way, and a signature binds
the body, not the method or the path.

## How a session is paid

ACP leaves the payment method open: a session declares *payment handlers*,
each a named specification, and the buyer's instrument names one. This
merchant declares two.

### On the ledger: `org.siontology.ledger`

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
    "credential": {"type": "signed_transaction", "token": "<hex of the signed transaction>"}
  }
}}
```

Completing the session settles it: the merchant writes down that it is
settling, submits the transaction, waits for it to execute, and verifies
its receipt as a light client does against the chain it pinned. Only then
is the order made. A payment that does not settle leaves the session ready
for payment, with an error message (`payment_declined`) and a fresh quote
to pay against. A payment still settling leaves it `complete_in_progress`,
finished from its receipt when it verifies.

No card token is delegated, because what bounds the agent is on the
ledger: paying under its Agent Charter, it cannot pay anyone or any amount
the charter does not allow. The trust atom is `agent:acp-checkout`. An
order says the merchant was paid, not that it delivered.

### By card: `dev.acp.tokenized.card`

ACP's card handler (version `2026-01-22`, [config
schema](commerce/card-config.schema.json), [instrument
schema](commerce/card-instrument.schema.json)) takes a card the agent
delegated to the merchant's PSP. The instrument carries the PSP's token:

```json
{"payment_data": {
  "handler_id": "card_tokenized",
  "instrument": {"type": "card", "credential": {"type": "spt", "token": "vt_…"}}
}}
```

The merchant charges it one of two ways:

- **Through an ACP PSP** (`--card-psp <url>`, the merchant's token at it
  in `SION_ACP_PSP_TOKEN`): the token is redeemed with `POST
  /agentic_commerce/create_and_process_payment_intent`, the shape of
  NVIDIA's ACP reference (ACP leaves redemption to the PSP).
- **Through Stripe** (`--stripe-key-env <name>`, a secret key): the token
  is a Stripe shared payment token, charged by a PaymentIntent with
  `payment_method_data[shared_payment_granted_token]` and `confirm=true`,
  API version `2026-09-30.preview`, as Stripe documents it for sellers. It
  was built to that documentation and tested against a stand-in for
  Stripe's API, not against Stripe.

A declined card leaves the session ready for payment, with
`payment_declined`. The charge's idempotency key is the session, the token
and the 3-D Secure result, so a retry is answered as the first try was and
charged once. A card payment is the PSP's word (`agent:acp-card`).

`sion agent commerce psp serve` is a PSP's delegate payment API (`POST
/agentic_commerce/delegate_payment`). It checks the card and the allowance
as ACP has it: one use, at most the checkout's total, for one merchant,
until it expires. It answers with a vault token. Each merchant redeems
only the tokens made for it. Card data is kept only in memory, encrypted
with a key made for the process; the CVC goes once the card is
authorized, and a token goes with its card an hour after it expires. Its acquirer is
simulated (`SimulatedAcquirer`): `4000000000000002` is declined,
`4000000000009995` has insufficient funds, `4000000000003220` and
`4000000000003063` want 3-D Secure, and any other valid card is
authorized. No money moves.

### 3-D Secure

With `--three-ds <file>`, the merchant asks for 3-D Secure on card
payments: the file is ACP's `AuthenticationMetadata` (the acquirer's
details, the directory server, a flow preference), shown in each
session's `authentication_metadata`:

```json
{"acquirer_details": {"acquirer_bin": "400000", "acquirer_country": "US", "acquirer_merchant_id": "acct_research_desk", "merchant_name": "Research desk"},
 "directory_server": "visa",
 "flow_preference": {"type": "challenge", "challenge": {"type": "preferred"}}}
```

A card completion without an acceptable `authentication_result`
(`authenticated`, `attempt_acknowledged`, `informational`, or
`not_supported` for a card that is not enrolled) is not charged: the
session becomes `authentication_required`, with a `requires_3ds` message.
The same happens when the issuer asks for authentication the merchant did
not. The result the agent brings is checked against ACP's schema and
passed to the PSP: as its cryptogram, ECI, transaction id and version, to
Stripe.

`sion agent commerce three-ds serve` is ACP's Delegate Authentication
API: `POST /delegate_authentication` makes a session for a card, an amount
and a merchant; `POST /delegate_authentication/{id}/authenticate` takes
the 3DS Method's completion; `GET /delegate_authentication/{id}` gives the
result. Its issuer is simulated (`SimulatedAcs`): it serves its own 3DS
Method and challenge pages, decides by test card number (`4000000000003220`
is challenged, with the code `123456`; `4000000000003063` is
frictionless; `4000000000001091` is rejected), makes a random cryptogram
no network would honour, and says in `cardholder_info` that it was
simulated. Real 3-D Secure takes an EMVCo-certified 3DS server, which is
not built.

## Orders, webhooks, fulfillment and refunds

An order is made for every verified payment, in ACP's shape: line items
with their ordered, current and fulfilled quantities, fulfillments,
adjustments and totals. With `--webhook-url`, each order is sent to the
agent's platform as an `order_create` event, and each change as
`order_update`. The body is signed in `Merchant-Signature:
t=<unix>,v1=<hex HMAC-SHA256(t.body)>` with the secret in
`SION_ACP_WEBHOOK_SECRET`. Deliveries are tried again with backoff, from
5 seconds doubling to an hour, up to twelve tries, and are delivered in
order: an event waiting to be tried again holds back the ones after it.
One given up on is kept a week, for the operator to see.
`sion agent commerce webhooks` receives them, checks each signature
(within 300 seconds), and prints the event:

```text
$ SION_ACP_WEBHOOK_SECRET=… sion agent commerce webhooks
receiving order webhooks at http://127.0.0.1:8800/agentic_checkout/webhooks/order_events
{"type":"order_create","data":{"type":"order","id":"ord_86f2d32b976b60e2b62c108a","checkout_session_id":"cs_ecc5b362a73f9014dce37e06","status":"confirmed"}}
{"type":"order_create","data":{"type":"order","id":"ord_68c5877bfdd7a1485def1925","checkout_session_id":"cs_e7807f6b38d012b3a6d389a2","status":"confirmed"}}
{"type":"order_update","data":{"type":"order","id":"ord_86f2d32b976b60e2b62c108a","checkout_session_id":"cs_ecc5b362a73f9014dce37e06","status":"completed"}}
{"type":"order_update","data":{"type":"order","id":"ord_86f2d32b976b60e2b62c108a","checkout_session_id":"cs_ecc5b362a73f9014dce37e06","status":"completed"}}
(each event carries the whole order; shortened here)
```

The operator fulfills and refunds through the admin API, with the token in
`SION_ACP_ADMIN_TOKEN`:

```text
$ sion agent sell orders http://127.0.0.1:8796
ord_68c5877bfdd7a1485def1925  confirmed  4.50 USD  session cs_e7807f6b38d012b3a6d389a2
ord_86f2d32b976b60e2b62c108a  confirmed  6.50 USD  session cs_ecc5b362a73f9014dce37e06
$ sion agent sell fulfill http://127.0.0.1:8796 ord_86f2d32b976b60e2b62c108a --access-url https://shop.example/reports/1 --description "the report"
order ord_86f2d32b976b60e2b62c108a is completed
$ sion agent sell refund http://127.0.0.1:8796 ord_86f2d32b976b60e2b62c108a --cents 150 --reason "partly late"
refunded 150 cents of order ord_86f2d32b976b60e2b62c108a: refunded on Siontology in transaction ea13ba5fc6ff1f4b88e8c2d44efab713b67117152ae52908d650ed34014d167d
$ sion agent sell refund http://127.0.0.1:8796 ord_68c5877bfdd7a1485def1925
error: unexpected answer: the merchant refused (502): this PSP's API has no refunds (ACP does not define one); refund it at the PSP
```

A ledger payment is refunded by a transfer back to the payer from the
`--refunds-from` wallet; a Stripe payment by a Stripe refund. An ACP PSP
has no refund API (ACP defines none), so such an order is refunded at the
PSP itself. Each refund is an adjustment on the order, and its total
`amount_refunded`; no more than was paid is refunded.

## Kept across restarts

The merchant keeps its state in a file (`--state`, by default
`acp-merchant-state.json` in the configuration directory; mode 0600,
replaced atomically). The file holds sessions, carts, orders, idempotency
keys, the payments made under each AP2 mandate, and the webhook outbox:

```text
$ # the merchant stopped, and started again with the same --state
$ sion agent sell orders http://127.0.0.1:8796
ord_68c5877bfdd7a1485def1925  confirmed  4.50 USD  session cs_e7807f6b38d012b3a6d389a2
ord_86f2d32b976b60e2b62c108a  completed  6.50 USD  session cs_ecc5b362a73f9014dce37e06
```

A restart takes back each open session's quote, so a payment signed
before it still settles. A ledger payment that was settling when the
merchant stopped is finished from its verified receipt once the merchant
is back, or let go if the ledger refused it, or never saw it within five
minutes. A card payment that was interrupted is payable again; its charge
key is the same, so it is charged once. The PSP and the 3-D Secure
provider keep nothing across a restart, so no card data is written down.

## Product feeds

ACP's Feed API is hosted by the agent: merchants publish their products to
it before an agent checks out. `sion agent commerce feeds serve` is that
API (`POST /feeds`, `GET /feeds/{id}`, `GET` and `PATCH
/feeds/{id}/products`), its products checked against ACP's feed schema and
kept across restarts. `sion agent sell feed` publishes a catalog to one,
each offer a product whose one variant is the checkout item id, as ACP
asks, or writes ACP's offline snapshot (`metadata.json`,
`products.jsonl`):

```text
$ SION_ACP_FEED_TOKENS=shop=… sion agent commerce feeds --state feeds.json &
$ SION_ACP_FEED_TOKEN=… sion agent sell feed catalog.json --name "Research desk" --public-url http://127.0.0.1:8796 \
    --to http://127.0.0.1:8799 --country US
published 2 product(s) to feed feed_1de1a2e3d1141bc077e23acc at http://127.0.0.1:8799
$ sion agent sell feed catalog.json --name "Research desk" --public-url http://127.0.0.1:8796 --export feed-snapshot
wrote feed feed_c14aa6a9ab77447132baa8c7 (2 product(s)) to feed-snapshot
```

## Buying

`sion agent buy` is the buyer. On the ledger it makes a session, checks
that the quote is the session's total, that the chain is the one it pinned
and that the total is within its cap, pays, and completes. With
`--sign-key` (PKCS#8, P-256 or Ed25519) or `--sign-secret-env` (HMAC) it
signs every request:

```text
$ sion agent buy http://127.0.0.1:8796 report review --max-usd 10 --wallet alice --chain 94425d72… --sign-key buyer.p8
bought report, review for 6.50 USD: order ord_86f2d32b976b60e2b62c108a (http://127.0.0.1:8796/orders/ord_86f2d32b976b60e2b62c108a)
$ sion agent buy http://127.0.0.1:8796 report review --max-usd 10 --wallet alice --chain 94425d72…
error: unexpected answer: the merchant did not make a session (401): this client signs its requests: Signature and Timestamp are required
$ sion agent buy http://127.0.0.1:8796 report report report --max-usd 10 --wallet alice --chain 94425d72… --sign-key buyer.p8
error: the checkout costs 13.5 USD, more than the 10 USD allowed
```

By card, it delegates the card (`--card`, ACP's `PaymentMethodCard` as
JSON, sent to the PSP and never kept) to the PSP for this checkout only,
authenticates it with 3-D Secure when the merchant or the issuer asks
(`--three-ds`), and completes the session with the PSP's token. It meets a
challenge only from the simulated issuer (`--simulated-otp`): a real
issuer's challenge is a page for the buyer to answer.

The card here is one of the simulated issuer's test cards, the one it
challenges:

```json
{"type": "card", "card_number_type": "fpan", "number": "4000000000003220", "exp_month": "12", "exp_year": "2030",
 "name": "Ada Lovelace", "cvc": "123", "display_card_funding_type": "credit", "display_brand": "visa",
 "display_last4": "3220", "metadata": {}}
```

```text
$ sion agent buy http://127.0.0.1:8796 report --max-usd 10 --sign-key buyer.p8 --card card.json \
    --psp http://127.0.0.1:8797 --three-ds http://127.0.0.1:8798 --simulated-otp 123456
bought report for 4.50 USD: order ord_68c5877bfdd7a1485def1925 (http://127.0.0.1:8796/orders/ord_68c5877bfdd7a1485def1925)
$ sion agent buy http://127.0.0.1:8796 report --max-usd 10 --sign-key buyer.p8 --card card.json --psp http://127.0.0.1:8797
error: the merchant asks for 3-D Secure: name an authentication provider
```

The merchant's balance went from 100,000 USD to 100,006.5 USD: the one ledger
payment, for the order made; the card order moved no money on the
ledger. With `--charter` and `--allotment` it pays as an agent under its
charter.
A checkout over the cap is canceled, not paid. The library has the parts
on their own (`sion_cli::acp::buyer`): `checkout`, the cart calls,
`delegate_card`, `authenticate_card`, and the request bodies.

## AP2 mandates

With `--ap2-trust <jwks.json>` (keys by `kid`) or `--ap2-roots
<roots.pem>` (root certificates), the merchant also takes AP2's mandates,
from the trusted surfaces those keys or certificates stand for (AP2's
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
`payment.reference`, `payment.execution_date`, `payment.budget` and
`payment.agent_recurrence`. A constraint not known here fails, as AP2
requires.

A root names its key by `kid` or by an `x5c` certificate chain, the
signer's certificate first, to a root the merchant trusts (sent or left
out). The chain is checked as RFC 5280 has it: each certificate valid
now, each signed by the next, every issuer a CA within its path length,
the signer's key usage allowing signatures, and no critical extension
unknown here. Signatures may be ECDSA P-256 or P-384, RSA of 2048 to 8192
bits, or Ed25519; the signer's own key is P-256, for ES256. Revocation is
not checked.

A budget and a recurrence are counted from the payments this merchant
recorded under the same open payment mandate, kept with its state. A
budget's `max` is in major units, as AP2's example and SDK read it
(`10.00` is 1,000 cents), and is compared exactly. A recurrence allows a
payment once a period of its `frequency` has passed since the last, in
calendar months for `MONTHLY`, `QUARTERLY` and `ANNUALLY`, and no more
than `max_occurrences`. As AP2's reference SDK requires, a recurring
mandate must also carry `payment.amount_range` and `payment.budget`. The
mandates are counted under one lock as they are checked, so two
completions under one mandate are counted one after the other; a payment
that fails gives its count back. An open checkout mandate makes one
order, unless the payment mandate with it recurs: AP2's line items cannot
be split across checkouts. The trust atom is `agent:ap2-mandate`.

A refused mandate leaves the session ready for payment with an error
message, and its error receipt in the session; nothing was paid.

The shopping agent's side is in the library (`sion_cli::ap2`): `issue`
and `issue_x5c` make a root mandate as a trusted surface would, `close` is
the agent's closing hop over it, `chain` joins them, and
`verify_checkout`, `verify_payment` and `receipt` are the merchant's and
the payment side's.

## Interoperability

What was checked against other people's work, at the commits named in
[`scripts/interop/README.md`](https://github.com/ColdAI-org/siontology/blob/main/scripts/interop/README.md):

- **ACP's published JSON Schemas** (2026-04-17): 34 requests and
  answers from a live run meet them. They cover discovery documents,
  sessions in each state, the buyer's requests, errors, carts, delegate
  payment, each step of an authentication, an order and its webhook event,
  and feed products and listings. The AP2 extension's field is set aside
  first, as ACP's extension mechanism declares it. Runs of it found four
  faults here, now fixed (the ledger instrument's credential, for one, is
  now carried as ACP's `token`), and two faults in ACP's schemas: a definition no
  session with a result can meet, and a cart example the cart schema
  refuses.
- **AP2's specification**: its encoded example tokens verify here, and so
  do this crate's test certificates through every path check.
- **RFC 8785** and **HMAC**: the RFC's examples canonicalize as it says,
  and the webhook and request HMACs match OpenSSL's answers.
- **AP2's reference SDK, acp-node and Vercel's acp-handler** were read;
  only the SDK was run, and not to the end. The harness for the SDK, in
  both directions, is in `scripts/interop/`, and no result is claimed for
  it. Reading them shows where they part
  from this code. The SDK names a receipt's mandate by the hash of its
  JWT alone, where AP2's specification hashes it with its disclosures, as
  `sd_hash` does; this code follows the specification. The SDK counts a
  recurrence's occurrences but not its periods, truncates some decimal
  budgets, and checks neither certificates' validity nor their CA
  constraints. acp-node names the webhook header `ACP-Signature`, where
  ACP's is `Merchant-Signature`.

## Not built

OpenID4VP delegation from a wallet's user credential; wallet handlers other
than the ledger; ACP's discounts and fulfillment other than digital; a
refund API at the PSP; certificate revocation; and real card networks: the
PSP's acquirer and the 3-D Secure issuer here are simulated.
