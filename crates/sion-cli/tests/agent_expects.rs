//! An agent that says what it expects of its calls (`--expect`), on a
//! devnet, with scripted models: a session is told where a call went
//! otherwise and keeps count, and what the agent did not expect on its
//! tasks is shown to what improves it, which finds from it a lesson that
//! the failed checks alone do not name.

use std::sync::{Arc, Mutex};

use serde_json::json;
use sion_agent::compare::TaskSet;
use sion_agent::Autonomy;
use sion_cli::improve_loop::{self, Improver, Variant};
use sion_cli::ledger::LedgerExt;
use sion_cli::profiles::Profile;
use sion_cli::runtime::SharedLedger;
use sion_cli::session::Session;
use sion_core::hash::Hash32;
use sion_core::ids::{AssetId, ObjectId, Pulse};
use sion_core::Amount;
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

/// An agent of alice's that may pay at most 10 USD at a time.
fn register(session: &mut Session<'_>, ledger: &SharedLedger, wallet: &str) -> ObjectId {
    let pulse = ledger.as_ref().pulse().unwrap();
    let account = session.id(wallet);
    let view = session
        .must(
            "alice",
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
        instructions: Some("You keep Acme's books.".into()),
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

/// What the proposer finds, which the agent's script follows.
const LESSON: &str = "get_total answers in cents: divide by 100 for USD.";

/// Tasks whose tool answers in a unit the agent does not expect: the
/// checks say only that the answer is wrong.
fn totals() -> TaskSet {
    let total = json!({
        "name": "get_total",
        "description": "The total of an invoice.",
        "otherwise": "1250 (cents)"
    });
    let case = |n: u32| {
        json!({
            "id": format!("total-{n}"),
            "task": format!("What is the total of invoice {n}, in USD?"),
            "tools": [total],
            "checks": [{"says": "12.50"}, "done"]
        })
    };
    TaskSet::parse(
        json!({"name": "totals", "cases": [case(7), case(8), case(9)]})
            .to_string()
            .as_bytes(),
    )
    .unwrap()
}

#[test]
fn what_an_agent_did_not_expect_reaches_its_session_and_what_improves_it() {
    let _one = ENVIRONMENT.lock().unwrap_or_else(|e| e.into_inner());
    let home = home("agent-expects");
    let (_devnet, ledger) = devnet();
    let mut session = Session::open(ledger.as_ref()).unwrap();
    session
        .fund("alice", AssetId::usd(), Amount::units(2_000))
        .unwrap();
    let agent = register(&mut session, &ledger, "clerk");

    // A session. It pays more than its charter lets it, and says it expects
    // the payment to go through; then one within the charter, as it says.
    let payer = script(
        &home,
        "payer",
        json!([
            {"call": "ledger_pay", "input": {
                "to": "acme-ltd", "amount": "50", "purpose": "invoice 7",
                "expect": {"outcome": "Acme is paid 50 USD"}
            }},
            {"when": "not as you expected", "call": "ledger_pay", "input": {
                "to": "acme-ltd", "amount": "5", "purpose": "invoice 7, in part",
                "expect": {"outcome": "Acme is paid 5 USD, which the charter allows"}
            }},
            {"say": "Paid 5 USD of the 50: my charter lets me pay no more at a time."}
        ]),
    );
    let paying = profile("clerk", agent, vec![payer], true);
    let (saved, lines) = paying
        .start(
            ledger.clone(),
            "Pay invoice 7 from Acme, 50 USD, to acme-ltd.",
        )
        .unwrap();
    let shown = lines.join("\n");
    eprintln!("{shown}");
    assert!(
        shown.contains(
            "[Not as you expected. You expected: Acme is paid 50 USD. What happened: the call \
             failed.]"
        ),
        "{shown}"
    );
    assert!(
        shown.contains(
            "what it expected: 1 of 2 calls went as it said they would (0 of its expectations \
             named something to check)"
        ),
        "{shown}"
    );
    let expected = &saved.state.expected;
    assert_eq!((expected.stated, expected.held), (2, 1));
    assert_eq!(expected.missed[0].tool, "ledger_pay");
    // The ledger's tool was given the payment, not the expectation: a
    // payment of 5 USD was made.
    assert!(shown.contains("agent paid 5 USD to acme-ltd"), "{shown}");
    // What `sion agent expected` prints of it.
    assert_eq!(
        sion_cli::runtime::expected_lines(&saved),
        [
            format!(
                "session {}: 1 of 2 calls went as it said they would (0 of its expectations \
                 named something to check)",
                saved.state.id
            ),
            "  ✗ ledger_pay: expected \"Acme is paid 50 USD\"; the call failed".to_string(),
        ]
    );

    // Without the option, the same profile says nothing of expectations.
    let quiet = script(&home, "quiet", json!([{"say": "Nothing to pay."}]));
    let (saved, lines) = profile("clerk", agent, vec![quiet], false)
        .start(ledger.clone(), "Is there anything to pay?")
        .unwrap();
    assert!(saved.state.expected.is_empty() && !saved.state.expecting);
    assert_eq!(
        sion_cli::runtime::expected_lines(&saved),
        [format!(
            "session {} was not asked to say what it expects (run --expect)",
            saved.state.id
        )]
    );
    assert!(
        !lines.iter().any(|l| l.contains("what it expected")),
        "{lines:?}"
    );

    // Its tasks. The agent takes the tool's answer for USD, unless it has
    // learned otherwise; either way it says what it expects of the call.
    let clerk = script(
        &home,
        "clerk",
        json!([
            {"when": "get_total answers in cents", "call": "get_total", "input": {
                "invoice": "7",
                "expect": {"outcome": "the total in cents", "contains": ["cents"]}
            }},
            {"when": "get_total answers in cents", "say": "The total is 12.50 USD."},
            {"call": "get_total", "input": {
                "invoice": "7",
                "expect": {"outcome": "the total in USD", "contains": ["usd"]}
            }},
            {"say": "The total is 1250 USD."}
        ]),
    );
    let tasks = totals();
    let variant = Variant {
        learned: String::new(),
        models: vec![clerk.clone()],
    };
    // The proposer writes the lesson only when it is shown what the agent
    // did not expect: nothing in the failed checks names the unit.
    let steps = json!([
        {"when": "the result does not hold \"usd\"", "call": "propose_change", "input": {
            "learned": LESSON, "why": "it expected USD of get_total and was given cents"
        }},
        {"say": "I see nothing to change."}
    ]);
    let improver = Improver::default();
    let propose = |profile: &Profile, name: &str| {
        let scored =
            improve_loop::score(profile, &variant, &tasks, Hash32::ZERO, 2, 20_000).unwrap();
        let spec = script(&home, name, steps.clone());
        let specs = [spec];
        let models = sion_cli::skill_improve::models_of(&specs)().unwrap();
        let (proposals, _) = improve_loop::propose(
            models[0].as_ref(),
            &improver,
            profile.instructions.as_deref(),
            &variant,
            std::slice::from_ref(&clerk),
            &tasks,
            &scored.result,
            &[],
            0,
        );
        (scored, proposals)
    };

    // A profile that does not say what it expects: the tasks fail, and the
    // proposer is shown only that the answers were wrong.
    let silent = profile("clerk", agent, vec![clerk.clone()], false);
    let (scored, proposals) = propose(&silent, "proposer-a");
    assert_eq!((scored.outcome.passed, scored.outcome.total), (0, 6));
    assert!(scored.result.trials.iter().all(|t| t.surprises.is_empty()));
    assert!(proposals.is_empty(), "{proposals:?}");

    // One that does: each run records the call that went otherwise, the
    // proposer is shown it once a task, and writes the lesson.
    let saying = profile("clerk", agent, vec![clerk.clone()], true);
    let (scored, proposals) = propose(&saying, "proposer-b");
    assert_eq!((scored.outcome.passed, scored.outcome.total), (0, 6));
    for trial in &scored.result.trials {
        assert_eq!(
            trial.surprises,
            ["get_total: expected \"the total in USD\"; the result does not hold \"usd\""],
            "{trial:?}"
        );
    }
    let (text, total) = improve_loop::surprises_shown(&tasks, &scored.result, 2);
    assert_eq!(total, 3);
    assert_eq!(
        text,
        "- Task total-7 (in 2 runs, all failed): get_total: expected \"the total in USD\"; the \
         result does not hold \"usd\"\n\
         - Task total-8 (in 2 runs, all failed): get_total: expected \"the total in USD\"; the \
         result does not hold \"usd\"\n"
    );
    assert_eq!(proposals.len(), 1, "{proposals:?}");
    assert_eq!(proposals[0].learned.as_deref(), Some(LESSON));

    // With the lesson the agent expects rightly and its tasks pass: nothing
    // is left that it did not expect.
    let learned = proposals[0].applied_to(&variant);
    let after = improve_loop::score(&saying, &learned, &tasks, Hash32::ZERO, 2, 20_000).unwrap();
    assert_eq!((after.outcome.passed, after.outcome.total), (6, 6));
    assert!(after.result.trials.iter().all(|t| t.surprises.is_empty()));
}
