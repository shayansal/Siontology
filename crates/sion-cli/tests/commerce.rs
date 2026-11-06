//! Agentic commerce end to end on a devnet: a Siontology merchant serves
//! ACP's checkout and cart API, an agent buys from it over HTTP and pays in
//! USD on the ledger, and, with AP2, presents the checkout and payment
//! mandates its user signed, which the merchant checks before any money
//! moves. The merchant keeps its state across restarts, finishes a payment
//! left settling from its receipt, signs and sends order webhooks, checks
//! its buyers' request signatures, and fulfills and refunds through its
//! admin API.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use sion_cli::acp::http::{self, Client, Response, Service};
use sion_cli::acp::signing::Signer;
use sion_cli::acp::state::Store;
use sion_cli::acp::webhooks::{self, Hook};
use sion_cli::acp::{self, Buyer, Catalog, Merchant, Request};
use sion_cli::ap2::{self, SigningKey, Trust};
use sion_cli::ledger::Ledger;
use sion_cli::session::Session;
use sion_cli::x402::{self, Facilitator, Payer, PaymentRequirements};
use sion_core::hash::Hash32;
use sion_core::ids::{AccountId, AssetId};
use sion_core::Amount;
use sion_node::{Devnet, DevnetConfig};

const TOKEN: &str = "buyer-token-0123456789";
const ADMIN: &str = "admin-token-0123456789";
const WEBHOOK_SECRET: &[u8] = b"whsec_test_0123456789abcdef";

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

fn buyer_client() -> Client {
    Client::new("buyer", TOKEN).unwrap()
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

fn ask(
    merchant: &Merchant<'_>,
    method: &str,
    path: &str,
    key: Option<&str>,
    body: &str,
) -> Response {
    merchant.answer(&Request {
        method,
        path,
        headers: &headers(key),
        body,
    })
}

fn temp(name: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("sion-acp-{name}-{}", std::process::id()));
    let _ = std::fs::remove_file(&path);
    path
}

/// A devnet with a shop and a buyer funded.
fn market() -> (Arc<Devnet>, Hash32) {
    let devnet = Arc::new(Devnet::start(DevnetConfig::default()).unwrap());
    let chain = devnet.shared.chain_id;
    let ledger: &dyn Ledger = devnet.as_ref();
    let mut session = Session::open(ledger).unwrap();
    session
        .fund("shop", AssetId::usd(), Amount::units(1))
        .unwrap();
    session
        .fund("buyer", AssetId::usd(), Amount::units(100))
        .unwrap();
    (devnet, chain)
}

fn shop(ledger: &dyn Ledger) -> AccountId {
    Session::open(ledger).unwrap().id("shop")
}

/// The ledger handler's quote in a session.
fn requirements_of(session: &Value) -> PaymentRequirements {
    let handler = acp::buyer::handler_of(session, acp::HANDLER).unwrap();
    serde_json::from_value(handler["config"]["requirements"].clone()).unwrap()
}

/// A completion paying on the ledger, its credential named as the
/// handler's first version named it (`transaction`), which a merchant still
/// takes; buyers now send ACP's `token` ([`acp::buyer::ledger_completion`]).
fn ledger_completion(signed: &sion_exec::SignedTransaction) -> String {
    json!({
        "payment_data": {
            "handler_id": acp::HANDLER_ID,
            "instrument": {
                "type": acp::INSTRUMENT,
                "credential": {
                    "type": "signed_transaction",
                    "transaction": hex::encode(sion_core::encode::encode(signed)),
                },
            },
        },
    })
    .to_string()
}

