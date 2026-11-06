//! AP2's own examples, checked: the encoded tokens of the AP2 v0.2
//! specification's checkout and payment mandate pages (Apache-2.0, from
//! google-agentic-commerce/AP2 at e1ea56d), and mandates made here, round
//! trip through the same checks.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde_json::{json, Value};
use sion_cli::ap2::{self, Expect, Paid, Payment, PublicKey, Refusal, SigningKey, Token, Trust};

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
    let check = |chain: &str, amount: u64, history: Option<&[Paid]>| {
        ap2::verify_payment(
            chain,
            &s.trust(),
            expect(),
            &Payment {
                checkout_hash: &checkout.checkout_hash,
                payee: &s.merchant,
                amount,
                currency: "usd",
                checkout_open: &checkout.open,
                history,
                now: NOW,
            },
        )
    };
    let reference = Token::parse(&open_checkout).unwrap().sd_hash().unwrap();
    let constraints = json!([
        {"type": "payment.amount_range", "currency": "USD", "min": 100, "max": 5000},
        {"type": "payment.allowed_payees", "allowed": [{"id": "m-1"}]},
        {"type": "payment.allowed_payment_instruments", "allowed": [{"id": "pi-1"}]},
        {"type": "payment.reference", "conditional_transaction_id": reference},
        {"type": "payment.execution_date", "not_before": "2027-01-01", "not_after": "2027-01-31T23:59:59Z"},
        // A budget is in major units, as AP2's example and SDK have it.
        {"type": "payment.budget", "currency": "USD", "max": 60.00},
    ]);
    let mandate = open(constraints.clone());
    let none: &[Paid] = &[];
    let approved = check(&closed(&mandate, 1999), 1999, Some(none)).unwrap();
    assert_eq!(
        approved.open,
        Some(Token::parse(&mandate).unwrap().sd_hash().unwrap())
    );
    // The amount it approves, and no other.
    assert!(matches!(
        check(&closed(&mandate, 1999), 2000, Some(none)),
        Err(Refusal::InvalidMandate(_))
    ));
    // Over the range.
    assert!(matches!(
        check(&closed(&mandate, 9999), 9999, Some(none)),
        Err(Refusal::InvalidMandate(_))
    ));
    // Over the budget, with what was paid before: 45.00 and 19.99 is more
    // than 60.00; 40.00 and 19.99 is not.
    let day = 24 * 3600;
    let before = [Paid {
        at: NOW - day,
        amount: 4500,
    }];
    assert!(matches!(
        check(&closed(&mandate, 1999), 1999, Some(&before)),
        Err(Refusal::InvalidMandate(_))
    ));
    let before = [Paid {
        at: NOW - day,
        amount: 4000,
    }];
    assert!(check(&closed(&mandate, 1999), 1999, Some(&before)).is_ok());
    // A budget without a history is not taken on trust.
    assert!(matches!(
        check(&closed(&mandate, 1999), 1999, None),
        Err(Refusal::UnresolvedConstraint(_))
    ));
    // Another payee is not paid.
    let elsewhere = json!({"id": "m-2", "name": "Other"});
    let to_other = ap2::verify_payment(
        &closed(&mandate, 1999),
        &s.trust(),
        expect(),
        &Payment {
            checkout_hash: &checkout.checkout_hash,
            payee: &elsewhere,
            amount: 1999,
            currency: "usd",
            checkout_open: &checkout.open,
            history: Some(none),
            now: NOW,
        },
    );
    assert!(matches!(to_other, Err(Refusal::InvalidMandate(_))));
    // Outside its dates.
    let january = open(json!([{"type": "payment.execution_date", "not_after": "2026-12-31"}]));
    assert!(matches!(
        check(&closed(&january, 1999), 1999, Some(none)),
        Err(Refusal::InvalidMandate(_))
    ));
}

