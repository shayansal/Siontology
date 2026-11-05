//! AP2's own examples, checked: the encoded tokens of the AP2 v0.2
//! specification's checkout and payment mandate pages (Apache-2.0, from
//! google-agentic-commerce/AP2 at e1ea56d), and mandates made here, round
//! trip through the same checks.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde_json::{json, Value};
use sion_cli::ap2::{self, Expect, Payment, PublicKey, Refusal, SigningKey, Token, Trust};

fn vector(name: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/tests/vectors/ap2/{name}.txt",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
    .trim()
    .to_string()
}

fn disclosed(token: &Token) -> Vec<Value> {
    token
        .disclosures
        .iter()
        .map(|d| serde_json::from_slice(&URL_SAFE_NO_PAD.decode(d).unwrap()).unwrap())
        .collect()
}

/// The agent's key an open mandate names (`cnf.jwk`).
fn cnf(open: &Token) -> PublicKey {
    let mandate = disclosed(open)
        .into_iter()
        .find_map(|d| d[1].get("cnf").cloned())
        .unwrap();
    PublicKey::from_jwk(&mandate["jwk"]).unwrap()
}

#[test]
fn the_specifications_checkout_chain_binds_as_ap2_says() {
    let open = Token::parse(&vector("open-checkout")).unwrap();
    let closed = Token::parse(&vector("closed-checkout")).unwrap();
    // A chain is its tokens joined by ~.
    let chain = vector("checkout-chain");
    assert_eq!(
        ap2::split(&chain).unwrap(),
        vec![open.clone(), closed.clone()]
    );
    assert_eq!(ap2::chain(&[&open.text(), &closed.text()]), chain);

    // The closing hop is signed with the key the open mandate names, and
    // bound to the open token by its hash.
    let (header, payload) = ap2::verify_jws(&closed.jwt, &cnf(&open)).unwrap();
    assert_eq!(header["typ"], "kb+sd-jwt");
    assert_eq!(payload["sd_hash"], open.sd_hash().unwrap().as_str());
    assert_eq!(payload["aud"], "merchant");
    assert!(ap2::verify_jws(&closed.jwt, &SigningKey::generate().public()).is_err());

    // checkout_hash is the hash of the checkout JWT it carries.
    let disclosures = disclosed(&closed);
    let checkout_jwt = disclosures
        .iter()
        .find(|d| d.as_array().unwrap().len() == 3 && d[1] == "checkout_jwt")
        .map(|d| d[2].as_str().unwrap().to_string())
        .unwrap();
    let mandate = disclosures
        .iter()
        .find(|d| d.as_array().unwrap().len() == 2)
        .map(|d| d[1].clone())
        .unwrap();
    assert_eq!(mandate["vct"], ap2::CHECKOUT);
    assert_eq!(mandate["checkout_hash"], ap2::hash(&checkout_jwt).as_str());

    // Its root is signed by an agent provider whose key the specification
    // does not give: the chain is refused as untrusted, not as malformed.
    let refused = ap2::verify_chain(
        &chain,
        &Trust::new(),
        Expect {
            audience: "merchant",
            nonce: "b9c8d7e6f5a4b3c2d1e0f9a8b7c6d5e4",
            now: 1_777_342_400,
        },
    )
    .unwrap_err();
    assert!(
        matches!(&refused, Refusal::InvalidCredential(why) if why.contains("agent-provider-key-1")),
        "{refused}"
    );
}

