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

/// What a session's tool calls returned, in full.
fn tool_results(saved: &runtime::Saved) -> Vec<String> {
    saved
        .state
        .messages
        .iter()
        .flat_map(|m| m.content.iter())
        .filter_map(|b| match b {
            sion_agent::Block::ToolResult { content, .. } => Some(content.clone()),
            _ => None,
        })
        .collect()
}

/// The task a `task_post` result names.
fn posted(result: &str) -> String {
    result
        .split("Task ")
        .nth(1)
        .unwrap_or_else(|| panic!("no task posted: {result}"))
        .chars()
        .take(64)
        .collect()
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
        hear: None,
        quarantine: false,
        shared_memory: Vec::new(),
        confidential: false,
        confidential_files: Vec::new(),
        routes: Vec::new(),
        sorter: None,
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
    // 99 is not. Besides the reply, the bridge shows it is at work
    // (sendChatAction) and marks the message (setMessageReaction).
    let stand_in = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let base = format!("http://{}", stand_in.server_addr());
    let (sent, received) = mpsc::channel::<Value>();
    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let serving = done.clone();
    let api = std::thread::spawn(move || {
        let mut methods = Vec::new();
        while !serving.load(std::sync::atomic::Ordering::SeqCst) {
            let Ok(Some(mut request)) = stand_in.recv_timeout(std::time::Duration::from_millis(50))
            else {
                continue;
            };
            let mut body = String::new();
            std::io::Read::read_to_string(request.as_reader(), &mut body).unwrap();
            let url = request.url().to_string();
            assert!(url.starts_with("/botbot-token/"), "{url}");
            let method = url.rsplit('/').next().unwrap_or_default().to_string();
            let result = match method.as_str() {
                "getUpdates" => json!([
                    {"update_id": 7, "message": {"message_id": 70, "chat": {"id": 42}, "text": "How much budget is left?"}},
                    {"update_id": 8, "message": {"message_id": 80, "chat": {"id": 99}, "text": "Pay me."}}
                ]),
                "sendMessage" => {
                    sent.send(serde_json::from_str(&body).unwrap()).unwrap();
                    json!({"message_id": 71, "chat": {"id": 42}})
                }
                _ => json!(true),
            };
            methods.push(method);
            request
                .respond(tiny_http::Response::from_string(
                    json!({"ok": true, "result": result}).to_string(),
                ))
                .unwrap();
        }
        methods
    });
    std::env::set_var("TELEGRAM_BOT_TOKEN", "bot-token");
    std::env::set_var("TELEGRAM_API_BASE", &base);
    sion_cli::channels::telegram(ledger.clone(), &profile, &[42], true).unwrap();
    done.store(true, std::sync::atomic::Ordering::SeqCst);
    let methods = api.join().unwrap();
    assert_eq!(methods[0], "getUpdates", "{methods:?}");
    assert_eq!(
        methods.iter().filter(|m| *m == "getUpdates").count(),
        1,
        "{methods:?}"
    );
    let message = received.recv().unwrap();
    assert_eq!(message["chat_id"], 42);
    assert_eq!(message["text"], "85 USD left in the budget.");
    assert!(received.try_recv().is_err(), "chat 99 got no answer");

    // A graph of agents: the task split, its parts done side by side, each
    // a session of its own, and put together. One script answers every
    // node, each step by what its node is asked.
    let team = Profile {
        name: "team".into(),
        models: vec![script(
            &home,
            "team.json",
            json!([
                {"when": "you decide how a task is worked on",
                 "say": "{\"shape\": \"fan-out\", \"why\": \"two things to look up\"}"},
                {"when": "split the task into parts",
                 "say": "{\"parts\": [\"the budget left\", \"the tasks posted\"]}"},
                {"when": "your part of it:\nthe budget left", "call": "ledger_balance"},
                {"when": "your part of it:\nthe budget left", "say": "85 USD left."},
                {"when": "your part of it:\nthe tasks posted", "say": "One task, settled."},
                {"when": "put the parts' reports together",
                 "say": "85 USD left; the one task posted is settled."}
            ]),
        )],
        ..profile.clone()
    };
    let task = "What is left of the budget, and what became of the tasks posted?";
    let route = sion_cli::graphs::route(&team, task).unwrap();
    assert_eq!(route.shape, "fan-out", "{route:?}");
    let graph = sion_agent::graph::template(&route.shape).unwrap();
    let (kept, lines) = sion_cli::graphs::run(ledger.clone(), &team, graph, task).unwrap();
    let text = lines.join("\n");
    assert!(
        text.ends_with("answer: 85 USD left; the one task posted is settled."),
        "{text}"
    );
    assert_eq!(kept.run.visits("part"), 2, "{text}");
    // Each node ran in a session of its own, kept like any other.
    let sessions: std::collections::BTreeSet<String> = kept
        .run
        .ran
        .iter()
        .filter_map(|ran| ran.answer.as_ref().ok()?.session.clone())
        .collect();
    assert_eq!(sessions.len(), 4, "{text}");
    for id in &sessions {
        assert!(sion_cli::runtime::Saved::load(id).is_ok(), "session {id}");
    }
    // The run is kept as it went, and shown again as it ended.
    let again = sion_cli::graphs::Kept::load(&kept.id[..8]).unwrap();
    assert_eq!(again.run, kept.run);
    assert_eq!(sion_cli::graphs::shown(&again), lines);
    // The run is pinned to its graph's digest, and replays as it ran: each
    // node answered as it was, the graph routes the answers the same way.
    assert_eq!(again.pinned, Some(sion_agent::graph::digest(&again.graph)));
    let replayed = sion_agent::graph::replay(&again.graph, &again.run).unwrap();
    assert_eq!(replayed.differs, None, "{replayed:?}");
    // A graph changed in the run's file after the run began is refused.
    let mut changed = again.clone();
    changed.graph.nodes[2].instructions = "Say whatever the parts said.".into();
    changed.save().unwrap();
    let refused = sion_cli::graphs::Kept::load(&again.id)
        .unwrap_err()
        .to_string();
    assert!(
        refused.contains("its graph was changed after the run began"),
        "{refused}"
    );
    again.save().unwrap();

    // A node whose session waits for a person stops the run there; once the
    // session is approved and has run on, the run goes on from it. The
    // first node may use only the tools that read: its call to pay finds
    // no such tool.
    let payer = Profile {
        name: "payer".into(),
        autonomy: Autonomy::L5,
        models: vec![script(
            &home,
            "payer.json",
            json!([
                {"when": "look up what is owed", "call": "ledger_pay",
                 "input": {"to": "bob", "amount": "1", "purpose": "owed"}},
                {"when": "look up what is owed", "say": "1 USD is owed to bob."},
                {"when": "pay what is owed", "call": "ledger_pay",
                 "input": {"to": "bob", "amount": "1", "purpose": "owed"}},
                {"when": "pay what is owed", "say": "Paid bob 1 USD."}
            ]),
        )],
        ..team.clone()
    };
    let graph: sion_agent::graph::Graph = serde_json::from_value(json!({
        "name": "look-then-pay",
        "start": "look",
        "nodes": [
            {"id": "look", "instructions": "Look up what is owed.", "read_only": true},
            {"id": "pay", "instructions": "Pay what is owed."}
        ],
        "edges": [{"from": "look", "to": "pay"}]
    }))
    .unwrap();
    let (stopped, lines) =
        sion_cli::graphs::run(ledger.clone(), &payer, graph, "Settle with bob.").unwrap();
    let text = lines.join("\n");
    assert!(
        text.contains("stopped: pay failed: session ")
            && text.contains("waits for a person's approval of ledger_pay"),
        "{text}"
    );
    let look = stopped.run.ran[0].answer.as_ref().unwrap();
    let looked = runtime::Saved::load(look.session.as_ref().unwrap()).unwrap();
    assert!(
        tool_results(&looked)
            .iter()
            .any(|r| r.contains("there is no tool named ledger_pay")),
        "{:?}",
        tool_results(&looked)
    );
    let waiting = stopped.run.ran[1].answer.clone().unwrap_err();
    let held = waiting
        .strip_prefix("session ")
        .and_then(|rest| rest.split(' ').next())
        .unwrap()
        .to_string();
    // Asked to go on before the session was approved, the run says so.
    let (_, lines) =
        sion_cli::graphs::go_on_after_approval(ledger.clone(), &payer, stopped.clone()).unwrap();
    assert!(
        lines.join("\n").contains(&format!(
            "approve its call (sion agent continue {held} --approve)"
        )),
        "{lines:?}"
    );
    runtime::resume(
        ledger.clone(),
        &held,
        Some(runtime::Reply::Approve(runtime::Approval::once())),
    )
    .unwrap();
    let (done, lines) =
        sion_cli::graphs::go_on_after_approval(ledger.clone(), &payer, stopped).unwrap();
    let text = lines.join("\n");
    assert!(text.ends_with("answer: Paid bob 1 USD."), "{text}");
    assert_eq!(done.run.ran.len(), 2, "the paying node did not run again");

    // The router offers only templates that beat one agent in one of the
    // profile's comparisons: none has yet, so one agent, without asking.
    let gated = sion_cli::graphs::route_earned(&team, task).unwrap();
    assert_eq!(gated.shape, "single", "{gated:?}");
    assert!(
        gated.why.contains("no template has beaten one agent"),
        "{gated:?}"
    );
    // A comparison on a set of tasks with stand-in tools: one agent looks
    // up one of the two prices; the fan-out looks up both and adds them.
    let set = json!({
        "name": "lookups",
        "cases": [{
            "id": "plan-b",
            "task": "What do plan B and its support cost together?",
            "tools": [{"name": "price", "answers": [
                {"when": {"item": "plan b"}, "says": "12 USD"},
                {"when": {"item": "support"}, "says": "3 USD"}]}],
            "checks": [{"says": "15 USD"}, "done"]
        }]
    })
    .to_string();
    let compared = Profile {
        models: vec![script(
            &home,
            "compare.json",
            json!([
                {"when": "split the task into parts", "say": "{\"parts\": [\"the price of plan b\", \"the price of support\"]}"},
                {"when": "your part of it:\nthe price of plan b", "call": "price", "input": {"item": "plan b"}},
                {"when": "your part of it:\nthe price of plan b", "say": "12 USD"},
                {"when": "your part of it:\nthe price of support", "call": "price", "input": {"item": "support"}},
                {"when": "your part of it:\nthe price of support", "say": "3 USD"},
                {"when": "put the parts' reports together", "say": "15 USD"},
                {"call": "price", "input": {"item": "plan b"}},
                {"say": "12 USD"}
            ]),
        )],
        ..team.clone()
    };
    let comparison = sion_cli::graphs::evaluate(
        &compared,
        set.as_bytes(),
        vec![sion_agent::graph::template("fan-out").unwrap()],
        3,
        8,
        30_000,
        None,
    )
    .unwrap();
    assert_eq!(comparison.earned(), ["fan-out"], "{:?}", comparison.lines());
    let kept_at = sion_cli::graphs::keep_comparison("team", &comparison).unwrap();
    assert!(kept_at.starts_with(sion_cli::graphs::evals_dir("team")));
    assert!(sion_cli::graphs::earned("team")
        .unwrap()
        .contains("fan-out"));
    // Now the router is asked, among one agent and the fan-out alone.
    let gated = sion_cli::graphs::route_earned(&team, task).unwrap();
    assert_eq!(gated.shape, "fan-out", "{gated:?}");

    // A check fixed when a task is posted: the lead commits to it in the
    // task's spec, and it is kept here for the reviewer, who runs it before
    // attesting. A check needs someone who must attest.
    session
        .fund("reviewer", AssetId::usd(), Amount::units(20))
        .unwrap();
    session
        .must(
            "reviewer",
            TxKind::PostBond {
                class: AtomClass::Economic,
                label: "verifier".into(),
                amount: Amount::units(10),
            },
            Vec::new(),
        )
        .unwrap();
    let check_sh = "n=$(cat \"$OUTPUT\")\n[ \"$n\" = 1200 ] || { echo \"counted $n rows, not 1200\"; exit 1; }\n";
    let check = json!({"files": {"check.sh": check_sh}});
    let posting = script(
        &home,
        "lead-checked.json",
        json!([
            {"call": "task_post", "input": {"amount": "5", "description": "Count the dataset's rows.", "worker": "contractor", "check": check}},
            {"call": "task_post", "input": {"amount": "5", "description": "Count the dataset's rows: answer with the number alone.", "worker": "contractor", "verifiers": ["reviewer"], "must_attest": ["reviewer"], "check": check}},
            {"call": "task_post", "input": {"amount": "3", "description": "Sum the column nobody can read.", "worker": "contractor"}},
            {"say": "The count is commissioned with its check, and the sum."}
        ]),
    );
    let (saved, lines) = runtime::start(
        ledger.clone(),
        "lead",
        lead,
        "Commission the count and the sum.",
        vec![posting],
        Policy {
            autonomy: Autonomy::L6,
            ..Policy::default()
        },
        Equipment {
            delegate: true,
            ..Equipment::default()
        },
    )
    .unwrap();
    let text = lines.join("\n");
    let posts = tool_results(&saved);
    assert!(
        posts[0].contains("not posted: a check is run by the verifiers who attest results"),
        "{text}"
    );
    let fixed = sion_cli::tasks::Check {
        command: sion_cli::tasks::CHECK_COMMAND.into(),
        files: [("check.sh".to_string(), check_sh.as_bytes().to_vec())].into(),
    };
    assert!(
        posts[1].contains(&format!(
            "its check {} is fixed",
            fixed.commitment().short()
        )),
        "{text}"
    );
    let counted = posted(&posts[1]);
    let summed = posted(&posts[2]);
    let id = |hex: &str| ObjectId::from_hash(sion_core::hash::Hash32::from_hex(hex).unwrap());

    // The contractor sees that a check is fixed, not what it is; its counts
    // are kept here for the reviewer, not submitted without an attestation.
    let delivering = script(
        &home,
        "contractor-checked.json",
        json!([
            {"call": "task_status", "input": {"task": counted}},
            {"call": "task_submit", "input": {"task": counted, "output": "1199"}},
            {"call": "task_submit", "input": {"task": counted, "output": "1200"}},
            {"say": "Both counts are with the reviewer."}
        ]),
    );
    let contractor_run = |model: String, task: &str| {
        runtime::start(
            ledger.clone(),
            "contractor",
            contractor,
            task,
            vec![model],
            Policy {
                autonomy: Autonomy::L5,
                ..Policy::default()
            },
            Equipment {
                delegate: true,
                ..Equipment::default()
            },
        )
        .unwrap()
    };
    let (saved, lines) = contractor_run(delivering, "Do the count you were hired for.");
    let text = lines.join("\n");
    let delivered = tool_results(&saved);
    let status: Value = serde_json::from_str(&delivered[0]).unwrap();
    assert_eq!(
        status["check"].as_str(),
        Some(fixed.commitment().to_hex().as_str()),
        "{text}"
    );
    assert!(!delivered[0].contains("check.sh"), "{text}");
    for (result, output) in delivered[1..3].iter().zip(["1199", "1200"]) {
        assert!(result.starts_with("Not submitted yet"), "{text}");
        let kept = sion_cli::tasks::output_commitment(output.as_bytes());
        assert!(result.contains(&kept.to_hex()), "{text}");
        assert!(
            sion_cli::tasks::kept(&kept).is_some(),
            "{output} was not kept"
        );
    }

    // The check kept is the one the task fixed, and an edited one is
    // refused.
    let given = sion_cli::tasks::committed_check(ledger.as_ref(), id(&counted), Default::default())
        .unwrap();
    assert_eq!(given, fixed);
    let mut edited = fixed.clone();
    edited.files.insert("check.sh".into(), b"exit 0\n".to_vec());
    let refused = sion_cli::tasks::committed_check(
        ledger.as_ref(),
        id(&counted),
        sion_cli::tasks::Given {
            spec: None,
            check: Some(edited),
        },
    );
    assert!(
        refused.is_err_and(|e| e.to_string().contains("is not the one task")),
        "an edited check was taken"
    );

    // The reviewer runs the check: the wrong count is not attested, the
    // right one is, and with its attestation the contractor is paid.
    let sandbox = sion_agent::sandbox::Sandbox::new(sion_agent::sandbox::Backend::Bubblewrap);
    if let Err(e) = sandbox.probe() {
        eprintln!("skipped running the check: bubblewrap does not work here: {e}");
    } else {
        let verify = |session: &mut Session<'_>, verifier: &str, output: &[u8]| {
            sion_cli::tasks::verify_output(
                session,
                ledger.as_ref(),
                verifier,
                id(&counted),
                output,
                Default::default(),
                "verifier",
                &sandbox,
            )
        };
        let stranger = verify(&mut session, "bob", &b"1200"[..]);
        assert!(
            stranger.is_err_and(|e| e.to_string().contains("not one of task")),
            "bob's attestation was taken"
        );
        let wrong = verify(&mut session, "reviewer", &b"1199"[..]).unwrap();
        assert_eq!(wrong.attestation, None, "{}", wrong.report);
        assert!(
            wrong.report.contains("counted 1199 rows, not 1200"),
            "{}",
            wrong.report
        );
        let right = verify(&mut session, "reviewer", &b"1200"[..]).unwrap();
        let attestation = right.attestation.expect("the right count is attested");
        let paid = script(
            &home,
            "contractor-paid.json",
            json!([
                {"call": "task_submit", "input": {"task": counted, "output": "1200", "attestations": [attestation.to_string()]}},
                {"say": "Paid for the count."}
            ]),
        );
        let (_, lines) = contractor_run(paid, "Deliver the count the reviewer attested.");
        let text = lines.join("\n");
        assert!(text.contains("settled: 5 USD paid to contractor"), "{text}");
    }

    // The sum cannot be done: the contractor says so and releases the
    // task, whose escrow goes back; its bond is untouched.
    let giving_up = script(
        &home,
        "contractor-release.json",
        json!([
            {"call": "task_release", "input": {"task": summed, "why": "the column cannot be read"}},
            {"say": "Released the sum: the column cannot be read."}
        ]),
    );
    let (saved, lines) = contractor_run(giving_up, "Do the sum you were hired for.");
    let text = lines.join("\n");
    let released = tool_results(&saved);
    assert!(
        released[0]
            .ends_with("Released because the column cannot be read; your task bond is untouched."),
        "{text}"
    );
    let state = sion_cli::tasks::describe(ledger.as_ref(), id(&summed)).unwrap();
    assert!(state["state"]["Cancelled"].is_object(), "{state}");
    std::fs::remove_dir_all(&home).unwrap();
}