#[test]
fn a_recurring_payment_mandate_is_used_as_often_as_it_says() {
    let s = setting();
    let open_checkout = s.open_checkout(json!([]));
    let checkout = s.verify(&s.closed_checkout(&open_checkout, "n-1")).unwrap();
    let open = |constraints: Value| {
        ap2::issue(
            &json!({
                "vct": ap2::PAYMENT_OPEN,
                "constraints": constraints,
                "cnf": {"jwk": s.agent.public().jwk()},
                "exp": NOW + 400 * 24 * 3600,
            }),
            &s.provider,
            "p",
        )
    };
    let closed = |open: &str| {
        let closing = ap2::close(
            open,
            &json!({
                "vct": ap2::PAYMENT,
                "transaction_id": checkout.checkout_hash,
                "payee": s.merchant,
                "payment_amount": {"amount": 999, "currency": "USD"},
                "payment_instrument": {"id": "pi-1", "type": "sion_transaction"},
            }),
            &s.agent,
            "m-1",
            "n-1",
            NOW,
        )
        .unwrap();
        ap2::chain(&[open, &closing])
    };
    let at = |now: u64, history: Option<&[Paid]>, chain: &str| {
        ap2::verify_payment(
            chain,
            &s.trust(),
            Expect {
                audience: "m-1",
                nonce: "n-1",
                now: NOW,
            },
            &Payment {
                checkout_hash: &checkout.checkout_hash,
                payee: &s.merchant,
                amount: 999,
                currency: "USD",
                checkout_open: &checkout.open,
                history,
                now,
            },
        )
    };
    // Recurring, bounded per payment and in all.
    let recurring = |recurrence: Value| {
        closed(&open(json!([
            recurrence,
            {"type": "payment.amount_range", "currency": "USD", "max": 1000},
            {"type": "payment.budget", "currency": "USD", "max": 100},
        ])))
    };
    let monthly = recurring(
        json!({"type": "payment.agent_recurrence", "frequency": "MONTHLY", "max_occurrences": 3}),
    );
    let day = 24 * 3600;
    let none: &[Paid] = &[];
    // The first payment, and one a calendar month after the last.
    assert!(at(NOW, Some(none), &monthly).is_ok());
    let once = [Paid {
        at: NOW - 31 * day,
        amount: 999,
    }];
    assert!(at(NOW, Some(&once), &monthly).is_ok());
    // Not sooner than a month after the last.
    let lately = [Paid {
        at: NOW - 10 * day,
        amount: 999,
    }];
    assert!(matches!(
        at(NOW, Some(&lately), &monthly),
        Err(Refusal::InvalidMandate(_))
    ));
    // Not more often than it says, however long ago.
    let thrice = [
        Paid {
            at: NOW - 100 * day,
            amount: 999,
        },
        Paid {
            at: NOW - 70 * day,
            amount: 999,
        },
        Paid {
            at: NOW - 40 * day,
            amount: 999,
        },
    ];
    let said = at(NOW, Some(&thrice), &monthly).unwrap_err();
    assert!(said.to_string().contains("allows 3 payments"), "{said}");
    // Recurrence is not taken on trust: without a history, or with a
    // frequency not known here, it is unresolved.
    assert!(matches!(
        at(NOW, None, &monthly),
        Err(Refusal::UnresolvedConstraint(_))
    ));
    let fortnightly =
        recurring(json!({"type": "payment.agent_recurrence", "frequency": "FORTNIGHTLY"}));
    assert!(matches!(
        at(NOW, Some(none), &fortnightly),
        Err(Refusal::UnresolvedConstraint(_))
    ));
    // Weekly, and on demand.
    let weekly = recurring(json!({"type": "payment.agent_recurrence", "frequency": "WEEKLY"}));
    let last = [Paid {
        at: NOW - 6 * day,
        amount: 999,
    }];
    assert!(at(NOW, Some(&last), &weekly).is_err());
    assert!(at(NOW + day, Some(&last), &weekly).is_ok());
    let any_time = recurring(json!({"type": "payment.agent_recurrence", "frequency": "ON_DEMAND"}));
    assert!(at(NOW, Some(&last), &any_time).is_ok());
    // Recurring without a bound per payment, or in all, is not taken.
    let unbounded = closed(&open(json!([
        {"type": "payment.agent_recurrence", "frequency": "ON_DEMAND"},
        {"type": "payment.budget", "currency": "USD", "max": 100},
    ])));
    let said = at(NOW, Some(none), &unbounded).unwrap_err();
    assert!(
        said.to_string().contains("requires payment.amount_range"),
        "{said}"
    );
}