#[test]
fn an_agent_buys_through_acp_and_pays_on_the_ledger() {
    let (devnet, chain) = market();
    let ledger: &dyn Ledger = devnet.as_ref();
    let mut session = Session::open(ledger).unwrap();
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let base = format!("http://{}", server.server_addr().to_ip().unwrap());
    let orders = temp("orders.jsonl");
    let merchant = Merchant::new(
        Facilitator::new(ledger, chain).unwrap(),
        shop(ledger),
        "Research desk",
        &base,
        catalog(),
        vec![buyer_client()],
    )
    .unwrap()
    .with_orders(orders.clone());

    // The contract's edges, answered without a network.
    let discovery = ask(&merchant, "GET", "/.well-known/acp.json", None, "");
    assert_eq!(discovery.body["protocol"]["version"], acp::API_VERSION);
    assert_eq!(discovery.body["api_base_url"], base.as_str());
    assert_eq!(
        discovery.body["capabilities"]["services"],
        json!(["checkout", "orders", "carts"])
    );
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
    assert_eq!(again.header("Idempotent-Replayed"), Some("true"));
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
    assert_eq!(acp::buyer::total_of(&made.body), Some(450));
    let handler = &made.body["capabilities"]["payment"]["handlers"][0];
    assert_eq!(handler["name"], acp::HANDLER);
    assert_eq!(handler["requires_delegate_payment"], false);
    assert_eq!(handler["config"]["requirements"]["amount"], "4500000");
    let id = made.body["id"].as_str().unwrap();
    let canceled = ask(
        &merchant,
        "POST",
        &format!("/checkout_sessions/{id}/cancel"),
        Some("k3"),
        "{}",
    );
    assert_eq!(canceled.body["status"], "canceled");
    let twice = ask(
        &merchant,
        "POST",
        &format!("/checkout_sessions/{id}/cancel"),
        Some("k4"),
        "{}",
    );
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
            signer: None,
        };
        let payer = Payer::Wallet("buyer".into());
        let done = acp::buy(&mut session, &payer, &buyer, &["report", "review"], |_| {
            Ok(None)
        })
        .unwrap();
        assert_eq!(done["status"], "completed", "{done}");
        let order = &done["order"];
        assert_eq!(order["type"], "order");
        assert_eq!(order["status"], "confirmed");
        assert_eq!(order["line_items"][0]["quantity"]["ordered"], 1);
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
    assert_eq!(record["paid"]["rail"], "ledger");
    let _ = std::fs::remove_file(&orders);
}

#[test]
fn a_merchant_keeps_its_sessions_carts_and_orders_across_a_restart() {
    let (devnet, chain) = market();
    let ledger: &dyn Ledger = devnet.as_ref();
    let mut session = Session::open(ledger).unwrap();
    let state = temp("state.json");
    let make = || {
        Merchant::new(
            Facilitator::new(ledger, chain).unwrap(),
            shop(ledger),
            "Research desk",
            "http://shop.invalid",
            catalog(),
            vec![buyer_client()],
        )
        .unwrap()
        .with_state(Store::new(state.clone()))
        .unwrap()
    };
    let body = r#"{"currency": "usd", "line_items": [{"id": "report"}]}"#;

    // Before it stops: a session, a cart, and a payment the buyer signed
    // against the session's quote.
    let first = make();
    let made = ask(&first, "POST", "/checkout_sessions", Some("s1"), body);
    assert_eq!(made.status, 201, "{}", made.body);
    let id = made.body["id"].as_str().unwrap().to_string();
    let cart = ask(
        &first,
        "POST",
        "/carts",
        Some("c1"),
        r#"{"line_items": [{"id": "review", "quantity": 2}]}"#,
    );
    assert_eq!(cart.status, 201, "{}", cart.body);
    let cart_id = cart.body["id"].as_str().unwrap().to_string();
    let requirements = requirements_of(&made.body);
    let signed =
        x402::sign_payment(&mut session, &Payer::Wallet("buyer".into()), &requirements).unwrap();
    drop(first);
    // The state is the owner's alone.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&state).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    // After: all of it is there, and the payment settles against the quote
    // the merchant took back.
    let second = make();
    let got = ask(
        &second,
        "GET",
        &format!("/checkout_sessions/{id}"),
        None,
        "",
    );
    assert_eq!(got.body["status"], "ready_for_payment");
    assert_eq!(
        requirements_of(&got.body).extra["nonce"],
        requirements.extra["nonce"]
    );
    let replayed = ask(&second, "POST", "/checkout_sessions", Some("s1"), body);
    assert_eq!(replayed.body["id"], id.as_str());
    assert_eq!(replayed.header("Idempotent-Replayed"), Some("true"));
    let kept_cart = ask(&second, "GET", &format!("/carts/{cart_id}"), None, "");
    assert_eq!(kept_cart.status, 200);
    assert_eq!(kept_cart.body["line_items"][0]["quantity"], 2);
    let done = ask(
        &second,
        "POST",
        &format!("/checkout_sessions/{id}/complete"),
        Some("p1"),
        &ledger_completion(&signed),
    );
    session.settle_all();
    assert_eq!(done.body["status"], "completed", "{}", done.body);
    let order = done.body["order"]["id"].as_str().unwrap().to_string();
    drop(second);

    let third = make();
    assert_eq!(third.orders().len(), 1);
    let page = ask(&third, "GET", &format!("/orders/{order}"), None, "");
    assert_eq!(page.status, 200);
    assert_eq!(page.body["order"]["checkout_session_id"], id.as_str());
    let _ = std::fs::remove_file(&state);
}

