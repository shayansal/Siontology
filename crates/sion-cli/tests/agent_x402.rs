//! An agent pays for what the web sells: a server's x402 quote is paid
//! from the agent's budget, under its charter, and the page fetched again
//! with the payment. Nothing is paid above what the call allows, to a payee
//! or of an amount the charter refuses, on another ledger, or for a quote
//! that asks the payment to say something else; an identical call is not
//! paid twice; and a payment whose answer was lost is found on the ledger.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use sion_agent::{Autonomy, Ended, Policy, Tool};
use sion_cli::agent_x402::FetchPaid;
use sion_cli::runtime::{self, Acting, Equipment, SharedLedger};
use sion_cli::session::Session;
use sion_cli::x402::{encode_header, network, Facilitator, PaidRoute, PaidServer};
use sion_core::ids::{AssetId, Pulse};
use sion_core::Amount;
use sion_exec::charter::Caveats;
use sion_exec::execute::Event;
use sion_exec::TxKind;
use sion_node::{Devnet, DevnetConfig};

/// What a stub answers a path with: the path, a status, headers, a body.
type Answer = (&'static str, u16, Vec<(&'static str, String)>, &'static str);

/// A server that answers each path as told.
fn stub(routes: Vec<Answer>) -> String {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let base = format!("http://{}", server.server_addr());
    std::thread::spawn(move || {
        for request in server.incoming_requests() {
            let found = routes.iter().find(|(path, ..)| request.url() == *path);
            let response = match found {
                Some((_, status, headers, body)) => {
                    let mut response =
                        tiny_http::Response::from_string(*body).with_status_code(*status);
                    for (name, value) in headers {
                        response.add_header(
                            format!("{name}: {value}")
                                .parse::<tiny_http::Header>()
                                .unwrap(),
                        );
                    }
                    response
                }
                None => tiny_http::Response::from_string("not found").with_status_code(404),
            };
            let _ = request.respond(response);
        }
    });
    base
}

fn route(path: &str, micros: u128, body: &'static str) -> PaidRoute {
    PaidRoute {
        path: path.into(),
        price: Amount(micros),
        description: "a quote".into(),
        mime_type: "text/plain".into(),
        body: Box::new(move |_| body.into()),
    }
}

