//! Receipts for which model answered. A session on a devnet with a
//! scripted model keeps a receipt of each answer; its operator commits to
//! them in a claim and a reader checks the file against it. An agent
//! rented from a simulated enclave signs its receipts with the key its
//! attestation binds, over the whole rental; the renter checks them and
//! keeps a file a third party can check, and a change to either file
//! shows.

use std::sync::{Arc, Mutex};

use serde_json::json;
use sion_agent::model::{Block, Message, Request, Response, Stop, Usage};
use sion_agent::receipt::{Purpose, Receipts};
use sion_agent::Autonomy;
use sion_cli::ledger::{Ledger, LedgerExt};
use sion_cli::profiles::Profile;
use sion_cli::receipts::{self, By, ReceiptsCommand, ReceiptsFile, Trusted};
use sion_cli::rentals::{self, EnclaveKeys, Listing, ModelAccount, RentalServer, RenterKey, Turn};
use sion_cli::runtime::SharedLedger;
use sion_cli::session::{wallet_for, Session};
use sion_cli::stream::{Payment, StreamDesk};
use sion_core::hash::Hash32;
use sion_core::ids::{AssetId, ObjectId, Pulse};
use sion_core::Amount;
use sion_enclave::simulated::SimulatedPlatform;
use sion_exec::charter::Caveats;
use sion_exec::execute::Event;
use sion_exec::TxKind;
use sion_node::{Devnet, DevnetConfig};

static ENVIRONMENT: Mutex<()> = Mutex::new(());

fn home(name: &str) -> std::path::PathBuf {
    let home = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    std::env::set_var("SION_HOME", &home);
    home
}

fn script(home: &std::path::Path, name: &str, steps: serde_json::Value) -> String {
    let path = home.join(format!("{name}.json"));
    std::fs::write(&path, steps.to_string()).unwrap();
    format!("script:{}", path.display())
}

fn devnet() -> (Arc<Devnet>, SharedLedger) {
    let devnet = Arc::new(
        Devnet::start(DevnetConfig {
            rounds_per_pulse: 2,
            prove_chunk: None,
            ..DevnetConfig::default()
        })
        .unwrap(),
    );
    let ledger: SharedLedger = devnet.clone();
    (devnet, ledger)
}

/// An agent of `owner`'s that may pay at most 10 USD at a time.
fn register(
    session: &mut Session<'_>,
    ledger: &SharedLedger,
    owner: &str,
    wallet: &str,
) -> ObjectId {
    let pulse = ledger.as_ref().pulse().unwrap();
    let account = session.id(wallet);
    let view = session
        .must(
            owner,
            TxKind::RegisterAgent {
                agent: account,
                card: Hash32::ZERO,
                caveats: Caveats {
                    payees: None,
                    max_per_payment: Amount::units(10),
                    burst: Amount::units(200),
                    rate: Amount(500_000),
                    expires_at: Pulse(pulse.0 + 100_000),
                    evidence_above: None,
                    evidence_policy: sion_assurance::Policy::standard(),
                    receipts: true,
                    may_delegate: false,
                },
                funding: Amount::units(300),
                agent_bond: Amount::ZERO,
            },
            Vec::new(),
        )
        .unwrap();
    session
        .fund(wallet, AssetId::usd(), Amount::units(1))
        .unwrap();
    view.effects
        .events
        .iter()
        .find_map(|e| match e {
            Event::AgentRegistered { agent, .. } => Some(*agent),
            _ => None,
        })
        .unwrap()
}

fn profile(name: &str, agent: ObjectId, models: Vec<String>, expect: bool) -> Profile {
    Profile {
        name: name.into(),
        wallet: name.into(),
        agent,
        models,
        autonomy: Autonomy::L6,
        memory: None,
        skills: None,
        workspace: None,
        web: false,
        delegate: false,
        subagent_models: Vec::new(),
        adjudicator: None,
        reflect: false,
        shell: None,
        mcp: Vec::new(),
        storage: false,
        instructions: Some("You keep the books.".into()),
        skill_approvals: true,
        screen: Default::default(),
        image_model: None,
        home_assistant: false,
        budget_tokens: None,
        budget_micros: None,
        max_turns: None,
        max_tokens: None,
        compact_at: None,
        summary_model: None,
        no_plan: false,
        no_change_check: false,
        expect,
        hear: None,
        rules: None,
        rules_model: None,
        embed: None,
        outside_files: Vec::new(),
        always_ask: Vec::new(),
        quarantine: false,
        shared_memory: Vec::new(),
        confidential: false,
        confidential_files: Vec::new(),
        confidential_tools: Vec::new(),
        routes: Vec::new(),
        sorter: None,
    }
}