#[test]
fn a_payment_left_settling_when_the_merchant_stopped_is_finished_from_its_receipt() {
    let (devnet, chain) = market();
    let ledger: &dyn Ledger = devnet.as_ref();
    let mut session = Session::open(ledger).unwrap();
    let state = temp("recovery.json");
    let make = || {
        Merchant::new(
            Facilitator::new(ledger, chain).unwrap(),
            shop(ledger),
            "Research desk",
            "http://shop.invalid",
            catalog(),
            vec![buyer_client()],
        )
        .unwrap()
        .with_state(Store::new(state.clone()))
        .unwrap()
    };
    let first = make();
    let body = r#"{"currency": "usd", "line_items": [{"id": "report"}]}"#;
    let paid = ask(&first, "POST", "/checkout_sessions", Some("s1"), body);
    let card = ask(&first, "POST", "/checkout_sessions", Some("s2"), body);
    let id = paid.body["id"].as_str().unwrap().to_string();
    let card_id = card.body["id"].as_str().unwrap().to_string();
    let signed = x402::sign_payment(
        &mut session,
        &Payer::Wallet("buyer".into()),
        &requirements_of(&paid.body),
    )
    .unwrap();
    let before = session.balance("shop", AssetId::usd()).unwrap();
    drop(first);

    // The merchant stopped after it submitted the payment and before it
    // made the order; another session stopped while a card was charged.
    ledger.submit(&signed).unwrap();
    let mut kept: Value = serde_json::from_str(&std::fs::read_to_string(&state).unwrap()).unwrap();
    kept["sessions"][&id]["status"] = json!("complete_in_progress");
    kept["sessions"][&id]["pending"] = json!({
        "digest": signed.digest().to_string(),
        "payer": signed.tx.sender.to_string(),
        "since": now(),
        "ap2": null,
    });
    kept["sessions"][&card_id]["status"] = json!("complete_in_progress");
    std::fs::write(&state, kept.to_string()).unwrap();

    // Back, it finishes the payment from its verified receipt.
    let second = make();
    let deadline = Instant::now() + Duration::from_secs(90);
    let done = loop {
        second.tick();
        let got = ask(
            &second,
            "GET",
            &format!("/checkout_sessions/{id}"),
            None,
            "",
        );
        if got.body["status"] == "completed" {
            break got.body;
        }
        assert_eq!(got.body["status"], "complete_in_progress", "{}", got.body);
        assert!(Instant::now() < deadline, "the payment was not finished");
        std::thread::sleep(Duration::from_millis(200));
    };
    assert_eq!(done["order"]["checkout_session_id"], id.as_str());
    session.settle_all();
    let after = session.balance("shop", AssetId::usd()).unwrap();
    assert_eq!(after.micros() - before.micros(), 4_500_000);
    // The interrupted card payment is payable again, and says why.
    let interrupted = ask(
        &second,
        "GET",
        &format!("/checkout_sessions/{card_id}"),
        None,
        "",
    );
    assert_eq!(interrupted.body["status"], "ready_for_payment");
    assert_eq!(interrupted.body["messages"][0]["code"], "interrupted");
    let _ = std::fs::remove_file(&state);
}

