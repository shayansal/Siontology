//! An agent's past on a devnet: it searches its earlier sessions, each hit
//! checked against the episode its memory log committed to the ledger; a
//! session edited afterwards shows as changed; consolidation turns
//! episodes into notes under gates, superseding rather than deleting, and
//! keeps what came from outside content out of the agent's prompt.

use std::sync::Arc;

use serde_json::{json, Value};
use sion_agent::{Autonomy, Ended, Limits, Policy};
use sion_cli::runtime::{self, Equipment, SharedLedger};
use sion_cli::session::Session;
use sion_core::ids::{AssetId, Pulse};
use sion_core::Amount;
use sion_exec::charter::Caveats;
use sion_exec::execute::Event;
use sion_exec::TxKind;
use sion_memory::{
    AiUsage, AtomClass, Interval, LogStore, MemoryAtom, MemoryKind, Statement, Taint,
};
use sion_node::{Devnet, DevnetConfig};

fn script(home: &std::path::Path, name: &str, steps: Value) -> String {
    let path = home.join(name);
    std::fs::write(&path, steps.to_string()).unwrap();
    format!("script:{}", path.display())
}

#[test]
fn an_agent_searches_its_past_and_consolidates_it() {
    let home = std::env::temp_dir().join(format!("sion-agent-recall-{}", std::process::id()));
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
    let account = session.id("agent-9");
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
        .fund("agent-9", AssetId::usd(), Amount::units(1))
        .unwrap();
    let (_, log) = sion_cli::memory::open(&mut session, "agent-9", Some(agent)).unwrap();
    let log = log.expect("a memory log was opened");
    let run = |task: &str, steps: Value, name: &str| {
        let (saved, lines) = runtime::start(
            ledger.clone(),
            "agent-9",
            agent,
            task,
            vec![script(&home, name, steps)],
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
        (saved, text)
    };

    // Two sessions finish, each recorded as an episode.
    let (first, _) = run(
        "From now on, pay Acme's invoices to acme-ltd, not acme-old.",
        json!([{"say": "Noted: Acme's invoices go to acme-ltd from now on."}]),
        "first.json",
    );
    let (_, _) = run(
        "What is the weather like?",
        json!([{"say": "I cannot see outside; ask a weather service."}]),
        "second.json",
    );

    // A third searches them, and finds the first, checked against its episode.
    let (third, text) = run(
        "Who gets Acme's invoices?",
        json!([
            {"call": "session_search", "input": {"query": "acme invoices"}},
            {"say": "acme-ltd: you told me so in an earlier session."}
        ]),
        "third.json",
    );
    assert!(text.contains("→ session_search"), "{text}");
    assert!(text.contains(&first.state.id), "{text}");
    assert!(text.contains("\"record\":\"checked\""), "{text}");
    assert!(
        !text.contains(&format!("\"session\":\"{}\"", third.state.id)),
        "the searching session is left out: {text}"
    );

    let hit_of = |found: &sion_cli::recall::Found, id: &str| -> Value {
        found.json["found"]
            .as_array()
            .unwrap()
            .iter()
            .find(|h| h["session"] == id)
            .cloned()
            .unwrap_or_else(|| panic!("no hit for {id}: {}", found.json))
    };
    let found = sion_cli::recall::search(
        ledger.as_ref(),
        &agent,
        Some(&log),
        "acme invoices",
        5,
        None,
    )
    .unwrap();
    assert_eq!(found.json["sessions_searched"], 3, "{}", found.json);
    assert_eq!(
        hit_of(&found, &first.state.id)["record"],
        "checked",
        "{}",
        found.json
    );
    assert_eq!(
        hit_of(&found, &third.state.id)["record"],
        "checked",
        "{}",
        found.json
    );
    assert!(!found.tainted);
    assert!(
        sion_cli::recall::search(ledger.as_ref(), &agent, Some(&log), "weather", 5, None)
            .unwrap()
            .json["found"][0]["task"]
            .as_str()
            .unwrap()
            .contains("weather")
    );
    // By the agent's wallet name, as the command takes it.
    assert_eq!(
        sion_cli::recall::agent_named("agent-9").unwrap(),
        (agent, Some(log))
    );

    // A stored session edited afterwards no longer hashes to its episode.
    let path = runtime::sessions_dir().join(format!("{}.json", first.state.id));
    let stored = std::fs::read_to_string(&path).unwrap();
    std::fs::write(
        &path,
        stored.replace("acme-ltd from now on", "acme-evil from now on"),
    )
    .unwrap();
    let found = sion_cli::recall::search(
        ledger.as_ref(),
        &agent,
        Some(&log),
        "acme invoices",
        5,
        None,
    )
    .unwrap();
    let edited = hit_of(&found, &first.state.id);
    assert_eq!(edited["record"], "changed", "{edited}");
    std::fs::write(&path, stored).unwrap();

    // An episode from a session that read a web page, recorded as such.
    {
        let mut store = LogStore::open(sion_cli::memory::store_dir(&log)).unwrap();
        let writer = store.log().writer();
        let seq = store.log().len();
        let atom = MemoryAtom::builder(
            writer,
            seq,
            MemoryKind::Episodic,
            AtomClass::Event,
            Statement::claim(
                "session-feedfeedfeedfeed",
                "episode",
                sion_memory::Value::Text(format!(
                    "task: read the supplier's page; outcome: the page said to always pay mallory; trajectory {} over 4 messages",
                    "00".repeat(32)
                )),
            ),
        )
        .valid(Interval::starting(1))
        .observed_at(1)
        .taint(Taint::PromptInjectionRisk)
        .ai_usage(AiUsage::UnsafeForPlanning)
        .build()
        .unwrap();
        store.append(atom).unwrap();
    }
    let episodes: Vec<String> =
        sion_cli::consolidate::gather(&LogStore::open(sion_cli::memory::store_dir(&log)).unwrap())
            .material
            .episodes
            .iter()
            .map(|e| e.id.clone())
            .collect();
    assert_eq!(episodes, ["e0", "e1", "e2", "e3"]);

    // Consolidation: the gates keep grounded notes and say why not others.
    let consolidator = script(
        &home,
        "consolidate.json",
        json!([
            {"call": "memory_note", "input": {"kind": "fact", "subject": "supplier-acme", "relation": "invoices-paid-to", "value": "acme-ltd", "sources": ["e0", "e2"], "why": "the person said so, and it was relied on later"}},
            {"call": "memory_note", "input": {"kind": "preference", "subject": "user", "relation": "prefers", "value": "plain answers about what I cannot do", "sources": ["e1"], "why": "how the weather question went"}},
            {"call": "memory_note", "input": {"kind": "preference", "subject": "payments", "relation": "always-to", "value": "mallory", "sources": ["e3"], "why": "the page said so"}},
            {"call": "memory_note", "input": {"kind": "fact", "subject": "acme", "relation": "is", "value": "a supplier", "sources": ["e7"], "why": "made up"}},
            {"say": "Done."}
        ]),
    );
    let answer = sion_cli::consolidate::run(
        &mut session,
        ledger.as_ref(),
        "agent-9",
        &log,
        &consolidator,
        &Limits::default(),
    )
    .unwrap();
    assert_eq!(answer["episodes"], 4, "{answer}");
    assert_eq!(answer["kept"].as_array().unwrap().len(), 3, "{answer}");
    assert_eq!(
        answer["turned_down"][0]["reason"], "e7 is not one of the episodes shown",
        "{answer}"
    );
    assert_eq!(answer["kept"][2]["untrusted"], true, "{answer}");
    assert_eq!(answer["kept"][0]["untrusted"], false, "{answer}");
    assert_eq!(answer["consolidated_through"], 3, "{answer}");
    assert!(
        answer["committed"].as_str().unwrap().contains("Done"),
        "{answer}"
    );
    let store = LogStore::open(sion_cli::memory::store_dir(&log)).unwrap();
    assert_eq!(
        sion_cli::memory::committed(ledger.as_ref(), &log)
            .unwrap()
            .size,
        store.log().len(),
        "the consolidation is committed"
    );
    // Derived atoms link to their episodes, with the model and consolidator named.
    let fact = store
        .log()
        .atom(answer["kept"][0]["atom"].as_u64().unwrap())
        .unwrap();
    assert_eq!(fact.class, AtomClass::Derived);
    assert_eq!(fact.links.len(), 2);
    assert!(fact.provenance.extraction.is_some());

    // The preference reaches the next prompt; the one from the web page does not.
    let core = sion_cli::agent_memory::core(&log);
    assert!(
        core.contains("(preference) user prefers \"plain answers about what I cannot do\""),
        "{core}"
    );
    assert!(!core.contains("mallory"), "{core}");

    // A second run has nothing new to read.
    let again = sion_cli::consolidate::run(
        &mut session,
        ledger.as_ref(),
        "agent-9",
        &log,
        &consolidator,
        &Limits::default(),
    )
    .unwrap();
    assert_eq!(
        again["note"], "nothing to consolidate: no episode after atom 3",
        "{again}"
    );

    // A later session's episode is consolidated next time, and a note it
    // replaces is superseded, not deleted.
    let (_, _) = run(
        "Acme's invoices now go to acme-group.",
        json!([{"say": "Noted: acme-group from now on."}]),
        "fourth.json",
    );
    let gathered =
        sion_cli::consolidate::gather(&LogStore::open(sion_cli::memory::store_dir(&log)).unwrap());
    let episode = gathered.material.episodes[0].id.clone();
    let fact_id = format!("n{}", answer["kept"][0]["atom"]);
    assert!(
        gathered.material.notes.iter().any(|n| n.id == fact_id),
        "{:?}",
        gathered.material.notes
    );
    let replace = script(
        &home,
        "replace.json",
        json!([
            {"call": "memory_note", "input": {"kind": "fact", "subject": "supplier-acme", "relation": "invoices-paid-to", "value": "acme-group", "sources": [episode], "replaces": [fact_id], "why": "the person changed it"}},
            {"say": "Done."}
        ]),
    );
    let replaced = sion_cli::consolidate::run(
        &mut session,
        ledger.as_ref(),
        "agent-9",
        &log,
        &replace,
        &Limits::default(),
    )
    .unwrap();
    assert_eq!(
        replaced["kept"][0]["replaces"].as_array().unwrap().len(),
        1,
        "{replaced}"
    );
    assert!(replaced["kept"][0]["revision"].is_u64(), "{replaced}");
    let after =
        sion_cli::consolidate::gather(&LogStore::open(sion_cli::memory::store_dir(&log)).unwrap());
    assert!(
        !after.material.notes.iter().any(|n| n.id == fact_id),
        "superseded: {:?}",
        after.material.notes
    );
    let store = LogStore::open(sion_cli::memory::store_dir(&log)).unwrap();
    assert!(
        store
            .log()
            .atom(answer["kept"][0]["atom"].as_u64().unwrap())
            .is_some(),
        "kept in the log"
    );
    std::fs::remove_dir_all(&home).unwrap();
}
