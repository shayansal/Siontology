//! What other implementations of ACP and AP2 check: the requests this
//! crate sends and the answers it serves, written out for
//! `scripts/interop/acp_schemas.py` to validate against ACP's published
//! JSON Schemas (API version 2026-04-17). Run it with
//! `SION_INTEROP_OUT=<dir> cargo test -p sion-cli --test interop -- --ignored`;
//! `scripts/interop/README.md` says the rest.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use sion_cli::acp::buyer::{self, Authenticator, Card};
use sion_cli::acp::card::{self, CardHandler, PspProcessor};
use sion_cli::acp::feeds::{self, FeedHost};
use sion_cli::acp::http::{self, Client, Response, Service};
use sion_cli::acp::psp::{Psp, SimulatedAcquirer};
use sion_cli::acp::three_ds::{self, ThreeDs};
use sion_cli::acp::webhooks::{self, Hook};
use sion_cli::acp::{self, Buyer, Catalog, Merchant, Request};
use sion_cli::ap2::SigningKey;
use sion_cli::ledger::Ledger;
use sion_cli::session::Session;
use sion_cli::x402::Facilitator;
use sion_core::Amount;
use sion_node::{Devnet, DevnetConfig};

const BUYER: &str = "buyer-token-0123456789";
const AGENT_AT_PSP: &str = "agent-psp-token-0123456789";
const MERCHANT_AT_PSP: &str = "merchant-psp-token-0123456789";
const AGENT_AT_3DS: &str = "agent-3ds-token-0123456789";
const FEED_TOKEN: &str = "shop-feed-token-0123456789";
const MERCHANT_ID: &str = "acct_research_desk";
const SECRET: &[u8] = b"whsec_interop_0123456789abcdef";

struct Unblock<'s>(&'s tiny_http::Server);

impl Drop for Unblock<'_> {
    fn drop(&mut self) {
        self.0.unblock();
    }
}

fn listen() -> (tiny_http::Server, String) {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let base = format!("http://{}", server.server_addr().to_ip().unwrap());
    (server, base)
}

struct Events(Mutex<Vec<Value>>);

impl Service for Events {
    fn answer(&self, request: &Request<'_>) -> Response {
        match webhooks::receive(request, SECRET, http::now()) {
            Ok(event) => {
                self.0.lock().unwrap().push(event);
                Response::json(200, json!({"received": true}))
            }
            Err(refused) => refused,
        }
    }
}

/// A sample: what was sent or answered, and the schema it should meet.
fn sample(samples: &mut Vec<Value>, name: &str, schema: &str, def: &str, body: &Value) {
    samples.push(json!({"name": name, "schema": schema, "def": def, "body": body}));
}