#[test]
fn a_cart_holds_items_and_estimates_before_a_checkout() {
    let (devnet, chain) = market();
    let ledger: &dyn Ledger = devnet.as_ref();
    let merchant = Merchant::new(
        Facilitator::new(ledger, chain).unwrap(),
        shop(ledger),
        "Research desk",
        "http://shop.invalid",
        catalog(),
        vec![buyer_client()],
    )
    .unwrap();
    // An item not sold here is a message, not a silent drop.
    let made = ask(
        &merchant,
        "POST",
        "/carts",
        Some("c1"),
        r#"{"line_items": [{"id": "review", "quantity": 2}, {"id": "nothing"}]}"#,
    );
    assert_eq!(made.status, 201, "{}", made.body);
    assert_eq!(made.body["line_items"].as_array().unwrap().len(), 1);
    assert_eq!(acp::buyer::total_of(&made.body), Some(400));
    assert_eq!(made.body["messages"][0]["code"], "not_found");
    assert_eq!(made.body["messages"][0]["param"], "$.line_items[1].id");
    let id = made.body["id"].as_str().unwrap();
    // A cart's POST takes an idempotency key, as a session's does.
    assert_eq!(
        ask(
            &merchant,
            "POST",
            "/carts",
            None,
            r#"{"line_items": [{"id": "review"}]}"#
        )
        .status,
        400
    );
    // Replaced whole.
    let replaced = merchant.answer(&Request {
        method: "PUT",
        path: &format!("/carts/{id}"),
        headers: &headers(None),
        body: r#"{"line_items": [{"id": "report"}, {"id": "review"}]}"#,
    });
    assert_eq!(replaced.status, 200, "{}", replaced.body);
    assert_eq!(acp::buyer::total_of(&replaced.body), Some(650));
    assert!(replaced.body["messages"].as_array().unwrap().is_empty());
    let read = ask(&merchant, "GET", &format!("/carts/{id}"), None, "");
    assert_eq!(read.body, replaced.body);
    // Nothing sold here at all.
    let empty = ask(
        &merchant,
        "POST",
        "/carts",
        Some("c2"),
        r#"{"line_items": [{"id": "nothing"}]}"#,
    );
    assert_eq!(
        (empty.status, empty.body["code"].as_str()),
        (422, Some("out_of_stock"))
    );
    let bad = ask(
        &merchant,
        "POST",
        "/carts",
        Some("c3"),
        r#"{"line_items": [{"id": "review", "quantity": 0}]}"#,
    );
    assert_eq!(bad.status, 400);
    // Canceled, it is gone.
    let canceled = ask(
        &merchant,
        "POST",
        &format!("/carts/{id}/cancel"),
        Some("c4"),
        "{}",
    );
    assert_eq!(canceled.status, 200);
    assert_eq!(
        ask(&merchant, "GET", &format!("/carts/{id}"), None, "").status,
        404
    );
}

