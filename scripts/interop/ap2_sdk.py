#!/usr/bin/env python3
"""AP2 against Google's reference SDK (google-agentic-commerce/AP2,
code/sdk/python), in both directions.

    ap2_sdk.py make   <sdk dir> <out.json>   the SDK makes mandates, chains
                                             and receipts for Siontology's
                                             tests to verify
    ap2_sdk.py verify <sdk dir> <in.json>    the SDK verifies the mandates,
                                             chains and receipts Siontology
                                             made; a report is printed

<sdk dir> is the SDK's package root (the directory holding `ap2/`). Run it
isolated (python -I) with the SDK's dependencies installed: cryptography,
jwcrypto, pydantic and sd-jwt, at the versions its pyproject.toml pins.
"""

from __future__ import annotations

import base64
import datetime
import hashlib
import json
import sys
import time


def b64url(data: bytes) -> str:
    return base64.urlsafe_b64encode(data).rstrip(b"=").decode()


def sha256_b64url(text: str) -> str:
    return b64url(hashlib.sha256(text.encode()).digest())


def make(out_path: str) -> None:
    from cryptography import x509
    from cryptography.hazmat.primitives import hashes, serialization
    from cryptography.hazmat.primitives.asymmetric import ec
    from cryptography.x509.oid import NameOID
    from jwcrypto.jwk import JWK

    from ap2.sdk.disclosure_metadata import DisclosureMetadata
    from ap2.sdk.generated.checkout_mandate import CheckoutMandate
    from ap2.sdk.generated.open_checkout_mandate import (
        AllowedMerchants,
        Item as MandateItem,
        LineItemRequirements,
        LineItems,
        OpenCheckoutMandate,
    )
    from ap2.sdk.generated.open_payment_mandate import (
        AgentRecurrence,
        AllowedPayees,
        AmountRange,
        Budget,
        Frequency,
        OpenPaymentMandate,
        PaymentReference,
    )
    from ap2.sdk.generated.payment_mandate import PaymentMandate
    from ap2.sdk.generated.types.amount import Amount
    from ap2.sdk.generated.types.checkout import Checkout
    from ap2.sdk.generated.types.merchant import Merchant
    from ap2.sdk.generated.types.payment_instrument import PaymentInstrument
    from ap2.sdk.jwt_helper import create_jwt
    from ap2.sdk.mandate import MandateClient
    from ap2.sdk.receipt_wrapper import ReceiptClient
    from ap2.sdk.sdjwt import common
    from ap2.sdk.utils import compute_sha256_b64url

    def key(kid: str | None = None) -> JWK:
        jwk = JWK.from_pyca(ec.generate_private_key(ec.SECP256R1()))
        if kid:
            fields = json.loads(jwk.export())
            fields["kid"] = kid
            jwk = JWK.from_json(json.dumps(fields))
        return jwk

    def public(jwk: JWK) -> dict:
        return json.loads(jwk.export_public())

    now = int(time.time())
    issuer, agent, merchant_key = key("sdk-issuer"), key(), key("merchant")
    client = MandateClient()
    merchant = Merchant(id="m-sdk", name="SDK Shop", website="https://shop.example")
    audience, nonce = "m-sdk", "nonce-from-the-merchant"

    # The merchant's checkout, as UCP has it, signed by the merchant.
    checkout = Checkout.model_validate(
        {
            "id": "chk_sdk_1",
            "merchant": merchant.model_dump(exclude_none=True),
            "line_items": [
                {
                    "id": "li_1",
                    "item": {"id": "sku-report", "title": "A research report", "price": 450},
                    "quantity": 1,
                    "totals": [{"type": "subtotal", "amount": 450}, {"type": "total", "amount": 450}],
                }
            ],
            "status": "ready_for_complete",
            "currency": "USD",
            "totals": [{"type": "subtotal", "amount": 450}, {"type": "total", "amount": 450}],
            "links": [{"type": "terms_of_service", "url": "https://shop.example/terms"}],
        }
    )
    checkout_jwt = create_jwt(
        {"alg": "ES256", "typ": "JWT", "kid": "merchant"},
        json.loads(checkout.model_dump_json(exclude_none=True)),
        merchant_key,
    )
    checkout_hash = compute_sha256_b64url(checkout_jwt)

    # The user's open checkout mandate, issued for the agent's key; the
    # agent closes it for this checkout.
    open_checkout = client.create(
        [
            OpenCheckoutMandate(
                constraints=[
                    AllowedMerchants(allowed=[merchant]),
                    LineItems(
                        items=[
                            LineItemRequirements(
                                id="line_1",
                                acceptable_items=[MandateItem(id="sku-report", title="A research report")],
                                quantity=1,
                            )
                        ]
                    ),
                ],
                cnf={"jwk": public(agent)},
                iat=now,
                exp=now + 3600,
            )
        ],
        issuer,
    )
    checkout_chain = client.present(
        agent,
        open_checkout,
        [CheckoutMandate(checkout_jwt=checkout_jwt, checkout_hash=checkout_hash)],
        nonce=nonce,
        aud=audience,
    )
    open_checkout_hash = compute_sha256_b64url(open_checkout)

    # The open payment mandate: recurring, bounded per payment and in all,
    # tied to the open checkout mandate; closed for this checkout's total.
    open_payment = client.create(
        [
            OpenPaymentMandate(
                constraints=[
                    AmountRange(currency="USD", max=1000, min=100),
                    AllowedPayees(allowed=[merchant]),
                    PaymentReference(conditional_transaction_id=open_checkout_hash),
                    # The SDK's issuer cannot serialize its own Frequency
                    # enum (a TypeError in sd-jwt's json.dumps): the
                    # constraint is built without validation, its
                    # frequency the string AP2's schema names.
                    AgentRecurrence.model_construct(
                        type="payment.agent_recurrence",
                        frequency=Frequency.ON_DEMAND.value,
                        max_occurrences=2,
                    ),
                    Budget(max=10.0, currency="USD"),
                ],
                cnf={"jwk": public(agent)},
                iat=now,
                exp=now + 3600,
            )
        ],
        issuer,
    )
    payment_chain = client.present(
        agent,
        open_payment,
        [
            PaymentMandate(
                transaction_id=checkout_hash,
                payee=merchant,
                payment_amount=Amount(amount=450, currency="USD"),
                payment_instrument=PaymentInstrument(id="pi-1", type="sion_transaction"),
            )
        ],
        nonce=nonce,
        aud=audience,
    )

    # A root that names its key by certificate chain: a CA, and the leaf
    # whose key signs the open mandate (the SDK's x5c, base64url).
    def name(common_name: str) -> x509.Name:
        return x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, common_name)])

    ca_key = ec.generate_private_key(ec.SECP256R1())
    leaf_key = ec.generate_private_key(ec.SECP256R1())
    start = datetime.datetime(2026, 1, 1, tzinfo=datetime.timezone.utc)
    end = datetime.datetime(2126, 1, 1, tzinfo=datetime.timezone.utc)
    ca = (
        x509.CertificateBuilder()
        .subject_name(name("SDK interop root"))
        .issuer_name(name("SDK interop root"))
        .public_key(ca_key.public_key())
        .serial_number(x509.random_serial_number())
        .not_valid_before(start)
        .not_valid_after(end)
        .add_extension(x509.BasicConstraints(ca=True, path_length=0), critical=True)
        .add_extension(
            x509.KeyUsage(
                digital_signature=False, content_commitment=False, key_encipherment=False,
                data_encipherment=False, key_agreement=False, key_cert_sign=True,
                crl_sign=True, encipher_only=False, decipher_only=False,
            ),
            critical=True,
        )
        .sign(ca_key, hashes.SHA256())
    )
    leaf = (
        x509.CertificateBuilder()
        .subject_name(name("SDK interop issuer"))
        .issuer_name(ca.subject)
        .public_key(leaf_key.public_key())
        .serial_number(x509.random_serial_number())
        .not_valid_before(start)
        .not_valid_after(end)
        .add_extension(x509.BasicConstraints(ca=False, path_length=None), critical=True)
        .add_extension(
            x509.KeyUsage(
                digital_signature=True, content_commitment=False, key_encipherment=False,
                data_encipherment=False, key_agreement=False, key_cert_sign=False,
                crl_sign=False, encipher_only=False, decipher_only=False,
            ),
            critical=True,
        )
        .sign(ca_key, hashes.SHA256())
    )
    x5c_open = OpenCheckoutMandate(constraints=[], cnf={"jwk": public(agent)}, iat=now, exp=now + 3600)
    claims = common.selectively_disclosable_claims(
        common.delegate_claims_from_model(x5c_open), DisclosureMetadata.from_model(x5c_open)
    )
    x5c_root = common.issue_sd_jwt(
        claims=claims,
        issuer_key=JWK.from_pyca(leaf_key),
        header_params={"x5c": [b64url(leaf.public_bytes(serialization.Encoding.DER))]},
    ).sd_jwt_issuance
    x5c_chain = client.present(
        agent,
        x5c_root,
        [CheckoutMandate(checkout_jwt=checkout_jwt, checkout_hash=checkout_hash)],
        nonce=nonce,
        aud=audience,
    )

    # The merchant's receipts, as the SDK makes them.
    receipts = ReceiptClient()
    checkout_reference = compute_sha256_b64url(client.get_closed_mandate_jwt(checkout_chain))
    payment_reference = compute_sha256_b64url(client.get_closed_mandate_jwt(payment_chain))
    checkout_receipt = receipts.create_checkout_receipt("https://shop.example", checkout_reference, "ord_1")
    closed_payment = PaymentMandate(
        transaction_id=checkout_hash,
        payee=merchant,
        payment_amount=Amount(amount=450, currency="USD"),
        payment_instrument=PaymentInstrument(id="pi-1", type="sion_transaction"),
    )
    payment_receipt = receipts.create_payment_receipt(closed_payment, payment_reference)

    def sign(receipt) -> str:
        return create_jwt(
            {"alg": "ES256", "typ": "JWT"},
            json.loads(receipt.model_dump_json(exclude_none=True)),
            merchant_key,
        )

    vectors = {
        "made_by": "google-agentic-commerce/AP2 code/sdk/python",
        "made_at": now,
        "issuer_jwk": public(issuer),
        "merchant_jwk": public(merchant_key),
        "merchant": merchant.model_dump(exclude_none=True),
        "audience": audience,
        "nonce": nonce,
        "checkout_jwt": checkout_jwt,
        "checkout_hash": checkout_hash,
        "checkout_chain": checkout_chain,
        "open_checkout_hash": open_checkout_hash,
        "payment_chain": payment_chain,
        "payment": {"amount": 450, "currency": "USD"},
        "x5c_root_der": base64.b64encode(ca.public_bytes(serialization.Encoding.DER)).decode(),
        "x5c_chain": x5c_chain,
        "checkout_receipt": sign(checkout_receipt),
        "checkout_reference": checkout_reference,
        "payment_receipt": sign(payment_receipt),
        "payment_reference": payment_reference,
    }
    with open(out_path, "w") as out:
        json.dump(vectors, out, indent=2)
        out.write("\n")
    print(f"wrote {out_path}")


