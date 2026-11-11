//! A profile's routes choose its models for each task: the sorter is asked
//! for a task no rule decides, a chat's own model still comes first, what
//! each route was given is kept, and a model earns first place in a route
//! only by beating the route's first on a set of tasks.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use serde_json::json;
use sion_agent::{Autonomy, Ended};
use sion_cli::channels::ChatSettings;
use sion_cli::ledger::LedgerExt;
use sion_cli::model_routes::{self, ModelRoute, Task};
use sion_cli::profiles::{self, Profile};
use sion_cli::runtime::{Hooks, SharedLedger};
use sion_cli::session::Session;
use sion_core::ids::{AssetId, ObjectId, Pulse};
use sion_core::Amount;
use sion_exec::charter::Caveats;
use sion_exec::execute::Event;
use sion_exec::TxKind;
use sion_node::{Devnet, DevnetConfig};

static ENVIRONMENT: Mutex<()> = Mutex::new(());

fn one_at_a_time() -> std::sync::MutexGuard<'static, ()> {
    ENVIRONMENT.lock().unwrap_or_else(|e| e.into_inner())
}

fn home(name: &str) -> std::path::PathBuf {
    let home = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    std::env::set_var("SION_HOME", &home);
    home
}

/// A scripted model that plays `steps`, as a model spec.
fn script(home: &std::path::Path, name: &str, steps: serde_json::Value) -> String {
    let path = home.join(format!("{name}.json"));
    std::fs::write(&path, steps.to_string()).unwrap();
    format!("script:{}", path.display())
}

fn register(session: &mut Session<'_>, ledger: &SharedLedger, wallet: &str) -> ObjectId {
    let pulse = ledger.as_ref().pulse().unwrap();
    let account = session.id(wallet);
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
    view.effects
        .events
        .iter()
        .find_map(|e| match e {
            Event::AgentRegistered { agent, .. } => Some(*agent),
            _ => None,
        })
        .unwrap()
}