#[test]
fn the_specifications_payment_chain_names_its_checkout() {
    let open_checkout = Token::parse(&vector("open-checkout")).unwrap();
    let closed_checkout = Token::parse(&vector("closed-checkout")).unwrap();
    let open = Token::parse(&vector("open-payment")).unwrap();
    let closed = Token::parse(&vector("closed-payment")).unwrap();
    let (_, payload) = ap2::verify_jws(&closed.jwt, &cnf(&open)).unwrap();
    assert_eq!(payload["sd_hash"], open.sd_hash().unwrap().as_str());
    let mandate = disclosed(&closed)[0][1].clone();
    assert_eq!(mandate["vct"], ap2::PAYMENT);
    // transaction_id is the checkout's hash, as the closed checkout states.
    let checkout = disclosed(&closed_checkout)
        .into_iter()
        .find(|d| d[1]["vct"] == ap2::CHECKOUT)
        .unwrap();
    assert_eq!(mandate["transaction_id"], checkout[1]["checkout_hash"]);
    // payment.reference names the open checkout mandate by its hash.
    let open_mandate = disclosed(&open)
        .into_iter()
        .find(|d| d[1]["vct"] == ap2::PAYMENT_OPEN)
        .unwrap();
    let reference = open_mandate[1]["constraints"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["type"] == "payment.reference")
        .unwrap();
    assert_eq!(
        reference["conditional_transaction_id"],
        open_checkout.sd_hash().unwrap().as_str()
    );
    assert_eq!(
        ap2::split(&vector("payment-chain")).unwrap(),
        vec![open, closed]
    );
}

/// A provider, an agent, a merchant, and the merchant's checkout.
struct Setting {
    provider: SigningKey,
    agent: SigningKey,
    merchant_key: SigningKey,
    merchant: Value,
    checkout_jwt: String,
}

fn setting() -> Setting {
    let merchant_key = SigningKey::generate();
    let merchant = json!({"id": "m-1", "name": "Shop", "website": "https://shop.example"});
    let checkout_jwt = ap2::sign_jws(
        &json!({"typ": "JWT"}),
        &json!({
            "id": "cs_1",
            "merchant": merchant,
            "line_items": [{"id": "li_0", "item": {"id": "BAB1234", "title": "Red", "price": 1999}, "quantity": 1}],
            "currency": "USD",
            "totals": [{"type": "total", "amount": 1999}],
        }),
        &merchant_key,
    );
    Setting {
        provider: SigningKey::generate(),
        agent: SigningKey::generate(),
        merchant_key,
        merchant,
        checkout_jwt,
    }
}

const NOW: u64 = 1_800_000_000;

fn expect() -> Expect<'static> {
    Expect {
        audience: "m-1",
        nonce: "n-1",
        now: NOW,
    }
}

impl Setting {
    fn trust(&self) -> Trust {
        Trust::new().with("p", self.provider.public())
    }

    fn open_checkout(&self, constraints: Value) -> String {
        ap2::issue(
            &json!({
                "vct": ap2::CHECKOUT_OPEN,
                "constraints": constraints,
                "cnf": {"jwk": self.agent.public().jwk()},
                "iat": NOW - 10,
                "exp": NOW + 3600,
            }),
            &self.provider,
            "p",
        )
    }

    fn closed_checkout(&self, open: &str, nonce: &str) -> String {
        let closed = ap2::close(
            open,
            &json!({
                "vct": ap2::CHECKOUT,
                "checkout_jwt": self.checkout_jwt,
                "checkout_hash": ap2::hash(&self.checkout_jwt),
            }),
            &self.agent,
            "m-1",
            nonce,
            NOW,
        )
        .unwrap();
        ap2::chain(&[open, &closed])
    }

    fn verify(&self, chain: &str) -> Result<ap2::CheckoutApproved, Refusal> {
        ap2::verify_checkout(
            chain,
            &self.trust(),
            expect(),
            &self.checkout_jwt,
            &self.merchant_key.public(),
        )
    }
}