#[test]
fn an_agent_pays_a_quote_within_its_charter_and_nothing_else() {
    let home = std::env::temp_dir().join(format!("sion-agent-x402-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    // The only test in this binary, so the environment is its own.
    std::env::set_var("SION_HOME", &home);
    // The servers are on this machine.
    std::env::set_var("SION_X402_LOCAL", "1");

    let devnet = Arc::new(
        Devnet::start(DevnetConfig {
            rounds_per_pulse: 2,
            prove_chunk: None,
            ..DevnetConfig::default()
        })
        .unwrap(),
    );
    let ledger: SharedLedger = devnet.clone();
    let chain = devnet.shared.chain_id;
    let mut session = Session::open(ledger.as_ref()).unwrap();
    session
        .fund("alice", AssetId::usd(), Amount::units(1_000))
        .unwrap();

    // alice gives agent-9 an identity: it may pay only carol, at most 1 USD
    // a payment, from a budget of 10 USD.
    let carol = session.id("carol");
    let bob = session.id("bob");
    let account = session.id("agent-9");
    let pulse = sion_cli::ledger::LedgerExt::pulse(ledger.as_ref()).unwrap();
    let view = session
        .must(
            "alice",
            TxKind::RegisterAgent {
                agent: account,
                card: sion_core::hash::Hash32::ZERO,
                caveats: Caveats {
                    payees: Some([carol].into()),
                    max_per_payment: Amount::units(1),
                    burst: Amount::units(10),
                    rate: Amount(50_000),
                    expires_at: Pulse(pulse.0 + 100_000),
                    evidence_above: None,
                    evidence_policy: sion_assurance::Policy::standard(),
                    receipts: true,
                    may_delegate: false,
                },
                funding: Amount::units(10),
                agent_bond: Amount::ZERO,
            },
            Vec::new(),
        )
        .unwrap();
    let agent = view
        .effects
        .events
        .iter()
        .find_map(|e| match e {
            Event::AgentRegistered { agent, .. } => Some(*agent),
            _ => None,
        })
        .expect("an agent was registered");
    session
        .fund("agent-9", AssetId::usd(), Amount::units(1))
        .unwrap();

    let acting = Acting {
        ledger: ledger.clone(),
        wallet: "agent-9".into(),
        agent,
        evidence: None,
        project: None,
    };
    let tool = FetchPaid::new(acting, true).unwrap();
    assert_eq!(tool.effect(), sion_agent::Effect::Spend);
    assert_eq!(tool.dedup(), sion_agent::Dedup::Reconciled);

    // What other servers ask: nothing; a payment on another ledger; one
    // whose purpose is not the quote's own; one of no amount.
    let quote = |network: String, chain_hex: String, amount: &str, extra: Value| {
        let mut extra = extra;
        extra["chain"] = json!(chain_hex);
        encode_header(&json!({
            "x402Version": 2,
            "resource": {"url": "/x"},
            "accepts": [{"scheme": "exact", "network": network, "asset": "USD",
                "amount": amount, "payTo": carol.to_string(), "maxTimeoutSeconds": 60,
                "extra": extra}]
        }))
    };
    let ours = network(&chain);
    let other = stub(vec![
        ("/free", 200, vec![], "free words"),
        (
            "/elsewhere",
            402,
            vec![(
                "PAYMENT-REQUIRED",
                quote(
                    "eip155:8453".into(),
                    "00".repeat(32),
                    "50000",
                    json!({"nonce": "n-1"}),
                ),
            )],
            "",
        ),
        (
            "/purpose",
            402,
            vec![(
                "PAYMENT-REQUIRED",
                quote(
                    ours.clone(),
                    chain.to_hex(),
                    "50000",
                    json!({"nonce": "n-2", "purpose": "a gift to mallory"}),
                ),
            )],
            "",
        ),
        (
            "/nothing",
            402,
            vec![(
                "PAYMENT-REQUIRED",
                quote(ours.clone(), chain.to_hex(), "0", json!({"nonce": "n-3"})),
            )],
            "",
        ),
        ("/bare", 402, vec![], "pay up"),
    ]);

    let facilitator = Facilitator::new(devnet.as_ref(), chain).unwrap();
    let carols = PaidServer::bind("127.0.0.1:0").unwrap();
    let bobs = PaidServer::bind("127.0.0.1:0").unwrap();
    let routes = [
        route("/quote", 500_000, "42"),
        route("/dear", 5_000_000, "gold"),
        route("/again", 250_000, "7"),
    ];
    let stop = AtomicBool::new(false);
    let at = |server: &PaidServer, path: &str| format!("http://{}{path}", server.address);
    let balance =
        |session: &mut Session<'_>, who: &str| session.balance(who, AssetId::usd()).unwrap().0;
    std::thread::scope(|scope| {
        let serving = [
            scope.spawn(|| {
                carols.serve(&facilitator, carol, &routes, Duration::from_secs(60), &stop)
            }),
            scope.spawn(|| bobs.serve(&facilitator, bob, &routes, Duration::from_secs(60), &stop)),
        ];
        let before = balance(&mut session, "carol");
        let call = |url: String, most: &str, key: &str| {
            tool.call(&json!({"url": url, "max_usd": most}), key)
        };

        // A page that asks for nothing is fetched without paying.
        let free = call(format!("{other}/free"), "1", "k-free");
        assert!(!free.is_error && free.untrusted, "{free:?}");
        assert_eq!(free.content, "free words");

        // More than the call allows: not paid.
        let dear = call(at(&carols, "/quote"), "0.25", "k-low");
        assert!(dear.is_error && dear.certain, "{dear:?}");
        assert!(
            dear.content
                .contains("asks 0.5 USD, more than the 0.25 USD this call allows"),
            "{}",
            dear.content
        );
        // More than the charter allows a payment, whatever the call allows.
        let above = call(at(&carols, "/dear"), "10", "k-above");
        assert!(
            above.is_error
                && above
                    .content
                    .contains("above your charter's limit of 1 USD a payment"),
            "{above:?}"
        );
        // A payee the charter does not allow.
        let stranger = call(at(&bobs, "/quote"), "1", "k-bob");
        assert!(
            stranger.is_error
                && stranger
                    .content
                    .contains("which your charter does not let you pay"),
            "{stranger:?}"
        );
        // Another ledger, another purpose, no amount, no quote at all.
        let elsewhere = call(format!("{other}/elsewhere"), "1", "k-else");
        assert!(
            elsewhere.is_error
                && elsewhere.content.contains("but not in USD on your ledger")
                && elsewhere.content.contains("exact in USD on eip155:8453"),
            "{elsewhere:?}"
        );
        let purpose = call(format!("{other}/purpose"), "1", "k-purpose");
        assert!(
            purpose.is_error
                && purpose
                    .content
                    .contains("asks for a purpose other than its own nonce"),
            "{purpose:?}"
        );
        let nothing = call(format!("{other}/nothing"), "1", "k-nothing");
        assert!(
            nothing.is_error && nothing.content.contains("is not an amount"),
            "{nothing:?}"
        );
        let bare = call(format!("{other}/bare"), "1", "k-bare");
        assert!(
            bare.is_error && bare.content.contains("without saying how"),
            "{bare:?}"
        );
        assert_eq!(
            balance(&mut session, "carol"),
            before,
            "nothing was paid so far"
        );

        // Within both: paid, and served.
        let mut notes = Vec::new();
        let input = json!({"url": at(&carols, "/quote"), "max_usd": "1"});
        let paid = tool.call_noting(&input, "k-paid", false, &mut |note| {
            notes.push(note.to_string())
        });
        assert!(!paid.is_error && paid.certain && paid.untrusted, "{paid:?}");
        assert!(
            paid.content.starts_with("Paid 0.5 USD to "),
            "{}",
            paid.content
        );
        assert!(paid.content.ends_with("\n\n42"), "{}", paid.content);
        assert_eq!(balance(&mut session, "carol") - before, 500_000);
        // It noted its transaction before it was sent: had the answer been
        // lost, the ledger says it was paid, and it is not paid again.
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(
            notes[0].starts_with("transaction ") && notes[0].contains(" may execute until Pulse "),
            "{notes:?}"
        );
        let found = tool
            .reconcile(&input, "k-paid", &notes)
            .expect("found on the ledger");
        assert!(found.certain && found.is_error, "{found:?}");
        assert!(
            found.content.starts_with("paid, but what ")
                && found.content.contains("Do not pay for it again"),
            "{}",
            found.content
        );
        let never = vec![format!(
            "transaction {} may execute until Pulse 1",
            "00".repeat(32)
        )];
        let gone = tool
            .reconcile(&input, "k-paid", &never)
            .expect("past its last Pulse");
        assert!(
            gone.certain && gone.is_error && gone.content.starts_with("not paid"),
            "{gone:?}"
        );

        // In a run: the tool comes with the web, a model calls it, and the
        // same call made again is answered from the journal, not paid twice.
        let script = home.join("buy.json");
        let url = at(&carols, "/again");
        std::fs::write(
            &script,
            json!([
                {"call": "fetch_paid", "input": {"url": url, "max_usd": "0.50"}},
                {"call": "fetch_paid", "input": {"url": url, "max_usd": "0.50"}},
                {"say": "It says 7."}
            ])
            .to_string(),
        )
        .unwrap();
        let (saved, lines) = runtime::start(
            ledger.clone(),
            "agent-9",
            agent,
            "What does the quote service say?",
            vec![format!("script:{}", script.display())],
            Policy {
                autonomy: Autonomy::L6,
                ..Policy::default()
            },
            Equipment {
                web: true,
                ..Default::default()
            },
        )
        .unwrap();
        let text = lines.join("\n");
        assert!(matches!(saved.ended, Some(Ended::Done { .. })), "{text}");
        assert_eq!(text.matches("Paid 0.25 USD to ").count(), 2, "{text}");
        assert_eq!(
            balance(&mut session, "carol") - before,
            750_000,
            "the second call was answered from the journal:\n{text}"
        );
        assert!(saved.system.contains("fetch_paid"), "{}", saved.system);

        // At L5, paying waits for a person, who sees the address and the
        // most it may pay.
        std::fs::write(
            &script,
            json!([
                {"call": "fetch_paid", "input": {"url": at(&carols, "/quote"), "max_usd": "0.60"}},
                {"say": "Bought."}
            ])
            .to_string(),
        )
        .unwrap();
        let (waiting, lines) = runtime::start(
            ledger.clone(),
            "agent-9",
            agent,
            "Buy the quote.",
            vec![format!("script:{}", script.display())],
            Policy {
                autonomy: Autonomy::L5,
                ..Policy::default()
            },
            Equipment {
                web: true,
                ..Default::default()
            },
        )
        .unwrap();
        let text = lines.join("\n");
        assert!(
            matches!(waiting.ended, Some(Ended::NeedsApproval { .. })),
            "{text}"
        );
        assert!(text.contains("0.60") && text.contains("/quote"), "{text}");
        assert_eq!(balance(&mut session, "carol") - before, 750_000);

        stop.store(true, Ordering::Relaxed);
        for server in serving {
            server.join().unwrap();
        }
    });
    std::fs::remove_dir_all(&home).unwrap();
}