#[test]
fn a_buyer_that_signs_is_refused_unsigned_or_wrongly_signed() {
    let (devnet, chain) = market();
    let ledger: &dyn Ledger = devnet.as_ref();
    let hmac = Signer::hmac(b"a shared signing secret, 32 bytes");
    let es256 = Signer::Es256(SigningKey::generate());
    let other = Signer::Es256(SigningKey::generate());
    const ES_TOKEN: &str = "es256-buyer-token-0123456789";
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let base = format!("http://{}", server.server_addr().to_ip().unwrap());
    let merchant = Merchant::new(
        Facilitator::new(ledger, chain).unwrap(),
        shop(ledger),
        "Research desk",
        &base,
        catalog(),
        vec![
            buyer_client().signing(hmac.verifier(), true),
            Client::new("es-buyer", ES_TOKEN)
                .unwrap()
                .signing(es256.verifier(), true),
        ],
    )
    .unwrap();
    let body = r#"{"currency": "usd", "line_items": [{"id": "report"}]}"#;
    let signed = |token: &str, key: &str, signature: (String, String), body: &str| {
        let mut with = vec![
            ("Authorization".to_string(), format!("Bearer {token}")),
            ("API-Version".to_string(), acp::API_VERSION.to_string()),
            ("Idempotency-Key".to_string(), key.to_string()),
        ];
        with.push(("Signature".to_string(), signature.0));
        with.push(("Timestamp".to_string(), signature.1));
        merchant.answer(&Request {
            method: "POST",
            path: "/checkout_sessions",
            headers: &with,
            body,
        })
    };
    let unsigned = ask(&merchant, "POST", "/checkout_sessions", Some("u1"), body);
    assert_eq!(
        (unsigned.status, unsigned.body["code"].as_str()),
        (401, Some("invalid_signature"))
    );
    // HMAC over timestamp.body.
    assert_eq!(
        signed(TOKEN, "h1", hmac.sign(body, now()), body).status,
        201
    );
    let wrong_body = signed(
        TOKEN,
        "h2",
        hmac.sign(body, now()),
        r#"{"currency": "usd", "line_items": [{"id": "review"}]}"#,
    );
    assert_eq!(wrong_body.status, 401);
    let stale = signed(TOKEN, "h3", hmac.sign(body, now() - 600), body);
    assert_eq!(stale.status, 401);
    assert!(stale.body["message"]
        .as_str()
        .unwrap()
        .contains("Timestamp"));
    // ES256 over the canonical body: key order and spacing do not matter.
    let reordered = r#"{ "line_items": [ {"id": "report"} ], "currency": "usd" }"#;
    assert_eq!(
        signed(ES_TOKEN, "e1", es256.sign(body, now()), reordered).status,
        201
    );
    // Another client's key does not sign for this one.
    assert_eq!(
        signed(ES_TOKEN, "e2", other.sign(body, now()), body).status,
        401
    );
    assert_eq!(
        signed(ES_TOKEN, "e3", hmac.sign(body, now()), body).status,
        401
    );

    // Over HTTP, the buyer signs what it sends.
    std::thread::scope(|scope| {
        scope.spawn(|| acp::serve(&merchant, &server, None));
        let _stop = Unblock(&server);
        let signing = Buyer {
            base_url: &base,
            token: ES_TOKEN,
            chain,
            max_price: Amount::units(10),
            signer: Some(&es256),
        };
        let cart = signing.cart(&[("report", 1)]).unwrap();
        assert_eq!(
            signing.cart_get(cart["id"].as_str().unwrap()).unwrap()["id"],
            cart["id"]
        );
        let unsigned = Buyer {
            signer: None,
            ..signing
        };
        assert!(unsigned.cart(&[("report", 1)]).is_err());
    });
}

/// An agent platform's webhook endpoint: every event verified, kept.
struct Receiver {
    events: Mutex<Vec<Value>>,
}

impl Service for Receiver {
    fn answer(&self, request: &Request<'_>) -> Response {
        match webhooks::receive(request, WEBHOOK_SECRET, http::now()) {
            Ok(event) => {
                self.events.lock().unwrap().push(event);
                Response::json(200, json!({"received": true}))
            }
            Err(refused) => refused,
        }
    }
}