#[test]
#[ignore = "interop: writes ACP samples for scripts/interop/acp_schemas.py (set SION_INTEROP_OUT)"]
fn acp_samples_for_the_published_schemas() {
    let Some(out) = std::env::var_os("SION_INTEROP_OUT") else {
        eprintln!("skipped: set SION_INTEROP_OUT to a directory to write the samples to");
        return;
    };
    let devnet = Arc::new(Devnet::start(DevnetConfig::default()).unwrap());
    let ledger: &dyn Ledger = devnet.as_ref();
    let chain = devnet.shared.chain_id;
    let shop_id = Session::open(ledger).unwrap().id("shop");
    let catalog = Catalog::from_json(&json!({"offers": [
        {"id": "report", "name": "A research report", "description": "a cited report on a question", "price_usd": "4.50"},
        {"id": "review", "name": "A code review", "price_usd": "2.00"},
    ]}))
    .unwrap();
    let (psp_server, psp_base) = listen();
    let (acs_server, acs_base) = listen();
    let (hook_server, hook_base) = listen();
    let (shop_server, shop_base) = listen();
    let (feed_server, feed_base) = listen();
    let vault = Psp::new(
        &psp_base,
        vec![Client::new("agent", AGENT_AT_PSP).unwrap()],
        vec![(
            Client::new(MERCHANT_ID, MERCHANT_AT_PSP).unwrap(),
            MERCHANT_ID.into(),
        )],
        Box::new(SimulatedAcquirer),
    )
    .unwrap();
    let provider =
        ThreeDs::new(&acs_base, vec![Client::new("agent", AGENT_AT_3DS).unwrap()]).unwrap();
    let events = Events(Mutex::new(Vec::new()));
    let metadata = json!({
        "acquirer_details": {
            "acquirer_bin": "400000",
            "acquirer_country": "US",
            "acquirer_merchant_id": MERCHANT_ID,
            "merchant_name": "Research desk",
        },
        "directory_server": "visa",
        "flow_preference": {"type": "challenge", "challenge": {"type": "preferred"}},
    });
    let shop = Merchant::new(
        Facilitator::new(ledger, chain).unwrap(),
        shop_id,
        "Research desk",
        &shop_base,
        catalog.clone(),
        vec![Client::new("buyer", BUYER).unwrap()],
    )
    .unwrap()
    .with_ap2(SigningKey::generate(), sion_cli::ap2::Trust::new(), false)
    .with_card(CardHandler {
        processor: Box::new(PspProcessor::new("acp-psp", &psp_base, MERCHANT_AT_PSP).unwrap()),
        merchant_id: MERCHANT_ID.into(),
        accepted_brands: vec!["visa".into()],
        three_ds: Some(metadata),
    })
    .with_webhooks(
        Hook::new(
            &format!("{hook_base}/agentic_checkout/webhooks/order_events"),
            SECRET,
        )
        .unwrap(),
    );
    let feed_host = FeedHost::new(vec![Client::new("shop", FEED_TOKEN).unwrap()], None).unwrap();
    let mut samples = Vec::new();

    std::thread::scope(|scope| {
        for (service, server) in [
            (&vault as &(dyn Service + Sync), &psp_server),
            (&provider as &(dyn Service + Sync), &acs_server),
            (&events as &(dyn Service + Sync), &hook_server),
            (&shop as &(dyn Service + Sync), &shop_server),
            (&feed_host as &(dyn Service + Sync), &feed_server),
        ] {
            scope.spawn(move || acp::serve(service, server, None));
        }
        let _stops = [
            Unblock(&psp_server),
            Unblock(&acs_server),
            Unblock(&hook_server),
            Unblock(&shop_server),
            Unblock(&feed_server),
        ];
        let buyer = Buyer {
            base_url: &shop_base,
            token: BUYER,
            chain,
            max_price: Amount::units(10),
            signer: None,
        };
        const CHECKOUT: &str = "schema.agentic_checkout.json";

        let (_, discovery) = buyer.call("GET", "/.well-known/acp.json", None).unwrap();
        sample(
            &mut samples,
            "merchant discovery",
            CHECKOUT,
            "DiscoveryResponse",
            &discovery,
        );
        let (_, psp_discovery) = buyer::send(
            &format!("{psp_base}/.well-known/acp.json"),
            AGENT_AT_PSP,
            None,
            "GET",
            None,
        )
        .unwrap();
        sample(
            &mut samples,
            "PSP discovery",
            CHECKOUT,
            "DiscoveryResponse",
            &psp_discovery,
        );

        let create = buyer::checkout_request(&["report"]);
        sample(
            &mut samples,
            "create request",
            CHECKOUT,
            "CheckoutSessionCreateRequest",
            &create,
        );
        let (status, session) = buyer
            .call("POST", "/checkout_sessions", Some(&create))
            .unwrap();
        assert_eq!(status, 201, "{session}");
        sample(
            &mut samples,
            "session, ready for payment",
            CHECKOUT,
            "CheckoutSession",
            &session,
        );
        let id = session["id"].as_str().unwrap().to_string();
        let update = json!({"line_items": [{"id": "report"}, {"id": "review"}]});
        sample(
            &mut samples,
            "update request",
            CHECKOUT,
            "CheckoutSessionUpdateRequest",
            &update,
        );
        let (_, updated) = buyer
            .call("POST", &format!("/checkout_sessions/{id}"), Some(&update))
            .unwrap();
        sample(
            &mut samples,
            "session, updated",
            CHECKOUT,
            "CheckoutSession",
            &updated,
        );
        let (_, unknown) = buyer
            .call(
                "POST",
                "/checkout_sessions",
                Some(&buyer::checkout_request(&["nothing"])),
            )
            .unwrap();
        sample(&mut samples, "error", CHECKOUT, "Error", &unknown);

        // A card, delegated; 3-D Secure asked for, and met.
        let method = json!({
            "type": "card",
            "card_number_type": "fpan",
            "number": "4000000000003220",
            "exp_month": "12",
            "exp_year": "2030",
            "name": "Ada Lovelace",
            "cvc": "123",
            "display_card_funding_type": "credit",
            "display_brand": "visa",
            "display_last4": "3220",
            "metadata": {},
        });
        let card_given = Card {
            psp_url: &psp_base,
            psp_token: AGENT_AT_PSP,
            payment_method: &method,
            billing_address: None,
        };
        let handler = buyer::handler_of(&updated, card::HANDLER).unwrap().clone();
        let ledger = buyer::handler_of(&updated, acp::HANDLER).unwrap().clone();
        let on_ledger = buyer::ledger_completion(&ledger, "00ff");
        sample(
            &mut samples,
            "complete request, ledger",
            CHECKOUT,
            "CheckoutSessionCompleteRequest",
            &on_ledger,
        );
        let without = buyer::card_completion(&handler, "vt_none", None);
        let (_, asked) = buyer
            .call(
                "POST",
                &format!("/checkout_sessions/{id}/complete"),
                Some(&without),
            )
            .unwrap();
        sample(
            &mut samples,
            "session, authentication required",
            CHECKOUT,
            "CheckoutSession",
            &asked,
        );
        let delegation = buyer::delegation_request(&card_given, &updated).unwrap();
        sample(
            &mut samples,
            "delegate payment request",
            "schema.delegate_payment.json",
            "DelegatePaymentRequest",
            &delegation,
        );
        let (status, delegated) = buyer::send(
            &format!("{psp_base}/agentic_commerce/delegate_payment"),
            AGENT_AT_PSP,
            None,
            "POST",
            Some(&delegation),
        )
        .unwrap();
        assert_eq!(status, 201, "{delegated}");
        sample(
            &mut samples,
            "delegate payment response",
            "schema.delegate_payment.json",
            "DelegatePaymentResponse",
            &delegated,
        );
        let notify = format!("{acs_base}/agent/notify");
        let challenge = |c: &Value| buyer::answer_simulated_challenge(c, three_ds::SIMULATED_OTP);
        let authenticator = Authenticator {
            url: &acs_base,
            token: AGENT_AT_3DS,
            notification_url: &notify,
            channel: None,
            challenge: &challenge,
        };
        const AUTH: &str = "schema.delegate_authentication.json";
        let create_auth =
            buyer::authentication_request(&authenticator, &card_given, &updated).unwrap();
        sample(
            &mut samples,
            "authentication request",
            AUTH,
            "DelegateAuthenticationCreateRequest",
            &create_auth,
        );
        let ask = |method: &str, path: &str, body: Option<&Value>| {
            buyer::send(
                &format!("{acs_base}{path}"),
                AGENT_AT_3DS,
                None,
                method,
                body,
            )
            .unwrap()
        };
        let (_, made) = ask("POST", "/delegate_authentication", Some(&create_auth));
        sample(
            &mut samples,
            "authentication session, fingerprint",
            AUTH,
            "DelegateAuthenticationSession",
            &made,
        );
        let auth_id = made["authentication_session_id"]
            .as_str()
            .unwrap()
            .to_string();
        let authenticate = json!({"fingerprint_completion": "Y"});
        sample(
            &mut samples,
            "authenticate request",
            AUTH,
            "DelegateAuthenticationAuthenticateRequest",
            &authenticate,
        );
        let (_, challenged) = ask(
            "POST",
            &format!("/delegate_authentication/{auth_id}/authenticate"),
            Some(&authenticate),
        );
        sample(
            &mut samples,
            "authentication session, challenge",
            AUTH,
            "DelegateAuthenticationSession",
            &challenged,
        );
        challenge(&challenged["action"]["challenge"]).unwrap();
        let (_, result) = ask("GET", &format!("/delegate_authentication/{auth_id}"), None);
        sample(
            &mut samples,
            "authentication session, with result",
            AUTH,
            "DelegateAuthenticationSessionWithResult",
            &result,
        );
        let outcome = three_ds::checkout_result(&result);
        sample(
            &mut samples,
            "checkout authentication result",
            CHECKOUT,
            "AuthenticationResult",
            &outcome,
        );
        let complete =
            buyer::card_completion(&handler, delegated["id"].as_str().unwrap(), Some(&outcome));
        sample(
            &mut samples,
            "complete request, card",
            CHECKOUT,
            "CheckoutSessionCompleteRequest",
            &complete,
        );
        let (status, done) = buyer
            .call(
                "POST",
                &format!("/checkout_sessions/{id}/complete"),
                Some(&complete),
            )
            .unwrap();
        assert_eq!(
            (status, done["status"].as_str()),
            (200, Some("completed")),
            "{done}"
        );
        sample(
            &mut samples,
            "session, completed",
            CHECKOUT,
            "CheckoutSessionWithOrder",
            &done,
        );
        sample(&mut samples, "order", CHECKOUT, "Order", &done["order"]);

        // Canceled.
        let (_, other) = buyer
            .call(
                "POST",
                "/checkout_sessions",
                Some(&buyer::checkout_request(&["review"])),
            )
            .unwrap();
        let cancel = json!({});
        sample(
            &mut samples,
            "cancel request",
            CHECKOUT,
            "CancelSessionRequest",
            &cancel,
        );
        let (_, canceled) = buyer
            .call(
                "POST",
                &format!(
                    "/checkout_sessions/{}/cancel",
                    other["id"].as_str().unwrap()
                ),
                Some(&cancel),
            )
            .unwrap();
        sample(
            &mut samples,
            "session, canceled",
            CHECKOUT,
            "CheckoutSession",
            &canceled,
        );

        // A cart.
        let cart = buyer.cart(&[("report", 2)]).unwrap();
        sample(
            &mut samples,
            "cart create request",
            "schema.cart.json",
            "CartCreateRequest",
            &buyer::cart_request(&[("report", 2)]),
        );
        sample(&mut samples, "cart", "schema.cart.json", "Cart", &cart);
        let replaced = buyer
            .cart_replace(cart["id"].as_str().unwrap(), &[("review", 1)])
            .unwrap();
        sample(
            &mut samples,
            "cart update request",
            "schema.cart.json",
            "CartUpdateRequest",
            &buyer::cart_request(&[("review", 1)]),
        );
        sample(
            &mut samples,
            "cart, replaced",
            "schema.cart.json",
            "Cart",
            &replaced,
        );

        // The order's webhook.
        let deadline = Instant::now() + Duration::from_secs(30);
        while events.0.lock().unwrap().is_empty() {
            assert!(Instant::now() < deadline, "no webhook came");
            std::thread::sleep(Duration::from_millis(100));
        }
        let event = events.0.lock().unwrap()[0].clone();
        sample(
            &mut samples,
            "webhook event's order",
            CHECKOUT,
            "Order",
            &event["data"],
        );
        samples.push(json!({"name": "webhook event", "openapi": "openapi.agentic_checkout_webhook.yaml", "body": event}));

        // A feed.
        const FEED: &str = "schema.feed.json";
        let products = feeds::products_of(&catalog, "Research desk", &shop_base);
        for product in &products {
            sample(&mut samples, "feed product", FEED, "Product", product);
        }
        let create_feed = json!({"target_country": "US"});
        sample(
            &mut samples,
            "create feed request",
            FEED,
            "CreateFeedRequest",
            &create_feed,
        );
        let (_, feed) = buyer::send(
            &format!("{feed_base}/feeds"),
            FEED_TOKEN,
            None,
            "POST",
            Some(&create_feed),
        )
        .unwrap();
        sample(&mut samples, "feed metadata", FEED, "FeedMetadata", &feed);
        let upsert = json!({"products": products});
        sample(
            &mut samples,
            "upsert products request",
            FEED,
            "UpsertProductsRequest",
            &upsert,
        );
        let feed_id = feed["id"].as_str().unwrap();
        buyer::send(
            &format!("{feed_base}/feeds/{feed_id}/products"),
            FEED_TOKEN,
            None,
            "PATCH",
            Some(&upsert),
        )
        .unwrap();
        let (_, listed) = buyer::send(
            &format!("{feed_base}/feeds/{feed_id}/products"),
            FEED_TOKEN,
            None,
            "GET",
            None,
        )
        .unwrap();
        sample(
            &mut samples,
            "products response",
            FEED,
            "ProductsResponse",
            &listed,
        );
    });
    let dir = std::path::PathBuf::from(out);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("acp-samples.json");
    std::fs::write(&path, serde_json::to_string_pretty(&samples).unwrap()).unwrap();
    eprintln!("wrote {} samples to {}", samples.len(), path.display());
}

