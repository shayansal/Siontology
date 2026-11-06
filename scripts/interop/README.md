# Interoperability checks

These scripts check Siontology's ACP and AP2 code against other people's
work: ACP's published schemas, and AP2's reference SDK. Each one takes the
other party's files from a checkout you make yourself. Nothing here is
downloaded for you. Run the Python with `-I`, so that nothing in the
checkout is imported unless asked for by path.

| Script | Checks against | Status |
| --- | --- | --- |
| `acp_schemas.py` | ACP's JSON Schemas (`spec/2026-04-17/json-schema`) and its webhook OpenAPI file | Run: 34 of 34 samples meet them |
| `ap2_sdk.py` | Google's AP2 reference SDK (`code/sdk/python`), both directions | Not run to the end: no result is claimed |

## ACP's schemas

```sh
git clone https://github.com/agentic-commerce-protocol/agentic-commerce-protocol acp
python3 -m venv env && env/bin/pip install jsonschema referencing pyyaml
SION_INTEROP_OUT=out cargo test -p sion-cli --test interop acp_samples -- --ignored
env/bin/python -I scripts/interop/acp_schemas.py acp/spec/2026-04-17/json-schema out/acp-samples.json
```

The test runs a devnet merchant, a PSP, a 3-D Secure provider, a webhook
receiver and a Feed API, and writes out what they sent and answered: 34
samples. The samples are the discovery documents, a session in each state
(with a ledger handler, a card handler, 3-D Secure metadata and the AP2
extension), the requests a buyer sends, errors, carts, the delegate
payment request and response, each step of an authentication, an order
and its webhook event, and feed products, metadata and listings. At ACP
commit `7fdd78d` (2026-07-17), all 34 meet the schemas, with three notes:

- **Extension fields.** A session's `ap2` field is set aside before it is
  checked. ACP's core schemas are closed (`additionalProperties: false`),
  and an extension adds its fields at the JSONPaths it declares
  (`$.CheckoutSession.ap2`), which the session does.
- **A definition that cannot be met.** `DelegateAuthenticationSessionWithResult`
  is a closed base and the result under `allOf`, so no session with a
  result can meet it as written. It is checked as its base, then its result.
- **A cart example the schema refuses.** ACP's cart example gives each
  item a `quantity`, but its `Item` has none. The merchant takes either
  form; the buyer names an item once for each unit, as the schema has it.

Runs of it found four faults here, now fixed:

- The discovery document named extensions by string instead of by object.
- The 3-D Secure request sent its currency in lower case.
- The issuer's result carried a field the schema does not allow, so the
  notice that it is simulated now goes in `cardholder_info`.
- The ledger instrument carried its signed transaction as
  `credential.transaction`, where ACP requires every credential's `token`.
  Buyers now send `token`, and merchants still take the old name.

## AP2's reference SDK

```sh
git clone https://github.com/google-agentic-commerce/AP2 ap2
python3 -m venv env && env/bin/pip install cryptography==46.0.5 jwcrypto==1.5.6 pydantic==2.12.5 sd-jwt==0.10.4
# The SDK makes mandates, chains and receipts; Siontology verifies them.
env/bin/python -I scripts/interop/ap2_sdk.py make ap2/code/sdk/python out/ap2-sdk.json
SION_INTEROP_AP2_VECTORS=out/ap2-sdk.json cargo test -p sion-cli --test interop ap2_reference_sdk -- --ignored
# Siontology makes them; the SDK verifies them.
SION_INTEROP_OUT=out cargo test -p sion-cli --test interop ap2_vectors_for -- --ignored
env/bin/python -I scripts/interop/ap2_sdk.py verify ap2/code/sdk/python out/ap2-ours.json
```

These were not run to the end, so no result is claimed for them. One run
of `make` got as far as issuing a recurring payment mandate, and found the
first difference below. Reading the SDK at commit `e1ea56d` (2026-04-29)
shows where it and this code part:

- **Recurring mandates.** The SDK cannot issue a mandate with
  `payment.agent_recurrence`: sd-jwt's `json.dumps` fails on the SDK's own
  `Frequency` enum. The script builds that constraint without validation.
- **Receipt references.** The SDK names a closed mandate by the SHA-256
  of its JWT alone. AP2's specification says a receipt's `reference` is
  "calculated in the same manner as `sd_hash`", which covers the final
  SD-JWT with its disclosures. This code follows the specification, so the
  SDK's store lookup would not find its references.
- **Recurrence and budgets.** The SDK checks `max_occurrences` only; this
  code also keeps the frequency's separation between payments, as the
  specification asks. The SDK computes a budget as `int(max * 100)`, which
  truncates some decimal amounts (19.99 becomes 1998); this code is exact.
  Both require `payment.amount_range` and `payment.budget` with a
  recurrence.
- **Certificate chains.** The SDK verifies `x5c` with ECDSA only, and
  checks neither validity periods nor CA constraints. This code checks
  both, and takes RSA and Ed25519 too. The SDK writes `x5c` in base64url,
  where RFC 7515 has base64; this code reads either and writes base64.

## acp-node and Vercel's acp-handler

These were read, not run. acp-node verifies webhooks as
`t=<unix>,v1=<hex HMAC-SHA256(t.body)>`, which is what `Merchant-Signature`
carries, under the header name `ACP-Signature` (ACP's own name is
`Merchant-Signature`). Vercel's handler verifies requests by
`Signature: hex HMAC-SHA256(timestamp.body)`, with `Timestamp` in Unix
seconds: this code's HMAC profile. Both schemes are checked against
OpenSSL's known answers in the unit tests.