fn wait_for(receiver: &Receiver, count: usize) -> Vec<Value> {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let events = receiver.events.lock().unwrap().clone();
        if events.len() >= count {
            return events;
        }
        assert!(
            Instant::now() < deadline,
            "{} event(s) of {count} came",
            events.len()
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn admin(base: &str, method: &str, path: &str, token: Option<&str>, body: Value) -> (u16, Value) {
    let client = http::outbound(true).unwrap();
    let bearer = token.map(|t| format!("Bearer {t}")).unwrap_or_default();
    let mut headers = vec![("Content-Type", "application/json")];
    if token.is_some() {
        headers.push(("Authorization", bearer.as_str()));
    }
    let text = body.to_string();
    http::call(
        &client,
        method,
        &format!("{base}{path}"),
        &headers,
        Some(text.as_bytes()),
    )
    .unwrap()
}

#[test]
fn orders_are_sent_by_webhook_fulfilled_and_refunded_on_the_ledger() {
    let (devnet, chain) = market();
    let ledger: &dyn Ledger = devnet.as_ref();
    let mut session = Session::open(ledger).unwrap();
    let hooks = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let hook_url = format!(
        "http://{}/agentic_checkout/webhooks/order_events",
        hooks.server_addr().to_ip().unwrap()
    );
    let receiver = Receiver {
        events: Mutex::new(Vec::new()),
    };
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let base = format!("http://{}", server.server_addr().to_ip().unwrap());
    let merchant = Merchant::new(
        Facilitator::new(ledger, chain).unwrap(),
        shop(ledger),
        "Research desk",
        &base,
        catalog(),
        vec![buyer_client()],
    )
    .unwrap()
    .with_webhooks(Hook::new(&hook_url, WEBHOOK_SECRET).unwrap())
    .with_admin(ADMIN)
    .unwrap()
    .with_refunds(Session::open(ledger).unwrap(), "shop");

    std::thread::scope(|scope| {
        scope.spawn(|| acp::serve(&receiver, &hooks, None));
        let _stop_hooks = Unblock(&hooks);
        scope.spawn(|| acp::serve(&merchant, &server, None));
        let _stop = Unblock(&server);
        let buyer = Buyer {
            base_url: &base,
            token: TOKEN,
            chain,
            max_price: Amount::units(10),
            signer: None,
        };
        let payer = Payer::Wallet("buyer".into());
        let done = acp::buy(&mut session, &payer, &buyer, &["report"], |_| Ok(None)).unwrap();
        let order = done["order"]["id"].as_str().unwrap().to_string();

        // The order is sent as it was made, signed.
        let events = wait_for(&receiver, 1);
        assert_eq!(events[0]["type"], "order_create");
        assert_eq!(events[0]["data"]["id"], order.as_str());
        assert_eq!(events[0]["data"]["status"], "confirmed");

        // The admin API is the operator's alone.
        let (status, _) = admin(
            &base,
            "POST",
            &format!("/admin/orders/{order}/fulfill"),
            None,
            json!({}),
        );
        assert_eq!(status, 401);
        let (status, _) = admin(
            &base,
            "POST",
            &format!("/admin/orders/{order}/fulfill"),
            Some(TOKEN),
            json!({}),
        );
        assert_eq!(status, 401);

        // Fulfilled: delivered, and the buyer's platform told.
        let (status, fulfilled) = admin(
            &base,
            "POST",
            &format!("/admin/orders/{order}/fulfill"),
            Some(ADMIN),
            json!({"access_url": "https://shop.example/reports/1", "description": "the report"}),
        );
        assert_eq!(status, 200, "{fulfilled}");
        assert_eq!(fulfilled["status"], "completed");
        assert_eq!(fulfilled["line_items"][0]["quantity"]["fulfilled"], 1);
        assert_eq!(
            fulfilled["fulfillments"][0]["digital_delivery"]["access_url"],
            "https://shop.example/reports/1"
        );
        let events = wait_for(&receiver, 2);
        assert_eq!(events[1]["type"], "order_update");
        assert_eq!(events[1]["data"]["fulfillments"][0]["status"], "delivered");

        // Refunded in part: a transfer back to the payer on the ledger.
        session.settle_all();
        let before = session.balance("buyer", AssetId::usd()).unwrap();
        let (status, refunded) = admin(
            &base,
            "POST",
            &format!("/admin/orders/{order}/refund"),
            Some(ADMIN),
            json!({"amount": 150, "reason": "partial"}),
        );
        assert_eq!(status, 200, "{refunded}");
        assert_eq!(refunded["adjustments"][0]["type"], "refund");
        assert_eq!(refunded["adjustments"][0]["amount"], 150);
        let amount_refunded = refunded["totals"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["type"] == "amount_refunded")
            .unwrap()["amount"]
            .clone();
        assert_eq!(amount_refunded, 150);
        let after = session.balance("buyer", AssetId::usd()).unwrap();
        assert_eq!(after.micros() - before.micros(), 1_500_000);
        // No more than was paid.
        let (status, _) = admin(
            &base,
            "POST",
            &format!("/admin/orders/{order}/refund"),
            Some(ADMIN),
            json!({"amount": 301}),
        );
        assert_eq!(status, 400);
        let events = wait_for(&receiver, 3);
        assert_eq!(events[2]["data"]["adjustments"][0]["amount"], 150);
        let (status, listed) = admin(&base, "GET", "/admin/orders", Some(ADMIN), json!({}));
        assert_eq!(status, 200);
        assert_eq!(listed["orders"].as_array().unwrap().len(), 1);
    });
    // Every event was delivered, once.
    assert!(merchant.outbox().iter().all(|d| d.delivered_at.is_some()));
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

    fn open_payment(&self, merchant: &Value, open_checkout: &str, extra: Value) -> String {
        let reference = ap2::Token::parse(open_checkout).unwrap().sd_hash().unwrap();
        let mut constraints = vec![
            json!({"type": "payment.amount_range", "currency": "USD", "max": 1000, "min": 0}),
            json!({"type": "payment.allowed_payees", "allowed": [{"id": merchant["id"], "name": merchant["name"]}]}),
            json!({"type": "payment.reference", "conditional_transaction_id": reference}),
        ];
        constraints.extend(extra.as_array().cloned().unwrap_or_default());
        ap2::issue(
            &json!({
                "vct": ap2::PAYMENT_OPEN,
                "constraints": constraints,
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
        let total = acp::buyer::total_of(session).unwrap();
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
    let (devnet, chain) = market();
    let ledger: &dyn Ledger = devnet.as_ref();
    let mut session = Session::open(ledger).unwrap();
    let wallet = Wallet {
        provider: SigningKey::generate(),
        agent: SigningKey::generate(),
    };
    let trust = Trust::new().with("provider-1", wallet.provider.public());
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let base = format!("http://{}", server.server_addr().to_ip().unwrap());
    let merchant = Merchant::new(
        Facilitator::new(ledger, chain).unwrap(),
        shop(ledger),
        "Research desk",
        &base,
        catalog(),
        vec![buyer_client()],
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
            signer: None,
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
        let open_payment = wallet.open_payment(&identity, &open_checkout, json!([]));
        let refused = acp::buy(&mut session, &payer, &buyer, &["report"], |offered| {
            Ok(Some(wallet.close(offered, &open_checkout, &open_payment)))
        });
        let said = refused.unwrap_err().to_string();
        assert!(said.contains("invalid_mandate"), "{said}");

        // The user's mandate allows the report up to 10 USD: the agent
        // closes it, the merchant checks it, is paid, and signs receipts.
        let open_checkout = wallet.open_checkout(&identity, "report");
        let open_payment = wallet.open_payment(&identity, &open_checkout, json!([]));
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

        // A recurring payment mandate is used again, as often as it says:
        // at most twice, and no more than 9.00 USD in all.
        let open_checkout = wallet.open_checkout(&identity, "report");
        let recurring = wallet.open_payment(
            &identity,
            &open_checkout,
            json!([
                {"type": "payment.agent_recurrence", "frequency": "ON_DEMAND", "max_occurrences": 2},
                {"type": "payment.budget", "currency": "USD", "max": 9.00},
            ]),
        );
        for _ in 0..2 {
            let done = acp::buy(&mut session, &payer, &buyer, &["report"], |offered| {
                Ok(Some(wallet.close(offered, &open_checkout, &recurring)))
            })
            .unwrap();
            assert_eq!(done["status"], "completed");
        }
        let third = acp::buy(&mut session, &payer, &buyer, &["report"], |offered| {
            Ok(Some(wallet.close(offered, &open_checkout, &recurring)))
        });
        let said = third.unwrap_err().to_string();
        assert!(
            said.contains("over the budget") || said.contains("allows 2 payments"),
            "{said}"
        );
    });
    let after = session.balance("shop", AssetId::usd()).unwrap();
    assert_eq!(
        after.micros() - before.micros(),
        3 * 4_500_000,
        "only the approved reports were paid for"
    );
}
