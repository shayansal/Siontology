//! An agent at work on a devnet: a scripted model drives the runtime, which
//! pays through the agent's charter. The ledger refuses what the charter
//! forbids whatever the runtime allows; the runtime asks before paying at
//! autonomy L5; the kill switch stops a run.

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
                    max_per_window: Amount::units(50),
                    window: 1_000,
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
    saved_again.decide(true, "").unwrap();
    let lines = saved_again.run(ledger.clone()).unwrap();
    assert!(
        matches!(saved_again.ended, Some(Ended::Done { .. })),
        "{}",
        lines.join("\n")
    );
    assert_eq!(
        session.balance("bob", AssetId::usd()).unwrap().0 - bob_after.0,
        Amount::units(3).0
    );
    // Deciding again has nothing to decide.
    assert!(saved_again.decide(true, "").is_err());
    let _ = saved.decide(false, "too late");

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
}