#[test]
fn a_sessions_receipts_are_committed_to_by_its_operator_and_checked_by_a_reader() {
    let _one = ENVIRONMENT.lock().unwrap_or_else(|e| e.into_inner());
    let home = home("agent-receipts");
    let out = std::env::temp_dir().join(format!("agent-receipts-out-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(&out).unwrap();
    let (_devnet, ledger) = devnet();
    let mut session = Session::open(ledger.as_ref()).unwrap();
    session
        .fund("alice", AssetId::usd(), Amount::units(2_000))
        .unwrap();
    let agent = register(&mut session, &ledger, "alice", "clerk");
    let keeper = script(
        &home,
        "keeper",
        json!([
            {"call": "ledger_balance", "input": {}},
            {"say": "Done."}
        ]),
    );
    let (saved, _) = profile("clerk", agent, vec![keeper.clone()], false)
        .start(ledger.clone(), "What is your balance?")
        .unwrap();
    let kept = &saved.state.receipts;
    assert_eq!(kept.count, 2);
    kept.check().unwrap();
    assert!(kept.kept.iter().all(|r| r.asked == keeper));
    assert_eq!(kept.tallies.len(), 1);
    assert_eq!(kept.tallies[0].usage, saved.state.usage);

    let shown = receipts::lines(&saved.state.id, kept, 10).join("\n");
    assert!(shown.contains("2 answer(s) from models"), "{shown}");
    assert!(shown.contains("the last 2:"), "{shown}");

    // Written with no one behind them, they show only that they agree
    // with themselves.
    let plain = out.join("plain.json");
    receipts::run(
        None,
        ReceiptsCommand::Write {
            session: saved.state.id.clone(),
            out: plain.clone(),
            wallet: None,
        },
    )
    .unwrap();
    let read = |path: &std::path::Path| -> ReceiptsFile {
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    };
    let (lines, holds) = receipts::check(None, &read(&plain), &Trusted::default()).unwrap();
    assert!(!holds);
    assert!(lines
        .join("\n")
        .contains("no one stands behind these receipts"));

    // Alice commits to them. A reader checks the file against her claim.
    let file = out.join("receipts.json");
    receipts::run(
        Some((&mut session, ledger.as_ref())),
        ReceiptsCommand::Write {
            session: saved.state.id.clone(),
            out: file.clone(),
            wallet: Some("alice".into()),
        },
    )
    .unwrap();
    let committed = read(&file);
    assert!(matches!(committed.by, By::Operator { .. }));
    let (lines, holds) =
        receipts::check(Some(ledger.as_ref()), &committed, &Trusted::default()).unwrap();
    let text = lines.join("\n");
    assert!(holds, "{text}");
    assert!(text.contains("holds: 2 receipt(s) in a chain"), "{text}");
    assert!(
        text.contains(&format!(
            "holds: account {} committed to this head and these tallies in claim",
            session.id("alice").short()
        )),
        "{text}"
    );
    assert!(
        text.contains("its operator's word about its own machine"),
        "{text}"
    );
    receipts::run(
        Some((&mut session, ledger.as_ref())),
        ReceiptsCommand::Check {
            file: file.clone(),
            measurement: None,
            simulated_platform: None,
        },
    )
    .unwrap();

    // Changed afterwards, the file no longer is what she committed to: the
    // model that answered, the tokens, or a receipt left out.
    let mut renamed = committed.clone();
    renamed.receipts[0].answered = "a-better-model".into();
    let (lines, holds) =
        receipts::check(Some(ledger.as_ref()), &renamed, &Trusted::default()).unwrap();
    assert!(!holds && lines[0].starts_with("differs:"), "{lines:?}");
    let mut recounted = committed.clone();
    recounted.tallies[0].calls = 1;
    recounted.count = 1;
    recounted.receipts.remove(0);
    let (_, holds) =
        receipts::check(Some(ledger.as_ref()), &recounted, &Trusted::default()).unwrap();
    assert!(!holds);
    // A chain of other receipts that agrees with itself is not hers.
    let mut other = Receipts::default();
    other.add(
        Purpose::Turn,
        "big-1",
        None,
        &asked("x"),
        &answered("big-1", 1, 1),
    );
    let mut forged = ReceiptsFile::of_session(&saved.state.id, &other);
    forged.by = committed.by.clone();
    let (lines, holds) =
        receipts::check(Some(ledger.as_ref()), &forged, &Trusted::default()).unwrap();
    assert!(!holds);
    assert!(lines
        .join("\n")
        .contains("commits to other receipts than these"));
    let _ = (Pulse(0), Hash32::ZERO);
}

fn asked(text: &str) -> Request {
    Request {
        system: "You summarize.".into(),
        messages: vec![Message::user(text)],
        tools: Vec::new(),
        max_tokens: 500,
        images: Default::default(),
    }
}

fn answered(model: &str, input: u64, output: u64) -> Response {
    Response {
        content: vec![Block::text("ok")],
        stop: Stop::EndTurn,
        usage: Usage {
            input,
            output,
            ..Usage::default()
        },
        model: model.into(),
        forgot_reasoning: false,
    }
}

/// A rented agent's runtime, standing in: the model it is set up with
/// answers the first request twice, and a smaller one answers the second.
fn rented_turn(message: &str) -> Turn {
    let at = Some("https://api.example.com".to_string());
    let mut kept = Receipts::default();
    let mut usage = Usage::default();
    let models: &[&str] = if message.contains("again") {
        &["small-1"]
    } else {
        &["big-1-2026", "big-1-2026"]
    };
    for model in models {
        let response = answered(model, 500, 100);
        usage.add(&response.usage);
        kept.add(
            Purpose::Turn,
            "big-1",
            at.clone(),
            &asked(message),
            &response,
        );
    }
    Turn {
        text: format!("answered: {message}"),
        usage,
        receipts: kept.kept,
    }
}

#[test]
fn a_rented_agents_enclave_signs_which_models_answered_over_the_whole_rental() {
    let _one = ENVIRONMENT.lock().unwrap_or_else(|e| e.into_inner());
    std::env::remove_var("SSL_CERT_FILE");
    let devnet = Arc::new(Devnet::start(DevnetConfig::default()).unwrap());
    let ledger: &dyn Ledger = devnet.as_ref();
    let mut session = Session::open(ledger).unwrap();
    session
        .fund("owner3", AssetId::usd(), Amount::units(1))
        .unwrap();
    session
        .fund("renter3", AssetId::usd(), Amount::units(50))
        .unwrap();
    let owner = session.id("owner3");
    let agent = ObjectId::derive(b"receipted-agent");
    let keys = EnclaveKeys::generate();
    let terms = rentals::terms_digest(
        &agent,
        &owner,
        Amount(3_000),
        Amount(15_000),
        ModelAccount::Owner,
    );
    let platform = SimulatedPlatform::generate().unwrap();
    let measurement = [0x7C; 48];
    let attestation = platform
        .attest(keys.binding(&agent, terms, 100_000), measurement)
        .unwrap();
    let listing = Listing {
        name: "receipted".into(),
        agent,
        payee: owner,
        per_1k_input: Amount(3_000),
        per_1k_output: Amount(15_000),
        model_account: ModelAccount::Owner,
        unit: Amount(1_000),
        min_deposit: Amount::units(1),
        description: "says which model answered".into(),
        attestation,
    };
    let stream = sion_cli::rent_cli::open_stream(
        &mut session,
        "renter3",
        &listing,
        Amount::units(5),
        10_000,
    )
    .unwrap();
    let server = RentalServer::new(keys, listing.clone(), StreamDesk::new(wallet_for("owner3")));
    let renter = RenterKey::generate();
    let signing_key = listing.attestation.binding.signing_public_key.clone();
    let now = || ledger.pulse().unwrap();

    // The first request: the model the agent is set up with answers twice.
    let head = sion_cli::stream::load_stream(ledger, &stream).unwrap().head;
    let first = Payment {
        stream,
        position: 0,
        value: head,
    };
    let sealed = renter
        .seal_request(&listing, "summarize this", first, None)
        .unwrap();
    let reply = renter
        .open_reply(
            &server
                .serve_turn(ledger, now(), &sealed, |m| Ok(rented_turn(m)))
                .unwrap(),
        )
        .unwrap();
    let signed = reply.receipts.clone().unwrap();
    signed.verify(&signing_key).unwrap();
    assert_eq!((signed.count, signed.new.len()), (2, 2));
    assert_eq!(signed.of, stream.to_string());
    assert!(!signed.extra_roots);
    let mut kept = signed.new.clone();

    // The second: a smaller model answers, and the enclave says so.
    let payment = session.streams.pay(&stream, reply.usage.position).unwrap();
    let sealed = renter
        .seal_request(&listing, "and again", payment, None)
        .unwrap();
    let reply = renter
        .open_reply(
            &server
                .serve_turn(ledger, now(), &sealed, |m| Ok(rented_turn(m)))
                .unwrap(),
        )
        .unwrap();
    let signed = reply.receipts.clone().unwrap();
    signed.verify(&signing_key).unwrap();
    // One chain over the rental: the new receipt follows the first two.
    assert_eq!((signed.count, signed.new.len()), (3, 1));
    assert_eq!(signed.new[0].seq, 2);
    assert_eq!(signed.new[0].previous, kept[1].digest());
    assert_eq!(signed.new[0].answered, "small-1");
    assert_eq!(signed.tallies.len(), 2);
    let tokens: u64 = signed.tallies.iter().map(|t| t.usage.input).sum();
    assert_eq!(tokens, reply.usage.usage.input);
    kept.extend(signed.new.clone());

    // What the enclave signed cannot be given another answer.
    let mut renamed = signed.clone();
    renamed.new[0].answered = "big-1-2026".into();
    assert!(renamed.verify(&signing_key).is_err());
    let mut retallied = signed.clone();
    retallied.tallies[1].answered = "big-1-2026".into();
    assert!(retallied.verify(&signing_key).is_err());
    assert!(signed
        .verify(&EnclaveKeys::generate().signing.public().bytes)
        .is_err());

    // The renter's file, for someone else: the receipts, the enclave's
    // signature and its attestation.
    let file = ReceiptsFile::of_rental(&signed, kept.clone(), &listing.attestation);
    let trusted = Trusted {
        measurement: Some(hex::encode(measurement)),
        simulated_platform: Some(hex::encode(platform.public_key())),
    };
    let (lines, holds) = receipts::check(None, &file, &trusted).unwrap();
    let text = lines.join("\n");
    assert!(holds, "{text}");
    assert!(text.contains("holds: 3 receipt(s) in a chain"), "{text}");
    assert!(
        text.contains("holds: the key is bound to a simulated enclave"),
        "{text}"
    );
    assert!(
        text.contains("holds: that key signed this head and these tallies"),
        "{text}"
    );
    assert!(
        text.contains(
            "  big-1-2026 (asked for big-1) at https://api.example.com: 2 answer(s), 1000 tokens \
             in and 200 out"
        ),
        "{text}"
    );
    assert!(
        text.contains("  small-1 (asked for big-1) at https://api.example.com: 1 answer(s)"),
        "{text}"
    );
    // Only the last receipt shown: the chain's end still checks, and the
    // tallies count the rest.
    let partial = ReceiptsFile::of_rental(&signed, signed.new.clone(), &listing.attestation);
    let (lines, holds) = receipts::check(None, &partial, &trusted).unwrap();
    assert!(holds, "{lines:?}");
    assert!(lines[0].contains("the last 1 of 3 receipt(s)"), "{lines:?}");

    // A simulated enclave is taken only from a platform named outright,
    // and code of another measurement is not the code trusted.
    let (lines, holds) = receipts::check(None, &file, &Trusted::default()).unwrap();
    assert!(!holds && lines.join("\n").contains("the enclave does not check out"));
    let other_code = Trusted {
        measurement: Some(hex::encode([0x11; 48])),
        ..trusted.clone()
    };
    assert!(!receipts::check(None, &file, &other_code).unwrap().1);
    // A file changed after the enclave signed it.
    let mut changed = file.clone();
    changed.tallies[1].answered = "big-1-2026".into();
    changed.receipts[2].answered = "big-1-2026".into();
    assert!(!receipts::check(None, &changed, &trusted).unwrap().1);
    let mut quiet = file.clone();
    quiet.extra_roots = true;
    let (lines, holds) = receipts::check(None, &quiet, &trusted).unwrap();
    assert!(!holds, "{lines:?}");
    let _ = SharedLedger::clone;
}
