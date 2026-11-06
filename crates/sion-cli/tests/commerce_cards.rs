//! Cards in an ACP checkout, end to end over HTTP: an agent delegates a
//! card to the merchant's PSP for one checkout, authenticates it with 3-D
//! Secure when the merchant or the issuer asks, and the merchant charges
//! the token; a merchant charges Stripe shared payment tokens (against a
//! stand-in for Stripe's API); and a merchant publishes its catalog to an
//! agent's Feed API. The PSP's acquirer and the 3-D Secure issuer are
//! simulated: no card network is reached.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use sion_cli::acp::buyer::{self, Authenticator, Card};
use sion_cli::acp::card::{self, CardHandler, PspProcessor, Stripe};
use sion_cli::acp::feeds::{self, FeedHost};
use sion_cli::acp::http::{self, Client, Response, Service};
use sion_cli::acp::psp::{Psp, SimulatedAcquirer};
use sion_cli::acp::state::Store;
use sion_cli::acp::three_ds::{self, ThreeDs};
use sion_cli::acp::{self, Buyer, Catalog, Merchant, Request};
use sion_cli::ledger::Ledger;
use sion_cli::session::Session;
use sion_cli::x402::Facilitator;
use sion_core::Amount;
use sion_node::{Devnet, DevnetConfig};

const BUYER: &str = "buyer-token-0123456789";
const AGENT_AT_PSP: &str = "agent-psp-token-0123456789";
const MERCHANT_AT_PSP: &str = "merchant-psp-token-0123456789";
const AGENT_AT_3DS: &str = "agent-3ds-token-0123456789";
const ADMIN: &str = "admin-token-0123456789";
const MERCHANT_ID: &str = "acct_research_desk";

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

fn catalog() -> Catalog {
    Catalog::from_json(&json!({"offers": [
        {"id": "report", "name": "A research report", "description": "a cited report on a question", "price_usd": "4.50"},
        {"id": "review", "name": "A code review", "price_usd": "2.00"},
    ]}))
    .unwrap()
}

/// A test card, as ACP's delegate payment API takes it.
fn card(number: &str) -> Value {
    json!({
        "type": "card",
        "card_number_type": "fpan",
        "number": number,
        "exp_month": "12",
        "exp_year": "2030",
        "name": "Ada Lovelace",
        "cvc": "123",
        "display_card_funding_type": "credit",
        "display_brand": "visa",
        "display_last4": &number[number.len() - 4..],
        "metadata": {},
    })
}

fn merchant<'l>(ledger: &'l dyn Ledger, base: &str, handler: CardHandler) -> Merchant<'l> {
    let chain = sion_cli::verify::served_chain(ledger).unwrap();
    let shop = Session::open(ledger).unwrap().id("shop");
    Merchant::new(
        Facilitator::new(ledger, chain).unwrap(),
        shop,
        "Research desk",
        base,
        catalog(),
        vec![Client::new("buyer", BUYER).unwrap()],
    )
    .unwrap()
    .with_card(handler)
    .with_admin(ADMIN)
    .unwrap()
}

fn psp_handler(psp: &str, three_ds: Option<Value>) -> CardHandler {
    CardHandler {
        processor: Box::new(PspProcessor::new("acp-psp", psp, MERCHANT_AT_PSP).unwrap()),
        merchant_id: MERCHANT_ID.into(),
        accepted_brands: vec!["visa".into(), "mastercard".into()],
        three_ds,
    }
}

fn psp(base: &str) -> Psp {
    Psp::new(
        base,
        vec![Client::new("agent", AGENT_AT_PSP).unwrap()],
        vec![(
            Client::new(MERCHANT_ID, MERCHANT_AT_PSP).unwrap(),
            MERCHANT_ID.into(),
        )],
        Box::new(SimulatedAcquirer),
    )
    .unwrap()
}

fn buyer_at(base: &str) -> Buyer<'_> {
    Buyer {
        base_url: base,
        token: BUYER,
        chain: Default::default(),
        max_price: Amount::units(10),
        signer: None,
    }
}