def verify(in_path: str) -> int:
    from cryptography import x509
    from jwcrypto.jwk import JWK

    from ap2.sdk.checkout_mandate_chain import CheckoutMandateChain
    from ap2.sdk.constraints import MandateContext
    from ap2.sdk.mandate import MandateClient
    from ap2.sdk.payment_mandate_chain import PaymentMandateChain
    from ap2.sdk.receipt_wrapper import ReceiptClient
    from ap2.sdk.sdjwt.chain import X5cOrKidPublicKeyProvider
    from ap2.sdk.utils import compute_sha256_b64url

    with open(in_path) as given:
        ours = json.load(given)
    client = MandateClient()
    issuer = JWK.from_json(json.dumps(ours["issuer_jwk"]))
    kid = ours["issuer_jwk"].get("kid")
    root = x509.load_der_x509_certificate(base64.b64decode(ours["x5c_root_der"]))
    keys = X5cOrKidPublicKeyProvider(lambda k: issuer if k == kid else None, trusted_roots=[root])
    merchant_key = JWK.from_json(json.dumps(ours["merchant_jwk"]))
    report: dict = {"verified_by": "google-agentic-commerce/AP2 code/sdk/python", "checks": {}}
    failed = []

    def check(label: str, run, expect_ok: bool = True):
        try:
            outcome = run()
            ok = outcome is True or outcome == [] or (isinstance(outcome, dict) and outcome.get("verified"))
            detail = outcome if not ok else "ok"
        except Exception as error:  # noqa: BLE001 (a refusal is a result here)
            ok, detail = False, f"{type(error).__name__}: {error}"
        report["checks"][label] = {"ok": ok, "expected": expect_ok, "detail": detail}
        if ok != expect_ok:
            failed.append(label)

    def payloads(chain: str):
        return client.verify(chain, keys, expected_aud=ours["audience"], expected_nonce=ours["nonce"],
                             current_time=ours["made_at"])

    check("checkout chain verifies", lambda: bool(payloads(ours["checkout_chain"])))
    check(
        "checkout chain meets its open mandate",
        lambda: CheckoutMandateChain.parse(payloads(ours["checkout_chain"])).verify(
            expected_checkout_hash=ours["checkout_hash"], checkout_jwt=ours["checkout_jwt"]
        ),
    )
    check("payment chain verifies", lambda: bool(payloads(ours["payment_chain"])))
    check(
        "payment chain meets its open mandate (first use)",
        lambda: PaymentMandateChain.parse(payloads(ours["payment_chain"])).verify(
            expected_transaction_id=ours["checkout_hash"],
            expected_open_checkout_hash=ours["open_checkout_hash"],
            mandate_context=MandateContext(),
        ),
    )
    check(
        "payment chain refused past max_occurrences",
        lambda: PaymentMandateChain.parse(payloads(ours["payment_chain"])).verify(
            expected_transaction_id=ours["checkout_hash"],
            expected_open_checkout_hash=ours["open_checkout_hash"],
            mandate_context=MandateContext(total_amount=900, total_uses=2),
        ),
        expect_ok=False,
    )
    check("x5c-rooted chain verifies to the trusted root", lambda: bool(payloads(ours["x5c_chain"])))
    check(
        "x5c-rooted chain refused without the root",
        lambda: bool(client.verify(
            ours["x5c_chain"],
            X5cOrKidPublicKeyProvider(lambda _k: None, trusted_roots=[x509.load_der_x509_certificate(
                base64.b64decode(ours["rogue_root_der"]))]),
            expected_aud=ours["audience"], expected_nonce=ours["nonce"], current_time=ours["made_at"],
        )),
        expect_ok=False,
    )
    receipts = ReceiptClient()
    merchant_public = merchant_key  # the SDK verifies receipts with a JWK
    for kind, is_payment in (("checkout", False), ("payment", True)):
        check(
            f"{kind} receipt verifies",
            lambda kind=kind, is_payment=is_payment: receipts.verify_receipt(
                ours[f"{kind}_receipt"], merchant_public,
                has_reference_in_store_cb=lambda r, kind=kind: r == ours[f"{kind}_reference"],
                is_payment_receipt=is_payment,
            ),
        )
        # The SDK names a closed mandate by the hash of its JWT alone; the
        # specification by sd_hash, the JWT with its disclosures.
        sdk_reference = compute_sha256_b64url(client.get_closed_mandate_jwt(ours[f"{kind}_chain"]))
        report["checks"][f"{kind} receipt reference is the SDK's"] = {
            "ok": sdk_reference == ours[f"{kind}_reference"],
            "expected": "differs: the specification hashes the final SD-JWT as sd_hash does",
            "detail": {"sdk": sdk_reference, "ours": ours[f"{kind}_reference"]},
        }
    report["failed"] = failed
    print(json.dumps(report, indent=2))
    return 1 if failed else 0


def main() -> int:
    if len(sys.argv) != 4 or sys.argv[1] not in ("make", "verify"):
        print(__doc__, file=sys.stderr)
        return 2
    command, sdk, path = sys.argv[1:]
    sys.path.insert(0, sdk)
    if command == "make":
        make(path)
        return 0
    return verify(path)


if __name__ == "__main__":
    sys.exit(main())
