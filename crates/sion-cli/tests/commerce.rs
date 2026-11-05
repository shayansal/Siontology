//! Agentic commerce end to end on a devnet: a Siontology merchant serves
//! ACP's checkout API, an agent buys from it over HTTP and pays in USD on
//! the ledger, and, with AP2, presents the checkout and payment mandates
//! its user signed, which the merchant checks before any money moves.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use sion_cli::acp::{self, Buyer, Catalog, Merchant, Request};
use sion_cli::ap2::{self, SigningKey, Trust};
use sion_cli::ledger::Ledger;
use sion_cli::session::Session;
use sion_cli::x402::{Facilitator, Payer};
use sion_core::ids::AssetId;
use sion_core::Amount;
use sion_node::{Devnet, DevnetConfig};

const TOKEN: &str = "buyer-token-0123456789";

/// Stops the server when dropped, so that a failed assertion ends the
/// test instead of leaving it serving.
struct Unblock<'s>(&'s tiny_http::Server);

impl Drop for Unblock<'_> {
    fn drop(&mut self) {
        self.0.unblock();
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn catalog() -> Catalog {
    Catalog::from_json(&json!({"offers": [
        {"id": "report", "name": "A research report", "description": "a cited report on a question", "price_usd": "4.50"},
        {"id": "review", "name": "A code review", "price_usd": "2.00"},
    ]}))
    .unwrap()
}

fn headers(key: Option<&str>) -> Vec<(String, String)> {
    let mut headers = vec![
        ("Authorization".to_string(), format!("Bearer {TOKEN}")),
        ("API-Version".to_string(), acp::API_VERSION.to_string()),
    ];
    if let Some(key) = key {
        headers.push(("Idempotency-Key".to_string(), key.to_string()));
    }
    headers
}

#[test]
fn an_agent_buys_through_acp_and_pays_on_the_ledger() {
    let devnet = Arc::new(Devnet::start(DevnetConfig::default()).unwrap());
    let ledger: &dyn Ledger = devnet.as_ref();
    let mut session = Session::open(ledger).unwrap();
    session
        .fund("shop", AssetId::usd(), Amount::units(1))
        .unwrap();
    session
        .fund("buyer", AssetId::usd(), Amount::units(100))
        .unwrap();
    let shop = session.id("shop");
    let chain = devnet.shared.chain_id;

    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let base = format!("http://{}", server.server_addr().to_ip().unwrap());
    let orders = std::env::temp_dir().join(format!("sion-acp-orders-{}.jsonl", std::process::id()));
    let _ = std::fs::remove_file(&orders);
    let merchant = Merchant::new(
        Facilitator::new(ledger, chain).unwrap(),
        shop,
        "Research desk",
        &base,
        catalog(),
        vec![TOKEN.to_string()],
    )
    .unwrap()
    .with_orders(orders.clone());

    // The contract's edges, answered without a network.
    let discovery = merchant.answer(&Request {
        method: "GET",
        path: "/.well-known/acp.json",
        headers: &[],
        body: "",
    });
    assert_eq!(discovery.body["protocol"]["version"], acp::API_VERSION);
    assert_eq!(discovery.body["api_base_url"], base.as_str());
    let body = r#"{"currency": "usd", "line_items": [{"id": "report"}], "capabilities": {}}"#;
    let post = |headers: &[(String, String)], body: &str| {
        merchant.answer(&Request {
            method: "POST",
            path: "/checkout_sessions",
            headers,
            body,
        })
    };
    let unauthorized = post(&[], body);
    assert_eq!(unauthorized.status, 401, "no token");
    let mut no_version = headers(Some("k0"));
    no_version.remove(1);
    let refused = post(&no_version, body);
    assert_eq!(
        (refused.status, refused.body["code"].as_str()),
        (400, Some("missing_api_version"))
    );
    assert_eq!(
        refused.body["supported_versions"],
        json!([acp::API_VERSION])
    );
    let refused = post(&headers(None), body);
    assert_eq!(refused.body["code"], "idempotency_key_required");
    let unknown = post(
        &headers(Some("k1")),
        r#"{"currency": "usd", "line_items": [{"id": "nothing"}], "capabilities": {}}"#,
    );
    assert_eq!(
        (unknown.status, unknown.body["code"].as_str()),
        (400, Some("invalid_item_id"))
    );
    assert_eq!(unknown.body["param"], "$.line_items[0].id");

    // The same key and body is answered with the stored answer; the same
    // key with another body is refused.
    let made = post(&headers(Some("k2")), body);
    assert_eq!(made.status, 201, "{}", made.body);
    let again = post(
        &headers(Some("k2")),
        r#"{"line_items": [{"id": "report"}], "capabilities": {}, "currency": "usd"}"#,
    );
    assert_eq!(again.body["id"], made.body["id"]);
    assert!(again
        .headers
        .iter()
        .any(|(n, v)| n == "Idempotent-Replayed" && v == "true"));
    let conflict = post(
        &headers(Some("k2")),
        r#"{"currency": "usd", "line_items": [{"id": "review"}], "capabilities": {}}"#,
    );
    assert_eq!(
        (conflict.status, conflict.body["code"].as_str()),
        (422, Some("idempotency_conflict"))
    );
    // The session is ACP's whole authoritative state: 450 cents, payable
    // through the Siontology handler at 4.5 USD.
    assert_eq!(made.body["status"], "ready_for_payment");
    assert_eq!(made.body["currency"], "usd");
    let total = made.body["totals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["type"] == "total")
        .unwrap();
    assert_eq!(total["amount"], 450);
    let handler = &made.body["capabilities"]["payment"]["handlers"][0];
    assert_eq!(handler["name"], acp::HANDLER);
    assert_eq!(handler["requires_delegate_payment"], false);
    assert_eq!(handler["config"]["requirements"]["amount"], "4500000");
    let id = made.body["id"].as_str().unwrap();
    let canceled = merchant.answer(&Request {
        method: "POST",
        path: &format!("/checkout_sessions/{id}/cancel"),
        headers: &headers(Some("k3")),
        body: "{}",
    });
    assert_eq!(canceled.body["status"], "canceled");
    let twice = merchant.answer(&Request {
        method: "POST",
        path: &format!("/checkout_sessions/{id}/cancel"),
        headers: &headers(Some("k4")),
        body: "{}",
    });
    assert_eq!(twice.status, 405);

    // Over HTTP: the agent buys a report and a review and pays on the
    // ledger; a checkout over its limit is not paid for.
    let before = session.balance("shop", AssetId::usd()).unwrap();
    std::thread::scope(|scope| {
        scope.spawn(|| acp::serve(&merchant, &server, None));
        let _stop = Unblock(&server);
        let buyer = Buyer {
            base_url: &base,
            token: TOKEN,
            chain,
            max_price: Amount::units(10),
        };
        let payer = Payer::Wallet("buyer".into());
        let done = acp::buy(&mut session, &payer, &buyer, &["report", "review"], |_| {
            Ok(None)
        })
        .unwrap();
        assert_eq!(done["status"], "completed", "{done}");
        let order = &done["order"];
        assert!(order["permalink_url"].as_str().unwrap().starts_with(&base));
        let page: Value = ureq::get(order["permalink_url"].as_str().unwrap())
            .call()
            .unwrap()
            .into_json()
            .unwrap();
        assert_eq!(page["order"]["id"], order["id"]);

        let cheap = Buyer {
            max_price: Amount::units(1),
            ..buyer
        };
        let over = acp::buy(&mut session, &payer, &cheap, &["report"], |_| Ok(None));
        assert!(over.is_err());
    });
    let after = session.balance("shop", AssetId::usd()).unwrap();
    assert_eq!(
        after.micros() - before.micros(),
        6_500_000,
        "paid 6.50 USD, once"
    );
    let written = std::fs::read_to_string(&orders).unwrap();
    assert_eq!(written.lines().count(), 1);
    let record: Value = serde_json::from_str(written.lines().next().unwrap()).unwrap();
    assert_eq!(record["total_cents"], 650);
    assert!(record["transaction"]
        .as_str()
        .is_some_and(|t| t.len() == 64));
    let _ = std::fs::remove_file(&orders);
}

/// The user's trusted surface signs open mandates for the agent's key; the
/// agent closes them for the session it was given.
struct Wallet {
    provider: SigningKey,
    agent: SigningKey,
}

impl Wallet {
    fn open_checkout(&self, merchant: &Value, item: &str) -> String {
        ap2::issue(
            &json!({
                "vct": ap2::CHECKOUT_OPEN,
                "constraints": [
                    {"type": "checkout.allowed_merchants", "allowed": [{"id": merchant["id"]}]},
                    {"type": "checkout.line_items", "items": [
                        {"id": "line_1", "quantity": 1, "acceptable_items": [{"id": item, "title": item}]},
                    ]},
                ],
                "cnf": {"jwk": self.agent.public().jwk()},
                "iat": now(),
                "exp": now() + 3600,
            }),
            &self.provider,
            "provider-1",
        )
    }

    fn open_payment(&self, merchant: &Value, open_checkout: &str, max_cents: u64) -> String {
        let reference = ap2::Token::parse(open_checkout).unwrap().sd_hash().unwrap();
        ap2::issue(
            &json!({
                "vct": ap2::PAYMENT_OPEN,
                "constraints": [
                    {"type": "payment.amount_range", "currency": "USD", "max": max_cents, "min": 0},
                    {"type": "payment.allowed_payees", "allowed": [{"id": merchant["id"], "name": merchant["name"]}]},
                    {"type": "payment.reference", "conditional_transaction_id": reference},
                ],
                "cnf": {"jwk": self.agent.public().jwk()},
                "iat": now(),
                "exp": now() + 3600,
            }),
            &self.provider,
            "provider-1",
        )
    }

    /// The agent's closing of both, for the session it was given.
    fn close(&self, session: &Value, open_checkout: &str, open_payment: &str) -> Value {
        let offered = &session["ap2"];
        let checkout_jwt = offered["checkout_jwt"].as_str().unwrap();
        let checkout_hash = ap2::hash(checkout_jwt);
        let audience = offered["audience"].as_str().unwrap();
        let nonce = offered["nonce"].as_str().unwrap();
        let total = session["totals"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["type"] == "total")
            .unwrap()["amount"]
            .clone();
        let closed_checkout = ap2::close(
            open_checkout,
            &json!({"vct": ap2::CHECKOUT, "checkout_jwt": checkout_jwt, "checkout_hash": checkout_hash}),
            &self.agent,
            audience,
            nonce,
            now(),
        )
        .unwrap();
        let closed_payment = ap2::close(
            open_payment,
            &json!({
                "vct": ap2::PAYMENT,
                "transaction_id": checkout_hash,
                "payee": offered["merchant"],
                "payment_amount": {"amount": total, "currency": "USD"},
                "payment_instrument": {"id": "ledger", "type": acp::INSTRUMENT},
            }),
            &self.agent,
            audience,
            nonce,
            now(),
        )
        .unwrap();
        json!({
            "checkout_mandate": ap2::chain(&[open_checkout, &closed_checkout]),
            "payment_mandate": ap2::chain(&[open_payment, &closed_payment]),
        })
    }
}

#[test]
fn a_merchant_takes_ap2_mandates_and_moves_no_money_for_a_refused_one() {
    let devnet = Arc::new(Devnet::start(DevnetConfig::default()).unwrap());
    let ledger: &dyn Ledger = devnet.as_ref();
    let mut session = Session::open(ledger).unwrap();
    session
        .fund("shop", AssetId::usd(), Amount::units(1))
        .unwrap();
    session
        .fund("buyer", AssetId::usd(), Amount::units(100))
        .unwrap();
    let shop = session.id("shop");
    let chain = devnet.shared.chain_id;

    let wallet = Wallet {
        provider: SigningKey::generate(),
        agent: SigningKey::generate(),
    };
    let trust = Trust::new().with("provider-1", wallet.provider.public());
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let base = format!("http://{}", server.server_addr().to_ip().unwrap());
    let merchant = Merchant::new(
        Facilitator::new(ledger, chain).unwrap(),
        shop,
        "Research desk",
        &base,
        catalog(),
        vec![TOKEN.to_string()],
    )
    .unwrap()
    .with_ap2(SigningKey::generate(), trust, true);
    let merchant_key = merchant.ap2_key().unwrap();
    let identity = merchant.identity();

    let before = session.balance("shop", AssetId::usd()).unwrap();
    std::thread::scope(|scope| {
        scope.spawn(|| acp::serve(&merchant, &server, None));
        let _stop = Unblock(&server);
        let buyer = Buyer {
            base_url: &base,
            token: TOKEN,
            chain,
            max_price: Amount::units(10),
        };
        let payer = Payer::Wallet("buyer".into());

        // Without a mandate, a merchant that requires one refuses, and
        // nothing is paid.
        let refused = acp::buy(&mut session, &payer, &buyer, &["report"], |_| Ok(None));
        assert!(refused
            .unwrap_err()
            .to_string()
            .contains("requires an AP2 checkout mandate"));

        // A mandate for another item is refused before any money moves.
        let open_checkout = wallet.open_checkout(&identity, "review");
        let open_payment = wallet.open_payment(&identity, &open_checkout, 1_000);
        let refused = acp::buy(&mut session, &payer, &buyer, &["report"], |offered| {
            Ok(Some(wallet.close(offered, &open_checkout, &open_payment)))
        });
        let said = refused.unwrap_err().to_string();
        assert!(said.contains("invalid_mandate"), "{said}");

        // The user's mandate allows the report up to 10 USD: the agent
        // closes it, the merchant checks it, is paid, and signs receipts.
        let open_checkout = wallet.open_checkout(&identity, "report");
        let open_payment = wallet.open_payment(&identity, &open_checkout, 1_000);
        let mut presented = Value::Null;
        let done = acp::buy(&mut session, &payer, &buyer, &["report"], |offered| {
            // The checkout the merchant asks a mandate for is one it signed.
            let jwt = offered["ap2"]["checkout_jwt"].as_str().unwrap();
            let (_, checkout) = ap2::verify_jws(jwt, &merchant_key).unwrap();
            assert_eq!(checkout["merchant"], identity);
            assert_eq!(checkout["line_items"][0]["item"]["id"], "report");
            presented = wallet.close(offered, &open_checkout, &open_payment);
            Ok(Some(presented.clone()))
        })
        .unwrap();
        assert_eq!(done["status"], "completed", "{done}");
        let (_, receipt) = ap2::verify_jws(
            done["ap2"]["checkout_receipt"].as_str().unwrap(),
            &merchant_key,
        )
        .unwrap();
        assert_eq!(receipt["status"], "Success");
        assert_eq!(receipt["order_id"], done["order"]["id"]);
        // The receipt names the mandate it answers: its closing token's hash.
        let closed = ap2::split(presented["checkout_mandate"].as_str().unwrap()).unwrap();
        assert_eq!(receipt["reference"], closed[1].sd_hash().unwrap().as_str());
        let (_, paid) = ap2::verify_jws(
            done["ap2"]["payment_receipt"].as_str().unwrap(),
            &merchant_key,
        )
        .unwrap();
        assert_eq!(paid["status"], "Success");
        assert_eq!(paid["payment_id"].as_str().map(str::len), Some(64));

        // The same open mandates do not allow a second, dearer purchase:
        // the line item is one report, the amount at most 10 USD.
        let refused = acp::buy(
            &mut session,
            &payer,
            &buyer,
            &["report", "report", "review"],
            |offered| Ok(Some(wallet.close(offered, &open_checkout, &open_payment))),
        );
        assert!(refused.is_err());
        // Nor the same purchase again: an order was made under them.
        let again = acp::buy(&mut session, &payer, &buyer, &["report"], |offered| {
            Ok(Some(wallet.close(offered, &open_checkout, &open_payment)))
        });
        let said = again.unwrap_err().to_string();
        assert!(
            said.contains("already made under this open mandate"),
            "{said}"
        );
    });
    let after = session.balance("shop", AssetId::usd()).unwrap();
    assert_eq!(
        after.micros() - before.micros(),
        4_500_000,
        "only the approved report was paid for"
    );
}