fn x5c_vector(name: &str) -> Vec<u8> {
    std::fs::read(format!(
        "{}/tests/vectors/x5c/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

#[test]
#[ignore = "interop: writes AP2 vectors for `scripts/interop/ap2_sdk.py verify` (set SION_INTEROP_OUT)"]
fn ap2_vectors_for_the_reference_sdk() {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;
    use sion_cli::ap2::{self, Trust};
    let Some(out) = std::env::var_os("SION_INTEROP_OUT") else {
        eprintln!("skipped: set SION_INTEROP_OUT to a directory to write the vectors to");
        return;
    };
    let devnet = Arc::new(Devnet::start(DevnetConfig::default()).unwrap());
    let ledger: &dyn Ledger = devnet.as_ref();
    let provider = SigningKey::generate();
    let agent = SigningKey::generate();
    let merchant_key = SigningKey::generate();
    let shop = Merchant::new(
        Facilitator::new(ledger, devnet.shared.chain_id).unwrap(),
        Session::open(ledger).unwrap().id("shop"),
        "Research desk",
        "https://shop.example",
        Catalog::from_json(&json!({"offers": [{"id": "report", "name": "A research report", "price_usd": "4.50"}]})).unwrap(),
        vec![Client::new("buyer", BUYER).unwrap()],
    )
    .unwrap()
    .with_ap2(SigningKey::from_pkcs8(&merchant_key.pkcs8()).unwrap(), Trust::new(), false);
    // A session the merchant made: its checkout JWT, audience and nonce.
    let session = shop
        .answer(&Request {
            method: "POST",
            path: "/checkout_sessions",
            headers: &[
                ("Authorization".to_string(), format!("Bearer {BUYER}")),
                ("API-Version".to_string(), acp::API_VERSION.to_string()),
                ("Idempotency-Key".to_string(), "k1".to_string()),
            ],
            body: &buyer::checkout_request(&["report"]).to_string(),
        })
        .body;
    let offered = &session["ap2"];
    let checkout_jwt = offered["checkout_jwt"].as_str().unwrap();
    let checkout_hash = ap2::hash(checkout_jwt);
    let (audience, nonce) = (
        offered["audience"].as_str().unwrap(),
        offered["nonce"].as_str().unwrap(),
    );
    let identity = &offered["merchant"];
    let now = http::now();
    let open = |vct: &str, constraints: Value| json!({"vct": vct, "constraints": constraints, "cnf": {"jwk": agent.public().jwk()}, "iat": now, "exp": now + 3600});
    let open_checkout = ap2::issue(
        &open(
            ap2::CHECKOUT_OPEN,
            json!([
                {"type": "checkout.allowed_merchants", "allowed": [{"id": identity["id"], "name": identity["name"]}]},
                {"type": "checkout.line_items", "items": [{"id": "line_1", "quantity": 1, "acceptable_items": [{"id": "report", "title": "A research report"}]}]},
            ]),
        ),
        &provider,
        "provider-1",
    );
    let open_checkout_hash = ap2::Token::parse(&open_checkout)
        .unwrap()
        .sd_hash()
        .unwrap();
    let close_checkout = |open: &str| {
        let closed = ap2::close(
            open,
            &json!({"vct": ap2::CHECKOUT, "checkout_jwt": checkout_jwt, "checkout_hash": checkout_hash}),
            &agent,
            audience,
            nonce,
            now,
        )
        .unwrap();
        ap2::chain(&[open, &closed])
    };
    let checkout_chain = close_checkout(&open_checkout);
    let open_payment = ap2::issue(
        &open(
            ap2::PAYMENT_OPEN,
            json!([
                {"type": "payment.amount_range", "currency": "USD", "max": 1000, "min": 100},
                {"type": "payment.allowed_payees", "allowed": [{"id": identity["id"], "name": identity["name"]}]},
                {"type": "payment.reference", "conditional_transaction_id": open_checkout_hash},
                {"type": "payment.agent_recurrence", "frequency": "ON_DEMAND", "max_occurrences": 2},
                {"type": "payment.budget", "currency": "USD", "max": 10.0},
            ]),
        ),
        &provider,
        "provider-1",
    );
    let closed_payment = ap2::close(
        &open_payment,
        &json!({
            "vct": ap2::PAYMENT,
            "transaction_id": checkout_hash,
            "payee": identity,
            "payment_amount": {"amount": 450, "currency": "USD"},
            "payment_instrument": {"id": "ledger", "type": acp::INSTRUMENT},
        }),
        &agent,
        audience,
        nonce,
        now,
    )
    .unwrap();
    let payment_chain = ap2::chain(&[&open_payment, &closed_payment]);
    // A root named by its certificate chain (the test certificates).
    let leaf_key = SigningKey::from_pkcs8(&x5c_vector("leaf.p8")).unwrap();
    let x5c_open = ap2::issue_x5c(
        &open(ap2::CHECKOUT_OPEN, json!([])),
        &leaf_key,
        &[x5c_vector("leaf.der"), x5c_vector("inter.der")],
    );
    let x5c_chain = close_checkout(&x5c_open);
    // The merchant's receipts, naming each closed mandate as the
    // specification has it (its sd_hash).
    let reference = |chain: &str| {
        ap2::split(chain)
            .unwrap()
            .last()
            .unwrap()
            .sd_hash()
            .unwrap()
    };
    let checkout_reference = reference(&checkout_chain);
    let payment_reference = reference(&payment_chain);
    let checkout_receipt = ap2::receipt(
        ap2::Kind::Checkout,
        &merchant_key,
        "https://shop.example",
        &checkout_reference,
        Ok(&json!({"order_id": "ord_1"})),
        now,
    );
    let payment_receipt = ap2::receipt(
        ap2::Kind::Payment,
        &merchant_key,
        "https://shop.example",
        &payment_reference,
        Ok(
            &json!({"payment_id": "tx_1", "psp_confirmation_id": "tx_1", "network_confirmation_id": "sion@1"}),
        ),
        now,
    );
    let mut issuer_jwk = provider.public().jwk();
    issuer_jwk["kid"] = json!("provider-1");
    let vectors = json!({
        "made_by": "Siontology sion-cli",
        "made_at": now,
        "issuer_jwk": issuer_jwk,
        "merchant_jwk": merchant_key.public().jwk(),
        "merchant": identity,
        "audience": audience,
        "nonce": nonce,
        "checkout_jwt": checkout_jwt,
        "checkout_hash": checkout_hash,
        "checkout_chain": checkout_chain,
        "open_checkout_hash": open_checkout_hash,
        "payment_chain": payment_chain,
        "x5c_root_der": STANDARD.encode(x5c_vector("root.der")),
        "rogue_root_der": STANDARD.encode(x5c_vector("rogue-root.der")),
        "x5c_chain": x5c_chain,
        "checkout_receipt": checkout_receipt,
        "checkout_reference": checkout_reference,
        "payment_receipt": payment_receipt,
        "payment_reference": payment_reference,
    });
    let dir = std::path::PathBuf::from(out);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("ap2-ours.json"),
        serde_json::to_string_pretty(&vectors).unwrap(),
    )
    .unwrap();
}

#[test]
#[ignore = "interop: verifies what `scripts/interop/ap2_sdk.py make` wrote (set SION_INTEROP_AP2_VECTORS)"]
fn ap2_reference_sdk_vectors_verify_here() {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;
    use sion_cli::ap2::{self, Expect, Payment, PublicKey, Trust};
    let Some(path) = std::env::var_os("SION_INTEROP_AP2_VECTORS") else {
        eprintln!("skipped: set SION_INTEROP_AP2_VECTORS to the file `ap2_sdk.py make` wrote");
        return;
    };
    let given: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let text = |field: &str| given[field].as_str().unwrap().to_string();
    let issuer = PublicKey::from_jwk(&given["issuer_jwk"]).unwrap();
    let trust = Trust::new().with(given["issuer_jwk"]["kid"].as_str().unwrap(), issuer);
    let merchant_key = PublicKey::from_jwk(&given["merchant_jwk"]).unwrap();
    let (audience, nonce) = (text("audience"), text("nonce"));
    let expect = Expect {
        audience: &audience,
        nonce: &nonce,
        now: given["made_at"].as_u64().unwrap(),
    };
    let checkout_jwt = text("checkout_jwt");
    let approved = ap2::verify_checkout(
        &text("checkout_chain"),
        &trust,
        expect,
        &checkout_jwt,
        &merchant_key,
    )
    .unwrap();
    assert_eq!(approved.checkout_hash, text("checkout_hash"));
    let payment = ap2::verify_payment(
        &text("payment_chain"),
        &trust,
        expect,
        &Payment {
            checkout_hash: &approved.checkout_hash,
            payee: &given["merchant"],
            amount: given["payment"]["amount"].as_u64().unwrap(),
            currency: given["payment"]["currency"].as_str().unwrap(),
            checkout_open: &approved.open,
            history: Some(&[]),
            now: expect.now,
        },
    );
    assert!(payment.is_ok(), "{payment:?}");
    let roots = Trust::new().with_root(STANDARD.decode(text("x5c_root_der")).unwrap());
    let x5c = ap2::verify_checkout(
        &text("x5c_chain"),
        &roots,
        expect,
        &checkout_jwt,
        &merchant_key,
    );
    assert!(x5c.is_ok(), "{x5c:?}");
    for receipt in ["checkout_receipt", "payment_receipt"] {
        let (_, claims) = ap2::verify_jws(&text(receipt), &merchant_key).unwrap();
        assert_eq!(claims["status"], "Success", "{receipt}");
    }
}