#[test]
fn an_agent_pays_by_a_card_delegated_to_the_merchants_psp() {
    let devnet = Arc::new(Devnet::start(DevnetConfig::default()).unwrap());
    let ledger: &dyn Ledger = devnet.as_ref();
    let (psp_server, psp_base) = listen();
    let (shop_server, shop_base) = listen();
    let vault = psp(&psp_base);
    let shop = merchant(ledger, &shop_base, psp_handler(&psp_base, None));

    std::thread::scope(|scope| {
        scope.spawn(|| acp::serve(&vault, &psp_server, None));
        let _stop_psp = Unblock(&psp_server);
        scope.spawn(|| acp::serve(&shop, &shop_server, None));
        let _stop_shop = Unblock(&shop_server);
        let buyer = buyer_at(&shop_base);
        let pay = |number: &str| {
            let method = card(number);
            let given = Card {
                psp_url: &psp_base,
                psp_token: AGENT_AT_PSP,
                payment_method: &method,
                billing_address: None,
            };
            acp::buy_by_card(&buyer, &["report"], &given, None)
        };

        // The session declares the card handler, scoped to this merchant.
        let offered = buyer.checkout(&["report"]).unwrap();
        let handler = buyer::handler_of(&offered, card::HANDLER).unwrap();
        assert_eq!(handler["requires_delegate_payment"], true);
        assert_eq!(handler["config"]["merchant_id"], MERCHANT_ID);

        // A good card is charged once, and an order made.
        let done = pay("4242424242424242").unwrap();
        assert_eq!(done["status"], "completed", "{done}");
        assert_eq!(done["order"]["status"], "confirmed");
        assert_eq!(vault.holdings(), (1, 1), "one token, used");
        // Declined cards make no order.
        let declined = pay("4000000000000002").unwrap_err().to_string();
        assert!(declined.contains("declined"), "{declined}");
        assert!(pay("4000000000009995").is_err());
        // A card the issuer wants authenticated, with no 3-D Secure provider
        // named, is not charged.
        let unauthenticated = pay("4000000000003220").unwrap_err().to_string();
        assert!(unauthenticated.contains("3-D Secure"), "{unauthenticated}");
        assert_eq!(shop.orders().len(), 1);

        // A token is for one checkout's total, once.
        let session = buyer.checkout(&["report", "review"]).unwrap();
        let method = card("4242424242424242");
        let given = Card {
            psp_url: &psp_base,
            psp_token: AGENT_AT_PSP,
            payment_method: &method,
            billing_address: None,
        };
        let token = buyer::delegate_card(&given, &session, None).unwrap();
        assert!(token.starts_with("vt_"));
        let complete = |id: &str, token: &str| {
            buyer
                .call(
                    "POST",
                    &format!("/checkout_sessions/{id}/complete"),
                    Some(&json!({"payment_data": {
                        "handler_id": card::HANDLER_ID,
                        "instrument": {"type": "card", "credential": {"type": "spt", "token": token}},
                    }})),
                )
                .unwrap()
                .1
        };
        let id = session["id"].as_str().unwrap();
        assert_eq!(complete(id, &token)["status"], "completed");
        let other = buyer.checkout(&["review"]).unwrap();
        let reused = complete(other["id"].as_str().unwrap(), &token);
        assert_eq!(reused["status"], "ready_for_payment");
        assert_eq!(reused["messages"][0]["code"], "payment_declined");
        // A token for less than the total is not charged for more.
        let small = buyer.checkout(&["review"]).unwrap();
        let (status, made) = buyer::send(
            &format!("{psp_base}/agentic_commerce/delegate_payment"),
            AGENT_AT_PSP,
            None,
            "POST",
            Some(&json!({
                "payment_method": card("4242424242424242"),
                "allowance": {
                    "reason": "one_time",
                    "max_amount": 100,
                    "currency": "usd",
                    "checkout_session_id": small["id"],
                    "merchant_id": MERCHANT_ID,
                    "expires_at": http::rfc3339(http::now() + 600),
                },
                "risk_signals": [{"type": "card_testing", "score": 1, "action": "authorized"}],
                "metadata": {},
            })),
        )
        .unwrap();
        assert_eq!(status, 201, "{made}");
        let over = complete(small["id"].as_str().unwrap(), made["id"].as_str().unwrap());
        assert_eq!(over["messages"][0]["code"], "payment_declined");
        assert!(over["messages"][0]["content"]
            .as_str()
            .unwrap()
            .contains("allowance"));
        // Only a merchant the PSP knows redeems a token.
        let (status, _) = buyer::send(
            &format!("{psp_base}/agentic_commerce/create_and_process_payment_intent"),
            AGENT_AT_PSP,
            None,
            "POST",
            Some(&json!({"vault_token": made["id"], "amount": 100, "currency": "usd"})),
        )
        .unwrap();
        assert_eq!(status, 401);

        // A card order is refunded at its PSP, which has no refund API here.
        let order = done["order"]["id"].as_str().unwrap();
        let client = http::outbound(true).unwrap();
        let bearer = format!("Bearer {ADMIN}");
        let (status, refused) = http::call(
            &client,
            "POST",
            &format!("{shop_base}/admin/orders/{order}/refund"),
            &[("Authorization", bearer.as_str())],
            Some(b"{}"),
        )
        .unwrap();
        assert_eq!(status, 502, "{refused}");
    });
    // No card number is kept by the merchant.
    let kept = serde_json::to_string(&shop.orders()).unwrap();
    assert!(!kept.contains("4242424242424242"));
}

