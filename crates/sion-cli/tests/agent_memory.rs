//! An agent's memory and skills on a devnet: what it records reaches the
//! ledger when its session ends, with the session as an episode; its
//! preferences and its skills are in the next session's prompt; recall is
//! proved complete against the ledger; a retraction hides an atom without
//! deleting it.

use std::sync::Arc;

use serde_json::{json, Value};
use sion_agent::{Autonomy, Ended, Policy};
use sion_cli::agent_memory::Memory;
use sion_cli::runtime::{self, Acting, Equipment, SharedLedger};
use sion_cli::session::Session;
use sion_core::ids::{AssetId, Pulse};
use sion_core::Amount;
use sion_exec::charter::Caveats;
use sion_exec::execute::Event;
use sion_exec::TxKind;
use sion_node::{Devnet, DevnetConfig};

#[test]
fn an_agent_remembers_learns_a_skill_and_recalls_with_proof() {
    let home = std::env::temp_dir().join(format!("sion-agent-memory-{}", std::process::id()));
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
    let account = session.id("agent-8");
    let pulse = sion_cli::ledger::LedgerExt::pulse(ledger.as_ref()).unwrap();
    let view = session
        .must(
            "alice",
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
        .unwrap();
    session
        .fund("agent-8", AssetId::usd(), Amount::units(1))
        .unwrap();
    let (_, log) = sion_cli::memory::open(&mut session, "agent-8", Some(agent)).unwrap();
    let log = log.expect("a memory log was opened");

    // A first session records a preference and a fact, and writes a skill.
    let script = home.join("learn.json");
    std::fs::write(
        &script,
        json!([
            {"call": "memory_remember", "input": {"subject": "user", "relation": "prefers", "value": "invoices paid on Fridays", "kind": "preference"}},
            {"call": "memory_remember", "input": {"subject": "supplier-acme", "relation": "terms", "value": "net 30", "kind": "fact"}},
            {"call": "skill_write", "input": {"name": "pay-invoices", "description": "Pay a supplier's invoice after checking it against the order.", "instructions": "1. Recall the supplier's terms.\n2. Check the invoice.\n3. Pay it with ledger_pay."}},
            {"say": "Noted your preference and Acme's terms, and wrote down how to pay invoices."}
        ])
        .to_string(),
    )
    .unwrap();
    let (saved, lines) = runtime::start(
        ledger.clone(),
        "agent-8",
        agent,
        "Remember that I like invoices paid on Fridays and that Acme's terms are net 30.",
        vec![format!("script:{}", script.display())],
        Policy {
            autonomy: Autonomy::L3,
            ..Policy::default()
        },
        Equipment {
            memory: Some(log),
            ..Equipment::default()
        },
    )
    .unwrap();
    let text = lines.join("\n");
    assert!(matches!(saved.ended, Some(Ended::Done { .. })), "{text}");
    assert!(text.contains("memory committed"), "{text}");
    // Two atoms and the session's episode reached the ledger.
    let on_ledger = sion_cli::memory::committed(ledger.as_ref(), &log).unwrap();
    assert_eq!(on_ledger.size, 3, "{text}");

    // The next session sees the preference and the skill in its prompt.
    let prompt = saved.system_prompt();
    assert!(
        prompt.contains("(preference) user prefers \"invoices paid on Fridays\""),
        "{prompt}"
    );
    assert!(
        prompt.contains("- pay-invoices: Pay a supplier's invoice"),
        "{prompt}"
    );
    assert!(
        !prompt.contains("supplier-acme"),
        "facts are recalled, not shown: {prompt}"
    );

    // Recall with a relation is proved complete against the ledger.
    let memory = Memory {
        acting: Acting {
            ledger: ledger.clone(),
            wallet: "agent-8".into(),
            agent,
        },
        log,
    };
    let tools = memory.tools();
    let tool = |name: &str| tools.iter().find(|t| t.spec().name == name).unwrap();
    let recalled = tool("memory_recall").call(
        &json!({"subject": "supplier-acme", "relation": "terms"}),
        "k",
    );
    assert!(!recalled.is_error, "{recalled:?}");
    let value: Value = serde_json::from_str(&recalled.content).unwrap();
    assert_eq!(value["committed"].as_array().unwrap().len(), 1, "{value}");
    assert!(value["complete"]
        .as_str()
        .unwrap()
        .contains("proved complete"));
    let atom = value["committed"][0]["id"].as_str().unwrap().to_string();
    let episode = tool("memory_search").call(&json!({"words": "episode trajectory"}), "k");
    assert!(episode.content.contains("session-"), "{episode:?}");

    // A retraction hides the atom from recall and search; the log keeps it.
    let forgot = tool("memory_forget").call(
        &json!({"atom": &atom[..10], "reason": "Acme moved to net 45"}),
        "k",
    );
    assert!(!forgot.is_error, "{forgot:?}");
    let after = tool("memory_recall").call(
        &json!({"subject": "supplier-acme", "relation": "terms"}),
        "k",
    );
    let value: Value = serde_json::from_str(&after.content).unwrap();
    assert!(value["committed"].as_array().unwrap().is_empty(), "{value}");
    assert!(tool("memory_search")
        .call(&json!({"words": "supplier-acme"}), "k")
        .content
        .contains("[]"));
    let committed = tool("memory_commit").call(&json!({}), "k");
    assert!(committed.content.contains("Done"), "{committed:?}");
    assert_eq!(
        sion_cli::memory::committed(ledger.as_ref(), &log)
            .unwrap()
            .size,
        4
    );
    assert_eq!(
        tool("memory_commit").call(&json!({}), "k").content,
        "nothing to commit: the ledger holds the whole log"
    );

    // A long task done without writing a skill: the agent is asked whether
    // to write down how it did it, with only its skill tools, and does.
    let long = home.join("long.json");
    std::fs::write(
        &long,
        json!([
            {"call": "memory_search", "input": {"words": "invoice"}},
            {"call": "memory_search", "input": {"words": "order"}},
            {"call": "memory_search", "input": {"words": "terms"}},
            {"call": "memory_search", "input": {"words": "supplier"}},
            {"call": "memory_search", "input": {"words": "payment"}},
            {"say": "Nothing to pay this week."},
            {"when": "write it down as a skill", "call": "skill_write", "input": {"name": "weekly-check", "description": "Check what is due this week before paying anything.", "instructions": "1. Search memory for invoices, orders, terms, suppliers and payments.\n2. Pay nothing that is not due."}},
            {"say": "Wrote down the weekly check."}
        ])
        .to_string(),
    )
    .unwrap();
    let (reflected, lines) = runtime::start(
        ledger.clone(),
        "agent-8",
        agent,
        "Check whether anything is due this week.",
        vec![format!("script:{}", long.display())],
        Policy {
            autonomy: Autonomy::L3,
            ..Policy::default()
        },
        Equipment {
            memory: Some(log),
            reflect: true,
            ..Equipment::default()
        },
    )
    .unwrap();
    let text = lines.join("\n");
    assert_eq!(
        reflected.ended,
        Some(Ended::Done {
            text: "Nothing to pay this week.".into()
        }),
        "{text}"
    );
    assert!(
        text.contains("asked whether to write down how it did the task, as a skill:"),
        "{text}"
    );
    assert!(text.contains("wrote skill weekly-check"), "{text}");
    let skills = sion_agent::skills::list(reflected.skills.as_ref().unwrap());
    assert!(
        skills.iter().any(|s| s.name == "weekly-check"),
        "{skills:?}"
    );

    // A memory log that is not the agent's is refused.
    let (_, other) = sion_cli::memory::open(&mut session, "alice", None).unwrap();
    let refused = runtime::start(
        ledger,
        "agent-8",
        agent,
        "Remember something.",
        vec![format!("script:{}", script.display())],
        Policy::default(),
        Equipment {
            memory: other,
            ..Equipment::default()
        },
    );
    assert!(refused.is_err());
    std::fs::remove_dir_all(&home).unwrap();
}