#[test]
fn a_closed_checkout_is_approved_only_under_its_open_mandates_constraints() {
    let s = setting();
    let allowing = |item: &str| {
        json!([
            {"type": "checkout.allowed_merchants", "allowed": [{"name": "Shop", "website": "https://shop.example"}]},
            {"type": "checkout.line_items", "items": [{"id": "l", "quantity": 1, "acceptable_items": [{"id": item, "title": item}]}]},
        ])
    };
    let open = s.open_checkout(allowing("BAB1234"));
    let approved = s.verify(&s.closed_checkout(&open, "n-1")).unwrap();
    assert_eq!(approved.checkout_hash, ap2::hash(&s.checkout_jwt));
    assert_eq!(
        approved.open,
        vec![Token::parse(&open).unwrap().sd_hash().unwrap()]
    );
    assert_eq!(approved.checkout["id"], "cs_1");

    // Another item: verified, but not approved.
    let other = s.open_checkout(allowing("FAF1234"));
    assert!(matches!(
        s.verify(&s.closed_checkout(&other, "n-1")),
        Err(Refusal::InvalidMandate(_))
    ));
    // An unknown constraint fails, as AP2 requires.
    let unknown = s.open_checkout(json!([{"type": "checkout.weather", "sunny": true}]));
    assert!(matches!(
        s.verify(&s.closed_checkout(&unknown, "n-1")),
        Err(Refusal::UnresolvedConstraint(_))
    ));
    // Closed for another challenge.
    assert!(matches!(
        s.verify(&s.closed_checkout(&open, "n-2")),
        Err(Refusal::InvalidCredential(_))
    ));
    // Expired.
    let late = ap2::verify_checkout(
        &s.closed_checkout(&open, "n-1"),
        &s.trust(),
        Expect {
            now: NOW + 3600 + ap2::SKEW + 1,
            ..expect()
        },
        &s.checkout_jwt,
        &s.merchant_key.public(),
    );
    assert!(matches!(late, Err(Refusal::InvalidCredential(why)) if why.contains("expired")));
    // Closed by a key the open mandate does not name.
    let thief = SigningKey::generate();
    let stolen = ap2::close(
        &open,
        &json!({"vct": ap2::CHECKOUT, "checkout_jwt": s.checkout_jwt, "checkout_hash": ap2::hash(&s.checkout_jwt)}),
        &thief,
        "m-1",
        "n-1",
        NOW,
    )
    .unwrap();
    assert!(matches!(
        s.verify(&ap2::chain(&[&open, &stolen])),
        Err(Refusal::InvalidCredential(_))
    ));
    // A root no one trusts.
    let rogue = ap2::issue(
        &json!({"vct": ap2::CHECKOUT_OPEN, "cnf": {"jwk": s.agent.public().jwk()}}),
        &SigningKey::generate(),
        "p",
    );
    assert!(s.verify(&s.closed_checkout(&rogue, "n-1")).is_err());
    // A checkout the merchant did not sign, though the hash is right.
    let forged = ap2::sign_jws(&json!({}), &json!({"id": "cs_1"}), &SigningKey::generate());
    let closed = ap2::close(
        &open,
        &json!({"vct": ap2::CHECKOUT, "checkout_jwt": forged, "checkout_hash": ap2::hash(&forged)}),
        &s.agent,
        "m-1",
        "n-1",
        NOW,
    )
    .unwrap();
    assert!(ap2::verify_checkout(
        &ap2::chain(&[&open, &closed]),
        &s.trust(),
        expect(),
        &forged,
        &s.merchant_key.public()
    )
    .is_err());

    // With the user there, the provider signs the closed mandate itself.
    let direct = ap2::issue(
        &json!({"vct": ap2::CHECKOUT, "checkout_jwt": s.checkout_jwt, "checkout_hash": ap2::hash(&s.checkout_jwt)}),
        &s.provider,
        "p",
    );
    assert!(s.verify(&direct).is_ok());
}

