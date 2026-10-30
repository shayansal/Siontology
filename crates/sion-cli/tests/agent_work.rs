//! Agents working with others, on a devnet: an agent spawns a subagent
//! with a budget carved from its own, which cannot spend more than it was
//! given; it hires another agent through a Task Charter, which delivers
//! and is paid; a scheduled job runs and reports to a channel; and people
//! reach a profile over HTTP and Telegram (against a stand-in for its API).

use std::sync::{mpsc, Arc};

use serde_json::{json, Value};
use sion_agent::schedule::When;
use sion_agent::{Autonomy, Ended, Policy};
use sion_assurance::AtomClass;
use sion_cli::ledger::LedgerExt;
use sion_cli::profiles::Profile;
use sion_cli::runtime::{self, Equipment, SharedLedger};
use sion_cli::session::Session;
use sion_core::ids::{AssetId, ObjectId, Pulse};
use sion_core::Amount;
use sion_exec::charter::Caveats;
use sion_exec::execute::Event;
use sion_exec::object::ObjectData;
use sion_exec::TxKind;
use sion_node::{Devnet, DevnetConfig};

fn register(
    session: &mut Session<'_>,
    ledger: &SharedLedger,
    principal: &str,
    wallet: &str,
    funding: u64,
) -> ObjectId {
    let pulse = ledger.as_ref().pulse().unwrap();
    let account = session.id(wallet);
    let view = session
        .must(
            principal,
            TxKind::RegisterAgent {
                agent: account,
                card: sion_core::hash::Hash32::ZERO,
                caveats: Caveats {
                    payees: None,
                    max_per_payment: Amount::units(10),
                    burst: Amount::units(50),
                    rate: Amount(50_000),
                    expires_at: Pulse(pulse.0 + 100_000),
                    evidence_above: None,
                    evidence_policy: sion_assurance::Policy::standard(),
                    receipts: true,
                    may_delegate: true,
                },
                funding: Amount::units(funding),
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
        .unwrap();
    session
        .fund(wallet, AssetId::usd(), Amount::units(1))
        .unwrap();
    agent
}

fn budget(ledger: &SharedLedger, agent: ObjectId) -> u128 {
    let charter = sion_cli::agents::identity(ledger.as_ref(), agent).unwrap();
    charter
        .allotments
        .iter()
        .filter_map(|id| ledger.object(*id).ok().flatten())
        .filter_map(|o| match o.data {
            ObjectData::Allotment(a) => Some(a.remaining.0),
            _ => None,
        })
        .sum()
}

fn script(home: &std::path::Path, name: &str, steps: Value) -> String {
    let path = home.join(name);
    std::fs::write(&path, steps.to_string()).unwrap();
    format!("script:{}", path.display())
}

#[test]
fn agents_hand_work_to_others_on_a_schedule_and_through_channels() {
    let home = std::env::temp_dir().join(format!("sion-agent-work-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    std::env::set_var("SION_HOME", &home);

    let devnet = Arc::new(
        Devnet::start(DevnetConfig {
            rounds_per_pulse: 2,
            prove_chunk: None,
            ..DevnetConfig::default()
        })
        .unwrap(),
    );
    let ledger: SharedLedger = devnet.clone();
    let mut session = Session::open(ledger.as_ref()).unwrap();
    for (name, amount) in [("dana", 1_000), ("bob", 10)] {
        session
            .fund(name, AssetId::usd(), Amount::units(amount))
            .unwrap();
    }
    let lead = register(&mut session, &ledger, "dana", "lead", 100);
    let contractor = register(&mut session, &ledger, "dana", "contractor", 10);
    // The contractor's results rest on its task bond.
    session
        .fund("contractor", AssetId::usd(), Amount::units(30))
        .unwrap();
    session
        .must(
            "contractor",
            TxKind::PostBond {
                class: AtomClass::Economic,
                label: "task-worker".into(),
                amount: Amount::units(20),
            },
            Vec::new(),
        )
        .unwrap();

    // The lead hands a payment to a subagent with 10 USD of its budget;
    // the subagent pays bob, then cannot pay more than it was given.
    let lead_script = script(
        &home,
        "lead.json",
        json!([
            {"call": "agent_spawn", "input": {"name": "buyer", "budget": "10", "task": "Buy the dataset from bob: pay him 4 USD, then 8 USD for the update."}},
            {"call": "task_post", "input": {"amount": "5", "description": "Summarise the dataset in three lines.", "worker": "contractor"}},
            {"say": "The buyer paid bob, and the summary is commissioned."}
        ]),
    );
    let buyer_script = script(
        &home,
        "buyer.json",
        json!([
            {"call": "ledger_pay", "input": {"to": "bob", "amount": "4", "purpose": "dataset"}},
            {"call": "ledger_pay", "input": {"to": "bob", "amount": "8", "purpose": "update"}},
            {"say": "Paid bob 4 USD; the 8 USD update is more than I have left."}
        ]),
    );
    let (saved, lines) = runtime::start(
        ledger.clone(),
        "lead",
        lead,
        "Get the dataset from bob and have it summarised.",
        vec![lead_script],
        Policy {
            autonomy: Autonomy::L6,
            ..Policy::default()
        },
        Equipment {
            delegate: true,
            subagent_models: vec![buyer_script],
            ..Equipment::default()
        },
    )
    .unwrap();
    let text = lines.join("\n");
    assert!(matches!(saved.ended, Some(Ended::Done { .. })), "{text}");
    // What the tools returned, in full.
    let results: Vec<String> = saved
        .state
        .messages
        .iter()
        .flat_map(|m| m.content.iter())
        .filter_map(|b| match b {
            sion_agent::Block::ToolResult { content, .. } => Some(content.clone()),
            _ => None,
        })
        .collect();
    let spawned = &results[0];
    assert!(spawned.starts_with("subagent lead-buyer"), "{spawned}");
    assert!(
        spawned.contains("Its calls: ledger_pay ok, ledger_pay failed (not sent: no part of your budget holds 8 USD)"),
        "{spawned}"
    );
    // The subagent is keyless (spec v1.13): no seed was made for it, and the
    // lead's wallet signs for it.
    assert!(!home.join("wallets").join("lead-buyer.seed").exists());
    let alias = sion_cli::session::keyless_alias("lead-buyer").expect("a keyless subagent");
    assert_eq!(alias.signer, "lead");
    let child = sion_cli::agents::identity(ledger.as_ref(), alias.charter).unwrap();
    assert_eq!(
        child.agent,
        sion_exec::charter::keyless_account(&alias.charter)
    );
    // 10 USD went to the subagent, and 5 into the task's escrow.
    assert_eq!(budget(&ledger, lead), Amount::units(85).0, "{text}");
    let task = results[1]
        .split("Task ")
        .nth(1)
        .expect("a task was posted")
        .chars()
        .take(64)
        .collect::<String>();

    // The contractor delivers, and is paid from the lead's escrow.
    let contractor_script = script(
        &home,
        "contractor.json",
        json!([
            {"call": "task_status", "input": {"task": task}},
            {"call": "task_submit", "input": {"task": task, "output": "Three lines: rows, columns, a trend."}},
            {"say": "Delivered."}
        ]),
    );
    let (_, lines) = runtime::start(
        ledger.clone(),
        "contractor",
        contractor,
        "Do the summary you were hired for.",
        vec![contractor_script],
        Policy {
            autonomy: Autonomy::L5,
            ..Policy::default()
        },
        Equipment {
            delegate: true,
            ..Equipment::default()
        },
    )
    .unwrap();
    let text = lines.join("\n");
    assert!(text.contains("settled: 5 USD paid to contractor"), "{text}");
    let state = sion_cli::tasks::describe(ledger.as_ref(), {
        let h = sion_core::hash::Hash32::from_hex(&task).unwrap();
        ObjectId::from_hash(h)
    })
    .unwrap();
    assert!(state["state"]["Settled"].is_object(), "{state}");

    // A profile, and a job due now that reports to a file.
    let check = script(
        &home,
        "check.json",
        json!([{"call": "ledger_balance"}, {"say": "85 USD left in the budget."}]),
    );
    let profile = Profile {
        name: "lead".into(),
        wallet: "lead".into(),
        agent: lead,
        models: vec![check],
        autonomy: Autonomy::L3,
        memory: None,
        skills: None,
        workspace: None,
        web: false,
        delegate: false,
        subagent_models: Vec::new(),
        adjudicator: None,
        reflect: true,
        shell: None,
        mcp: Vec::new(),
        storage: false,
        instructions: None,
        skill_approvals: true,
    };
    sion_cli::profiles::save_all(
        &[("lead".to_string(), profile.clone())]
            .into_iter()
            .collect(),
    )
    .unwrap();
    let reports = home.join("reports.jsonl");
    let now = sion_cli::schedules::now();
    sion_cli::schedules::add(
        "budget-check",
        "lead",
        "How much budget is left?",
        When::Every {
            seconds: 3_600,
            from: now - 3_600,
        },
        Some(format!("file:{}", reports.display())),
        now - 3_600,
    )
    .unwrap();
    let lines = sion_cli::schedules::run_due(ledger.clone(), now).unwrap();
    let text = lines.join("\n");
    assert!(
        text.contains("job budget-check: 85 USD left in the budget. (delivered to file:"),
        "{text}"
    );
    let report: Value =
        serde_json::from_str(std::fs::read_to_string(&reports).unwrap().trim()).unwrap();
    assert_eq!(report["said"], "85 USD left in the budget.");
    let jobs = sion_cli::schedules::all().unwrap();
    assert_eq!(jobs["budget-check"].next, Some(now + 3_600));
    assert_eq!(jobs["budget-check"].runs.len(), 1);
    // Nothing else is due.
    assert!(sion_cli::schedules::run_due(ledger.clone(), now)
        .unwrap()
        .is_empty());

    // The HTTP channel: a token is needed.
    let port = {
        let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        probe.local_addr().unwrap().port()
    };
    let listen = format!("127.0.0.1:{port}");
    let token = "a-token-of-twenty-chars";
    let serving = {
        let ledger = ledger.clone();
        let listen = listen.clone();
        std::thread::spawn(move || sion_cli::channels::serve(ledger, &listen, token, Some(2)))
    };
    let client = sion_http::Client::new(sion_http::Options {
        allow_private: true,
        proxy_from_env: false,
        timeout: std::time::Duration::from_secs(120),
        ..sion_http::Options::default()
    })
    .unwrap();
    let url = format!("http://{listen}/v1/profiles/lead/messages");
    let body = json!({"text": "How much budget is left?", "conversation": "c1"}).to_string();
    let mut tries = 0;
    let refused = loop {
        match client.post_json(&url, &[], body.as_bytes(), 1 << 20) {
            Err(sion_http::HttpError::Transport(_)) if tries < 50 => {
                tries += 1;
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            other => break other,
        }
    };
    assert!(refused.unwrap_err().to_string().contains("401"));
    let answered = client
        .post_json(
            &url,
            &[("Authorization", &format!("Bearer {token}"))],
            body.as_bytes(),
            1 << 20,
        )
        .unwrap();
    let answer: Value = serde_json::from_slice(&answered.body).unwrap();
    assert_eq!(answer["said"], "85 USD left in the budget.", "{answer}");
    assert_eq!(answer["conversation"], "c1");
    serving.join().unwrap().unwrap();

    // Telegram, against a stand-in for its API: chat 42 is answered, chat
    // 99 is not.
    let stand_in = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let base = format!("http://{}", stand_in.server_addr());
    let (sent, received) = mpsc::channel::<Value>();
    let api = std::thread::spawn(move || {
        for _ in 0..2 {
            let mut request = stand_in.recv().unwrap();
            let mut body = String::new();
            std::io::Read::read_to_string(request.as_reader(), &mut body).unwrap();
            let url = request.url().to_string();
            let result = if url.ends_with("/getUpdates") {
                json!([
                    {"update_id": 7, "message": {"chat": {"id": 42}, "text": "How much budget is left?"}},
                    {"update_id": 8, "message": {"chat": {"id": 99}, "text": "Pay me."}}
                ])
            } else {
                sent.send(serde_json::from_str(&body).unwrap()).unwrap();
                json!({})
            };
            assert!(url.starts_with("/botbot-token/"), "{url}");
            request
                .respond(tiny_http::Response::from_string(
                    json!({"ok": true, "result": result}).to_string(),
                ))
                .unwrap();
        }
    });
    std::env::set_var("TELEGRAM_BOT_TOKEN", "bot-token");
    std::env::set_var("TELEGRAM_API_BASE", &base);
    sion_cli::channels::telegram(ledger.clone(), &profile, &[42], true).unwrap();
    api.join().unwrap();
    let message = received.recv().unwrap();
    assert_eq!(message["chat_id"], 42);
    assert_eq!(message["text"], "85 USD left in the budget.");
    assert!(received.try_recv().is_err(), "chat 99 got no answer");
    std::fs::remove_dir_all(&home).unwrap();
}
