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

    // The next session sees the preference and the skill in its prompt;
    // this one's prompt keeps what the agent knew when it began.
    let mut saved = saved;
    assert!(!saved.system_prompt().contains("invoices paid on Fridays"));
    let prompt = saved.known_now();
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

    // Dreaming: the session read whole by a reader of its own; a note it
    // proposes is kept only if the session says it, word for word.
    let reader = home.join("dream.json");
    std::fs::write(
        &reader,
        json!([
            {"when": "acme's terms are net 30", "say": json!({"notes": [
                {"kind": "fact", "subject": "acme", "relation": "is paid",
                 "value": "30 days after its invoice", "quote": "Acme's terms are net 30"},
                {"kind": "preference", "subject": "user", "relation": "wants",
                 "value": "invoices paid the day they arrive",
                 "quote": "pay every invoice the day it arrives"}
            ]}).to_string()}
        ])
        .to_string(),
    )
    .unwrap();
    let model = format!("script:{}", reader.display());
    let dreamt = sion_cli::consolidate::dream(
        &mut session,
        ledger.as_ref(),
        "agent-8",
        &log,
        &model,
        &Default::default(),
    )
    .unwrap();
    assert_eq!(dreamt["sessions_read"], 1, "{dreamt}");
    let kept = dreamt["kept"].as_array().unwrap();
    assert_eq!(kept.len(), 1, "{dreamt}");
    assert_eq!(kept[0]["note"], "acme is paid: 30 days after its invoice");
    // Its confidence is its provenance's: one trusted session, quoted.
    assert_eq!(kept[0]["confidence_permille"], 800, "{dreamt}");
    assert_eq!(kept[0]["from"].as_array().unwrap().len(), 1, "{dreamt}");
    assert!(
        dreamt["turned_down"]
            .to_string()
            .contains("its quote is not in the transcript"),
        "{dreamt}"
    );
    // A session is dreamt over once.
    let again = sion_cli::consolidate::dream(
        &mut session,
        ledger.as_ref(),
        "agent-8",
        &log,
        &model,
        &Default::default(),
    )
    .unwrap();
    assert_eq!(again["dreamt"], 0, "{again}");

    // Two more sessions, in each of which the same call fails the same way.
    for (n, task) in [
        "Forget the note about Globex.",
        "Forget the note about Initech.",
    ]
    .iter()
    .enumerate()
    {
        let script = home.join(format!("fail-{n}.json"));
        std::fs::write(
            &script,
            json!([
                {"call": "memory_forget", "input": {"atom": "ffffffffff", "reason": "outdated"}},
                {"say": "There was no such note to forget."}
            ])
            .to_string(),
        )
        .unwrap();
        let (saved, lines) = runtime::start(
            ledger.clone(),
            "agent-8",
            agent,
            task,
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
        assert!(
            matches!(saved.ended, Some(Ended::Done { .. })),
            "{}",
            lines.join("\n")
        );
    }
    // A reader that proposes a procedure from either session, and drafts a
    // skill for the failure they share.
    let reader = home.join("dream-2.json");
    std::fs::write(
        &reader,
        json!([
            {"when": "there was no such note to forget", "say": json!({"notes": [
                {"kind": "procedure", "subject": "forgetting", "relation": "starts by",
                 "value": "searching for the note before forgetting it",
                 "quote": "There was no such note to forget"}
            ]}).to_string()},
            {"when": "the same failure came up in several", "say": json!({
                "name": "forget-carefully",
                "description": "Use before forgetting a note: find it first.",
                "instructions": "1. Search your memory for the note (memory_search).\n2. Forget it by the id the search gives."
            }).to_string()}
        ])
        .to_string(),
    )
    .unwrap();
    let reader = format!("script:{}", reader.display());
    // The agent the held-out tasks are run with: it answers one way with
    // the procedure in mind, another without.
    let worker = home.join("held-out.json");
    std::fs::write(
        &worker,
        json!([
            {"when": "searching for the note before forgetting it", "say": "I search for it first."},
            {"say": "Done."}
        ])
        .to_string(),
    )
    .unwrap();
    let worker = format!("script:{}", worker.display());
    let set = |says: &str| {
        json!({"name": "held-out", "cases": [{
            "id": "forget-a-note",
            "task": "Forget the note about Hooli.",
            "checks": [{"says": says}]
        }]})
        .to_string()
        .into_bytes()
    };
    // Held-out tasks it would do worse on: nothing is recorded, and the
    // sessions are left to dream over.
    let worse = sion_cli::consolidate::dream(
        &mut session,
        ledger.as_ref(),
        "agent-8",
        &log,
        &reader,
        &sion_cli::consolidate::DreamOptions {
            eval: Some(set("Done")),
            eval_model: Some(worker.clone()),
            eval_runs: 2,
            into: None,
        },
    )
    .unwrap();
    assert_eq!(worse["recorded"], false, "{worse}");
    assert!(
        worse["why"]
            .as_str()
            .unwrap()
            .contains("passed fewer times: forget-a-note"),
        "{worse}"
    );
    assert_eq!(worse["test"][0]["passed_without"], 2, "{worse}");
    assert_eq!(worse["test"][0]["passed_with"], 0, "{worse}");
    // A case the dream has read is not held out.
    let seen = json!({"name": "seen", "cases": [{
        "id": "globex", "task": "Forget the note about Globex.", "checks": ["done"]
    }]})
    .to_string()
    .into_bytes();
    let refused = sion_cli::consolidate::dream(
        &mut session,
        ledger.as_ref(),
        "agent-8",
        &log,
        &reader,
        &sion_cli::consolidate::DreamOptions {
            eval: Some(seen),
            eval_model: Some(worker.clone()),
            eval_runs: 1,
            into: None,
        },
    );
    assert!(
        refused.is_err_and(|e| e.to_string().contains("is not held out")),
        "a case the dream read was taken as held out"
    );
    // Held-out tasks it does no worse on, and a curated log that only the
    // dream writes: the note is recorded there, with the confidence its
    // provenance gives, and the shared failure becomes a skill draft.
    session
        .fund("curator", AssetId::usd(), Amount::units(1))
        .unwrap();
    let (_, curated) = sion_cli::memory::open(&mut session, "curator", None).unwrap();
    let curated = curated.expect("a curated log was opened");
    let dreamt = sion_cli::consolidate::dream(
        &mut session,
        ledger.as_ref(),
        "agent-8",
        &log,
        &reader,
        &sion_cli::consolidate::DreamOptions {
            eval: Some(set("search")),
            eval_model: Some(worker.clone()),
            eval_runs: 2,
            into: Some((curated, "curator".into())),
        },
    )
    .unwrap();
    let kept = dreamt["kept"].as_array().unwrap();
    assert_eq!(kept.len(), 1, "{dreamt}");
    assert_eq!(
        kept[0]["note"],
        "forgetting starts by: searching for the note before forgetting it"
    );
    assert_eq!(kept[0]["confidence_permille"], 800, "{dreamt}");
    assert_eq!(dreamt["test"][0]["passed_with"], 2, "{dreamt}");
    assert_eq!(
        sion_cli::memory::committed(ledger.as_ref(), &curated)
            .unwrap()
            .writer,
        session.id("curator")
    );
    let draft = &dreamt["skill_drafts"][0];
    assert_eq!(draft["skill"], "forget-carefully", "{dreamt}");
    assert_eq!(draft["sessions"].as_array().unwrap().len(), 2, "{dreamt}");
    let drafted = std::path::PathBuf::from(draft["draft"].as_str().unwrap());
    assert!(drafted.starts_with(sion_cli::skill_improve::drafts_dir()));
    assert!(std::fs::read_to_string(drafted.join("SKILL.md"))
        .unwrap()
        .starts_with("---\nname: forget-carefully\n"));
    // A session that keeps the curated log in mind has its note in its
    // prompt, under a heading of its own.
    let quiet = home.join("quiet.json");
    std::fs::write(&quiet, json!([{"say": "Noted."}]).to_string()).unwrap();
    let (saved, _) = runtime::start(
        ledger.clone(),
        "agent-8",
        agent,
        "Anything to keep in mind?",
        vec![format!("script:{}", quiet.display())],
        Policy {
            autonomy: Autonomy::L3,
            ..Policy::default()
        },
        Equipment {
            memory: Some(log),
            shared_memory: vec![curated],
            ..Equipment::default()
        },
    )
    .unwrap();
    let shared_at = saved
        .prompt
        .find(sion_cli::agent_memory::SHARED_HEADER)
        .unwrap_or_else(|| panic!("no curated log in the prompt: {}", saved.prompt));
    assert!(
        saved.prompt[shared_at..].contains("- (procedure) forgetting starts by"),
        "{}",
        saved.prompt
    );
    assert!(
        saved.prompt[shared_at..].contains("searching for the note before forgetting it"),
        "{}",
        saved.prompt
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