/// The simulated issuer's challenge, met with `code`.
fn meet(code: &'static str) -> impl Fn(&Value) -> Result<(), sion_cli::error::CliError> {
    move |challenge: &Value| buyer::answer_simulated_challenge(challenge, code)
}

#[test]
fn three_d_secure_is_asked_for_and_met_before_a_card_is_charged() {
    let devnet = Arc::new(Devnet::start(DevnetConfig::default()).unwrap());
    let ledger: &dyn Ledger = devnet.as_ref();
    let (psp_server, psp_base) = listen();
    let (acs_server, acs_base) = listen();
    let (asking_server, asking_base) = listen();
    let (plain_server, plain_base) = listen();
    let vault = psp(&psp_base);
    let provider =
        ThreeDs::new(&acs_base, vec![Client::new("agent", AGENT_AT_3DS).unwrap()]).unwrap();
    let metadata = json!({
        "acquirer_details": {
            "acquirer_bin": "400000",
            "acquirer_country": "US",
            "acquirer_merchant_id": MERCHANT_ID,
            "merchant_name": "Research desk",
        },
        "directory_server": "visa",
        "flow_preference": {"type": "frictionless"},
    });
    // One merchant asks for 3-D Secure; the other does not, though an
    // issuer may.
    let asking = merchant(
        ledger,
        &asking_base,
        psp_handler(&psp_base, Some(metadata.clone())),
    );
    let plain = merchant(ledger, &plain_base, psp_handler(&psp_base, None));

    std::thread::scope(|scope| {
        for (service, server) in [
            (&vault as &(dyn Service + Sync), &psp_server),
            (&provider as &(dyn Service + Sync), &acs_server),
            (&asking as &(dyn Service + Sync), &asking_server),
            (&plain as &(dyn Service + Sync), &plain_server),
        ] {
            scope.spawn(move || acp::serve(service, server, None));
        }
        let _stops = [
            Unblock(&psp_server),
            Unblock(&acs_server),
            Unblock(&asking_server),
            Unblock(&plain_server),
        ];
        let notify = format!("{acs_base}/agent/notify");
        let buy = |shop: &str, number: &str, code: Option<&'static str>| {
            let method = card(number);
            let given = Card {
                psp_url: &psp_base,
                psp_token: AGENT_AT_PSP,
                payment_method: &method,
                billing_address: None,
            };
            let challenge = meet(code.unwrap_or("000000"));
            let authenticator = Authenticator {
                url: &acs_base,
                token: AGENT_AT_3DS,
                notification_url: &notify,
                channel: None,
                challenge: &challenge,
            };
            let buyer = buyer_at(shop);
            acp::buy_by_card(&buyer, &["report"], &given, code.map(|_| &authenticator))
        };

        // The merchant states what 3-D Secure needs.
        let offered = buyer_at(&asking_base).checkout(&["report"]).unwrap();
        assert_eq!(offered["authentication_metadata"], metadata);
        // Without a provider to authenticate with, nothing is charged.
        let said = buy(&asking_base, "4242424242424242", None)
            .unwrap_err()
            .to_string();
        assert!(said.contains("3-D Secure"), "{said}");

        // A challenge met with the issuer's code: authenticated, charged.
        let done = buy(
            &asking_base,
            "4000000000003220",
            Some(three_ds::SIMULATED_OTP),
        )
        .unwrap();
        assert_eq!(done["status"], "completed", "{done}");
        // Frictionless: authenticated without a challenge.
        let done = buy(
            &asking_base,
            "4000000000003063",
            Some(three_ds::SIMULATED_OTP),
        )
        .unwrap();
        assert_eq!(done["status"], "completed", "{done}");
        // A challenge failed: not charged.
        let said = buy(&asking_base, "4000000000003220", Some("999999"))
            .unwrap_err()
            .to_string();
        assert!(said.contains("denied"), "{said}");
        // Rejected by the issuer: not charged.
        let said = buy(
            &asking_base,
            "4000000000001091",
            Some(three_ds::SIMULATED_OTP),
        )
        .unwrap_err()
        .to_string();
        assert!(said.contains("rejected"), "{said}");
        assert_eq!(asking.orders().len(), 2);

        // The other merchant did not ask, but the issuer does: the session
        // asks for authentication, and the agent completes it again.
        let done = buy(
            &plain_base,
            "4000000000003220",
            Some(three_ds::SIMULATED_OTP),
        )
        .unwrap();
        assert_eq!(done["status"], "completed", "{done}");

        // The provider's own rules: a token, the API version, a valid card.
        let (status, _) = buyer::send(
            &format!("{acs_base}/delegate_authentication"),
            "nobody-0123456789abc",
            None,
            "POST",
            Some(&json!({})),
        )
        .unwrap();
        assert_eq!(status, 401);
        let (status, refused) = buyer::send(
            &format!("{acs_base}/delegate_authentication"),
            AGENT_AT_3DS,
            None,
            "POST",
            Some(&json!({
                "merchant_id": MERCHANT_ID,
                "payment_method": {"type": "card", "number": "4242424242424241", "exp_month": "12", "exp_year": "2030", "name": "A"},
                "amount": {"value": 450, "currency": "usd"},
            })),
        )
        .unwrap();
        assert_eq!(
            (status, refused["param"].as_str()),
            (400, Some("$.payment_method.number"))
        );
    });
    // The 3-D Secure results the merchant charged with were simulated, and
    // said so: the PSP's acquirer saw them, nothing else did.
    assert_eq!(vault.holdings().1, 3);
}