fn profile(agent: ObjectId, models: Vec<String>) -> Profile {
    Profile {
        name: "lead".into(),
        wallet: "lead".into(),
        agent,
        models,
        autonomy: Autonomy::L3,
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

fn said(ended: &Option<Ended>) -> String {
    match ended {
        Some(Ended::Done { text }) => text.clone(),
        other => panic!("the session did not end done: {other:?}"),
    }
}

#[test]
fn a_profiles_routes_choose_its_models_for_each_task() {
    let _one = one_at_a_time();
    let home = home("model-routes");
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
    let agent = register(&mut session, &ledger, "lead");
    session
        .fund("lead", AssetId::usd(), Amount::units(1))
        .unwrap();

    let usual = script(&home, "usual", json!([{"say": "On the usual model."}]));
    let coder = script(&home, "coder", json!([{"say": "On the coder."}]));
    // The sorter names the code route for a failing test, and none for the
    // rest; it is a model of its own, asked once a session.
    let sorter = script(
        &home,
        "sorter",
        json!([{"when": "failing test", "say": "code"}, {"say": "none"}]),
    );
    let mut lead = profile(agent, vec![usual.clone()]);
    lead.routes = vec![ModelRoute {
        name: "code".into(),
        about: "Writing or fixing code".into(),
        when: Vec::new(),
        longer_than: None,
        models: vec![coder.clone()],
    }];
    lead.sorter = Some(sorter);
    profiles::save_all(&BTreeMap::from([(lead.name.clone(), lead.clone())])).unwrap();

    let (saved, _) = lead
        .start(ledger.clone(), "Fix the failing test in parser.rs")
        .unwrap();
    assert_eq!(saved.models, vec![coder.clone(), usual.clone()]);
    assert_eq!(said(&saved.ended), "On the coder.");

    let (saved, _) = lead
        .start(ledger.clone(), "What is the capital of France?")
        .unwrap();
    assert_eq!(saved.models, vec![usual.clone()]);
    assert_eq!(said(&saved.ended), "On the usual model.");

    // A chat that chose a model keeps it first, the route's after it.
    let settings = ChatSettings {
        model: Some(usual.clone()),
        ..ChatSettings::default()
    };
    let (saved, _) = lead
        .start_in(
            ledger.clone(),
            "Fix the failing test in lexer.rs",
            &[],
            Hooks::default(),
            &settings,
        )
        .unwrap();
    assert_eq!(saved.models, vec![usual.clone(), coder.clone()]);
    assert_eq!(said(&saved.ended), "On the usual model.");

    // A work item goes to a route that asks for work, without the sorter.
    let decisions = model_routes::history("lead").unwrap();
    assert_eq!(decisions.len(), 3);
    assert_eq!(decisions[0].route.as_deref(), Some("code"));
    assert_eq!(decisions[0].model, coder);
    assert_eq!(decisions[0].ended.as_deref(), Some("done"));
    assert_eq!(decisions[0].why, "the sorter chose it");
    assert_eq!(decisions[1].route, None);
    assert_eq!(decisions[1].why, "the sorter chose no route (none)");
    assert_eq!(decisions[2].route.as_deref(), Some("code"));
    assert_eq!(decisions[2].model, usual);
    let lines = model_routes::show(&lead, &decisions, 10).join("\n");
    assert!(
        lines.contains(&format!("code on {coder}: 1 session, 1 done")),
        "{lines}"
    );
    assert!(
        lines.contains(&format!("- on {usual}: 1 session, 1 done")),
        "{lines}"
    );
    assert!(lines.contains("3 sessions routed"), "{lines}");

    // A route's signal decides before the sorter is asked.
    let mut working = lead.clone();
    working.sorter = Some("script:/nonexistent-sorter.json".into());
    working.routes.push(ModelRoute {
        name: "work".into(),
        about: String::new(),
        when: vec![model_routes::Signal::Work],
        longer_than: None,
        models: vec![coder.clone()],
    });
    let choice = model_routes::choose_for(
        &working,
        &Task {
            text: "Close issue 7",
            pictures: 0,
            work: true,
        },
    );
    assert_eq!(choice.route.as_deref(), Some("work"));
    assert_eq!(choice.why, "the task is a work item");
    // With no signal, a sorter that cannot be reached leaves the task on
    // the profile's own models.
    let choice = model_routes::choose_for(
        &working,
        &Task {
            text: "Hello",
            pictures: 0,
            work: false,
        },
    );
    assert_eq!(choice.route, None);
    assert!(
        choice.why.starts_with("the sorter could not answer"),
        "{}",
        choice.why
    );
    assert_eq!(choice.models, vec![usual.clone()]);
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn a_model_earns_first_place_in_a_route_only_by_beating_its_first() {
    let _one = one_at_a_time();
    let home = home("model-routes-eval");
    let unsure = script(&home, "unsure", json!([{"say": "I am not sure."}]));
    let sure = script(&home, "sure", json!([{"say": "It is 42."}]));
    let sorter = script(&home, "sorter", json!([{"say": "math"}]));
    let mut lead = profile(
        ObjectId::from_hash(sion_core::hash::Hash32::ZERO),
        vec![unsure.clone()],
    );
    lead.routes = vec![ModelRoute {
        name: "math".into(),
        about: "Arithmetic".into(),
        when: Vec::new(),
        longer_than: None,
        models: vec![unsure.clone(), sure.clone()],
    }];
    lead.sorter = Some(sorter);
    let set = json!({
        "name": "times",
        "cases": [{
            "id": "six-sevens",
            "task": "What is six times seven?",
            "checks": [{"says": "42"}, "done"]
        }]
    })
    .to_string();
    let task = Task {
        text: "What is six times seven?",
        pictures: 0,
        work: false,
    };
    // Before any comparison, the route keeps its order.
    assert_eq!(
        model_routes::choose_for(&lead, &task).models,
        vec![unsure.clone(), sure.clone()]
    );
    let comparison = model_routes::evaluate(&lead, "math", set.as_bytes(), 8, 10_000).unwrap();
    assert_eq!(comparison.models[0].1.passed(), 0);
    assert_eq!(comparison.models[1].1.passed(), 8);
    assert_eq!(comparison.earned(), Some(sure.clone()));
    model_routes::keep(&lead.name, &comparison).unwrap();
    assert_eq!(
        model_routes::choose_for(&lead, &task).models,
        vec![sure.clone(), unsure.clone()]
    );
    // One run each is not enough to tell them apart.
    let thin = model_routes::evaluate(&lead, "math", set.as_bytes(), 1, 10_000).unwrap();
    assert_eq!(thin.earned(), None);
    // A route with one model has nothing to compare.
    lead.routes[0].models.truncate(1);
    assert!(model_routes::evaluate(&lead, "math", set.as_bytes(), 2, 10_000).is_err());
    let _ = std::fs::remove_dir_all(&home);
}