#[test]
fn recurrence_counts_calendar_months_and_budgets_count_exactly() {
    // 2027-01-31 plus a month is the end of February; plus a quarter is the
    // end of April; a year after a leap day is the 28th of February.
    let jan31 = ap2::seconds_of("2027-01-31T12:00:00Z").unwrap() as u64;
    assert_eq!(
        ap2::next_allowed(jan31, "MONTHLY"),
        Some(ap2::seconds_of("2027-02-28T12:00:00Z").unwrap() as u64)
    );
    assert_eq!(
        ap2::next_allowed(jan31, "QUARTERLY"),
        Some(ap2::seconds_of("2027-04-30T12:00:00Z").unwrap() as u64)
    );
    let leap = ap2::seconds_of("2028-02-29T00:00:00Z").unwrap() as u64;
    assert_eq!(
        ap2::next_allowed(leap, "ANNUALLY"),
        Some(ap2::seconds_of("2029-02-28T00:00:00Z").unwrap() as u64)
    );
    assert_eq!(ap2::next_allowed(jan31, "DAILY"), Some(jan31 + 24 * 3600));
    assert_eq!(
        ap2::next_allowed(jan31, "BIWEEKLY"),
        Some(jan31 + 14 * 24 * 3600)
    );
    assert_eq!(ap2::next_allowed(jan31, "HOURLY"), None);
    // Major units to minor, exactly: no float is involved.
    assert_eq!(ap2::minor_units(&json!("1000.00"), "USD"), Some(100_000));
    assert_eq!(ap2::minor_units(&json!(19.99), "USD"), Some(1999));
    assert_eq!(ap2::minor_units(&json!(500), "JPY"), Some(500));
    assert_eq!(ap2::minor_units(&json!("0.001"), "USD"), None);
    assert_eq!(ap2::minor_units(&json!("1.5"), "JPY"), None);
    assert_eq!(ap2::minor_units(&json!("-1"), "USD"), None);
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

fn x5c_vector(name: &str) -> Vec<u8> {
    std::fs::read(format!(
        "{}/tests/vectors/x5c/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

fn pem(der: &[u8]) -> String {
    use base64::engine::general_purpose::STANDARD;
    let body = STANDARD.encode(der);
    let lines: Vec<&str> = body
        .as_bytes()
        .chunks(64)
        .map(|c| std::str::from_utf8(c).unwrap())
        .collect();
    format!(
        "-----BEGIN CERTIFICATE-----\n{}\n-----END CERTIFICATE-----\n",
        lines.join("\n")
    )
}

#[test]
fn a_root_mandate_is_trusted_through_its_certificate_chain() {
    // Test certificates (made for these tests, valid 2025 to 2125): a P-384
    // root, a P-256 intermediate, a P-256 leaf whose key signs the root
    // mandate; a rogue root; an intermediate that is not a CA; an expired
    // leaf; an RSA root.
    let s = setting();
    let root = x5c_vector("root.der");
    let inter = x5c_vector("inter.der");
    let leaf = x5c_vector("leaf.der");
    let leaf_key = SigningKey::from_pkcs8(&x5c_vector("leaf.p8")).unwrap();
    let open_by = |key: &SigningKey, chain: &[Vec<u8>]| {
        ap2::issue_x5c(
            &json!({
                "vct": ap2::CHECKOUT_OPEN,
                "constraints": [],
                "cnf": {"jwk": s.agent.public().jwk()},
                "iat": NOW - 10,
                "exp": NOW + 3600,
            }),
            key,
            chain,
        )
    };
    let verify = |open: &str, trust: &Trust| {
        ap2::verify_checkout(
            &s.closed_checkout(open, "n-1"),
            trust,
            expect(),
            &s.checkout_jwt,
            &s.merchant_key.public(),
        )
    };
    let trust = Trust::new().with_root(root.clone());

    // Leaf, then intermediate, to the trusted root (left out, as RFC 5280
    // allows), or with the root sent too.
    let open = open_by(&leaf_key, &[leaf.clone(), inter.clone()]);
    assert!(verify(&open, &trust).is_ok());
    let with_root = open_by(&leaf_key, &[leaf.clone(), inter.clone(), root.clone()]);
    assert!(verify(&with_root, &trust).is_ok());
    // The same root, trusted from PEM.
    let from_pem = Trust::new().with_roots_pem(&pem(&root)).unwrap();
    assert!(verify(&open, &from_pem).is_ok());

    // A chain to a root not trusted is refused, and keys trusted by kid do
    // not stand for roots.
    let rogue = Trust::new().with_root(x5c_vector("rogue-root.der"));
    assert!(matches!(
        verify(&open, &rogue),
        Err(Refusal::InvalidCredential(_))
    ));
    assert!(verify(&open, &s.trust()).is_err());
    // Signed by a key that is not the leaf's.
    assert!(verify(
        &open_by(&s.provider, &[leaf.clone(), inter.clone()]),
        &trust
    )
    .is_err());
    // Without its intermediate.
    assert!(verify(&open_by(&leaf_key, std::slice::from_ref(&leaf)), &trust).is_err());
    // In the wrong order.
    assert!(verify(&open_by(&leaf_key, &[inter.clone(), leaf.clone()]), &trust).is_err());
    // Through an intermediate that is not a CA.
    let noca_key = SigningKey::from_pkcs8(&x5c_vector("noca-leaf.p8")).unwrap();
    let noca = open_by(
        &noca_key,
        &[x5c_vector("noca-leaf.der"), x5c_vector("noca-inter.der")],
    );
    assert!(verify(&noca, &trust).is_err());
    // With an expired leaf.
    let expired_key = SigningKey::from_pkcs8(&x5c_vector("expired-leaf.p8")).unwrap();
    let expired = open_by(
        &expired_key,
        &[x5c_vector("expired-leaf.der"), inter.clone()],
    );
    assert!(verify(&expired, &trust).is_err());
    // A chain that is not certificates.
    assert!(verify(
        &open_by(&leaf_key, &[b"not a certificate".to_vec()]),
        &trust
    )
    .is_err());

    // An RSA root signs a P-256 leaf.
    let rsa = Trust::new().with_root(x5c_vector("rsa-root.der"));
    let rsa_key = SigningKey::from_pkcs8(&x5c_vector("rsa-leaf.p8")).unwrap();
    assert!(verify(&open_by(&rsa_key, &[x5c_vector("rsa-leaf.der")]), &rsa).is_ok());
    assert!(verify(&open_by(&rsa_key, &[x5c_vector("rsa-leaf.der")]), &trust).is_err());
}