/// A request the stand-in was sent: its path, its form fields, its headers.
type Asked = (String, BTreeMap<String, String>, Vec<(String, String)>);

/// Stripe's API, as much of it as a merchant charging shared payment
/// tokens uses: what was asked is kept.
struct StripeStandIn {
    asked: Mutex<Vec<Asked>>,
}

fn form_of(body: &str) -> BTreeMap<String, String> {
    fn decode(text: &str) -> String {
        let bytes = text.replace('+', " ").into_bytes();
        let mut out = Vec::new();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'%' && i + 2 < bytes.len() {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap();
                out.push(u8::from_str_radix(hex, 16).unwrap());
                i += 3;
            } else {
                out.push(bytes[i]);
                i += 1;
            }
        }
        String::from_utf8(out).unwrap()
    }
    body.split('&')
        .filter_map(|pair| pair.split_once('='))
        .map(|(k, v)| (decode(k), decode(v)))
        .collect()
}

impl Service for StripeStandIn {
    fn answer(&self, request: &Request<'_>) -> Response {
        let fields = form_of(request.body);
        let headers: Vec<(String, String)> = request.headers.to_vec();
        self.asked
            .lock()
            .unwrap()
            .push((request.path.to_string(), fields.clone(), headers));
        let authorized =
            request.header("Authorization") == Some("Basic c2tfdGVzdF9zdGFuZGluXzAxMjM0NTY3ODk6");
        if !authorized {
            return Response::json(
                401,
                json!({"error": {"type": "invalid_request_error", "message": "Invalid API Key provided"}}),
            );
        }
        match request.path {
            "/v1/payment_intents" => {
                let token = fields
                    .get("payment_method_data[shared_payment_granted_token]")
                    .cloned()
                    .unwrap_or_default();
                match token.as_str() {
                    "spt_needs_auth"
                        if !fields.contains_key(
                            "payment_method_options[card][three_d_secure][cryptogram]",
                        ) =>
                    {
                        Response::json(
                            200,
                            json!({"id": "pi_auth", "object": "payment_intent", "status": "requires_action"}),
                        )
                    }
                    "spt_declined" => Response::json(
                        402,
                        json!({"error": {"type": "card_error", "code": "card_declined", "message": "Your card was declined."}}),
                    ),
                    _ => Response::json(
                        200,
                        json!({"id": "pi_standin_1", "object": "payment_intent", "status": "succeeded"}),
                    ),
                }
            }
            "/v1/refunds" => Response::json(
                200,
                json!({"id": "re_standin_1", "object": "refund", "status": "succeeded"}),
            ),
            _ => Response::json(
                404,
                json!({"error": {"type": "invalid_request_error", "message": "Unrecognized request URL"}}),
            ),
        }
    }
}