#[test]
fn a_payment_mandate_approves_one_amount_to_one_payee_for_one_checkout() {
    let s = setting();
    let open_checkout = s.open_checkout(json!([]));
    let checkout = s.verify(&s.closed_checkout(&open_checkout, "n-1")).unwrap();
    let open = |constraints: Value| {
        ap2::issue(
            &json!({
                "vct": ap2::PAYMENT_OPEN,
                "constraints": constraints,
                "cnf": {"jwk": s.agent.public().jwk()},
                "exp": NOW + 3600,
            }),
            &s.provider,
            "p",
        )
    };
    let closed = |open: &str, amount: u64| {
        let closing = ap2::close(
            open,
            &json!({
                "vct": ap2::PAYMENT,
                "transaction_id": checkout.checkout_hash,
                "payee": s.merchant,
                "payment_amount": {"amount": amount, "currency": "USD"},
                "payment_instrument": {"id": "pi-1", "type": "sion_transaction"},
                "execution_date": "2027-01-15T12:00:00Z",
            }),
            &s.agent,
            "m-1",
            "n-1",
            NOW,
        )
        .unwrap();
        ap2::chain(&[open, &closing])
    };
    let payment = |amount: u64, spent: Option<u64>| Payment {
        checkout_hash: &checkout.checkout_hash,
        payee: &s.merchant,
        amount,
        currency: "usd",
        checkout_open: &checkout.open,
        spent_before: spent,
    };
    let check = |chain: &str, amount: u64, spent: Option<u64>| {
        ap2::verify_payment(chain, &s.trust(), expect(), &payment(amount, spent))
    };
    let reference = Token::parse(&open_checkout).unwrap().sd_hash().unwrap();
    let constraints = json!([
        {"type": "payment.amount_range", "currency": "USD", "min": 100, "max": 5000},
        {"type": "payment.allowed_payees", "allowed": [{"id": "m-1"}]},
        {"type": "payment.allowed_payment_instruments", "allowed": [{"type": "sion_transaction"}]},
        {"type": "payment.reference", "conditional_transaction_id": reference},
        {"type": "payment.execution_date", "not_before": "2027-01-01", "not_after": "2027-01-31T23:59:59Z"},
        {"type": "payment.budget", "currency": "USD", "max": 6000},
    ]);
    let mandate = open(constraints.clone());
    let approved = check(&closed(&mandate, 1999), 1999, Some(0)).unwrap();
    assert_eq!(
        approved.open,
        Some(Token::parse(&mandate).unwrap().sd_hash().unwrap())
    );
    // The amount it approves, and no other.
    assert!(matches!(
        check(&closed(&mandate, 1999), 2000, Some(0)),
        Err(Refusal::InvalidMandate(_))
    ));
    // Over the range.
    assert!(matches!(
        check(&closed(&mandate, 9999), 9999, Some(0)),
        Err(Refusal::InvalidMandate(_))
    ));
    // Over the budget, with what was spent before.
    assert!(matches!(
        check(&closed(&mandate, 1999), 1999, Some(4500)),
        Err(Refusal::InvalidMandate(_))
    ));
    // A budget without a history is not taken on trust.
    assert!(matches!(
        check(&closed(&mandate, 1999), 1999, None),
        Err(Refusal::UnresolvedConstraint(_))
    ));
    // Recurrence is not tracked here, so it is unresolved.
    let recurring = open(json!([{"type": "payment.agent_recurrence", "frequency": "MONTHLY"}]));
    assert!(matches!(
        check(&closed(&recurring, 1999), 1999, Some(0)),
        Err(Refusal::UnresolvedConstraint(_))
    ));
    // Another payee is not paid.
    let elsewhere = json!({"id": "m-2", "name": "Other"});
    let to_other = ap2::verify_payment(
        &closed(&mandate, 1999),
        &s.trust(),
        expect(),
        &Payment {
            payee: &elsewhere,
            ..payment(1999, Some(0))
        },
    );
    assert!(matches!(to_other, Err(Refusal::InvalidMandate(_))));
    // Outside its dates.
    let january = open(json!([{"type": "payment.execution_date", "not_after": "2026-12-31"}]));
    assert!(matches!(
        check(&closed(&january, 1999), 1999, Some(0)),
        Err(Refusal::InvalidMandate(_))
    ));
}

#[test]
fn a_receipt_names_the_mandate_it_answers() {
    let key = SigningKey::generate();
    let success = ap2::receipt(
        ap2::Kind::Checkout,
        &key,
        "https://shop.example",
        "ref-1",
        Ok(&json!({"order_id": "ord_1"})),
        NOW,
    );
    let (_, claims) = ap2::verify_jws(&success, &key.public()).unwrap();
    assert_eq!(claims["status"], "Success");
    assert_eq!(claims["reference"], "ref-1");
    assert_eq!(claims["order_id"], "ord_1");
    let refusal = Refusal::UnresolvedConstraint("not known".into());
    let error = ap2::receipt(
        ap2::Kind::Payment,
        &key,
        "https://shop.example",
        "ref-2",
        Err(&refusal),
        NOW,
    );
    let (_, claims) = ap2::verify_jws(&error, &key.public()).unwrap();
    assert_eq!(claims["status"], "Error");
    assert_eq!(claims["error"], "unresolved_constraint");
}