#[test]
fn a_merchant_charges_stripe_shared_payment_tokens() {
    let devnet = Arc::new(Devnet::start(DevnetConfig::default()).unwrap());
    let ledger: &dyn Ledger = devnet.as_ref();
    let (stripe_server, stripe_base) = listen();
    let stripe = StripeStandIn {
        asked: Mutex::new(Vec::new()),
    };
    let shop = merchant(
        ledger,
        "http://shop.invalid",
        CardHandler {
            processor: Box::new(
                Stripe::new("sk_test_standin_0123456789", Some(&stripe_base)).unwrap(),
            ),
            merchant_id: MERCHANT_ID.into(),
            accepted_brands: vec!["visa".into()],
            three_ds: None,
        },
    );
    let headers = |key: &str| {
        vec![
            ("Authorization".to_string(), format!("Bearer {BUYER}")),
            ("API-Version".to_string(), acp::API_VERSION.to_string()),
            ("Idempotency-Key".to_string(), key.to_string()),
        ]
    };
    let ask = |path: &str, key: &str, body: Value| {
        shop.answer(&Request {
            method: "POST",
            path,
            headers: &headers(key),
            body: &body.to_string(),
        })
    };
    let pay = |key: &str, token: &str, authentication: Option<Value>| {
        let made = ask(
            "/checkout_sessions",
            &format!("{key}-s"),
            json!({"currency": "usd", "line_items": [{"id": "report"}]}),
        );
        let id = made.body["id"].as_str().unwrap().to_string();
        let mut body = json!({"payment_data": {
            "handler_id": card::HANDLER_ID,
            "instrument": {"type": "card", "credential": {"type": "spt", "token": token}},
        }});
        if let Some(result) = authentication {
            body["authentication_result"] = result;
        }
        ask(
            &format!("/checkout_sessions/{id}/complete"),
            &format!("{key}-c"),
            body,
        )
        .body
    };

    std::thread::scope(|scope| {
        scope.spawn(|| acp::serve(&stripe, &stripe_server, None));
        let _stop = Unblock(&stripe_server);

        let done = pay("a", "spt_test_123", None);
        assert_eq!(done["status"], "completed", "{done}");
        let declared = &done["order"];
        assert_eq!(declared["status"], "confirmed");
        {
            let asked = stripe.asked.lock().unwrap();
            let (path, fields, headers) = &asked[0];
            assert_eq!(path, "/v1/payment_intents");
            assert_eq!(fields["amount"], "450");
            assert_eq!(fields["currency"], "usd");
            assert_eq!(fields["confirm"], "true");
            assert_eq!(
                fields["payment_method_data[shared_payment_granted_token]"],
                "spt_test_123"
            );
            let header = |name: &str| {
                headers
                    .iter()
                    .find(|(n, _)| n.eq_ignore_ascii_case(name))
                    .map(|(_, v)| v.as_str())
            };
            assert_eq!(header("Stripe-Version"), Some(card::STRIPE_VERSION));
            assert!(header("Idempotency-Key").is_some_and(|k| !k.is_empty()));
        }
        // Declined: no order.
        let declined = pay("b", "spt_declined", None);
        assert_eq!(declined["status"], "ready_for_payment");
        assert!(declined["messages"][0]["content"]
            .as_str()
            .unwrap()
            .contains("declined"));
        // The issuer asks for 3-D Secure: the session says so; with a
        // result, it is imported into the PaymentIntent.
        let asked_for = pay("c", "spt_needs_auth", None);
        assert_eq!(asked_for["status"], "authentication_required");
        assert_eq!(asked_for["messages"][0]["code"], "requires_3ds");
        let result = json!({
            "outcome": "authenticated",
            "outcome_details": {
                "three_ds_cryptogram": "AAABBBCCCDDDEEEFFFGGGHHHIII=",
                "electronic_commerce_indicator": "05",
                "transaction_id": "8f2e8c34-1f2a-4f0a-a5d5-4d1c8f0a1b2c",
                "version": "2.2.0",
            },
        });
        let authenticated = pay("d", "spt_needs_auth", Some(result));
        assert_eq!(authenticated["status"], "completed", "{authenticated}");
        {
            let asked = stripe.asked.lock().unwrap();
            let (_, fields, _) = asked.last().unwrap();
            assert_eq!(
                fields["payment_method_options[card][three_d_secure][ares_trans_status]"],
                "Y"
            );
            assert_eq!(
                fields
                    ["payment_method_options[card][three_d_secure][electronic_commerce_indicator]"],
                "05"
            );
        }

        // A refund is a Stripe refund of the PaymentIntent.
        let order = done["order"]["id"].as_str().unwrap();
        let refunded = shop.answer(&Request {
            method: "POST",
            path: &format!("/admin/orders/{order}/refund"),
            headers: &[("Authorization".to_string(), format!("Bearer {ADMIN}"))],
            body: r#"{"amount": 200}"#,
        });
        assert_eq!(refunded.status, 200, "{}", refunded.body);
        assert_eq!(refunded.body["adjustments"][0]["amount"], 200);
        let asked = stripe.asked.lock().unwrap();
        let (path, fields, _) = asked.last().unwrap();
        assert_eq!(path, "/v1/refunds");
        assert_eq!(fields["payment_intent"], "pi_standin_1");
        assert_eq!(fields["amount"], "200");
    });
}

#[test]
fn a_catalog_is_published_to_an_agents_feed_and_found_there() {
    const FEED_TOKEN: &str = "shop-feed-token-0123456789";
    let state = std::env::temp_dir().join(format!("sion-acp-feeds-{}.json", std::process::id()));
    let _ = std::fs::remove_file(&state);
    let (server, base) = listen();
    let host = FeedHost::new(
        vec![Client::new("shop", FEED_TOKEN).unwrap()],
        Some(Store::new(state.clone())),
    )
    .unwrap();
    let products = feeds::products_of(&catalog(), "Research desk", "https://shop.example");
    let feed = std::thread::scope(|scope| {
        scope.spawn(|| acp::serve(&host, &server, None));
        let _stop = Unblock(&server);
        let feed = feeds::publish(&base, FEED_TOKEN, None, Some("US"), &products).unwrap();
        // Upserted again into the same feed: still two products.
        assert_eq!(
            feeds::publish(&base, FEED_TOKEN, Some(feed.as_str()), None, &products).unwrap(),
            feed
        );
        let (status, listed) = buyer::send(
            &format!("{base}/feeds/{feed}/products"),
            FEED_TOKEN,
            None,
            "GET",
            None,
        )
        .unwrap();
        assert_eq!(status, 200);
        assert_eq!(listed["products"].as_array().unwrap().len(), 2);
        let (status, metadata) = buyer::send(
            &format!("{base}/feeds/{feed}"),
            FEED_TOKEN,
            None,
            "GET",
            None,
        )
        .unwrap();
        assert_eq!(
            (status, metadata["target_country"].as_str()),
            (200, Some("US"))
        );
        // A product without variants is refused, by its place.
        let (status, refused) = buyer::send(
            &format!("{base}/feeds/{feed}/products"),
            FEED_TOKEN,
            None,
            "PATCH",
            Some(&json!({"products": [products[0].clone(), {"id": "bare"}]})),
        )
        .unwrap();
        assert_eq!(
            (status, refused["param"].as_str()),
            (400, Some("$.products[1].variants"))
        );
        let (status, _) = buyer::send(
            &format!("{base}/feeds/feed_nothing/products"),
            FEED_TOKEN,
            None,
            "GET",
            None,
        )
        .unwrap();
        assert_eq!(status, 404);
        feed
    });
    // The agent finds the report, and its variant is the checkout item.
    let found = host.search("research report");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].0, feed);
    assert_eq!(found[0].1["variants"][0]["id"], "report");
    drop(host);
    // Kept across a restart.
    let again = FeedHost::new(
        vec![Client::new("shop", FEED_TOKEN).unwrap()],
        Some(Store::new(state.clone())),
    )
    .unwrap();
    assert_eq!(again.search("code review").len(), 1);
    let _ = std::fs::remove_file(&state);
}
