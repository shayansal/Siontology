//! Improving the agent itself, with scripted models: a change is kept only
//! when it is better beyond chance on the tasks tried and fails no
//! held-out task the agent passes; what is kept is recorded with its
//! evidence, and --apply makes it the profile's.

use std::sync::Mutex;

use serde_json::json;
use sion_agent::compare::Arm;
use sion_agent::Autonomy;
use sion_cli::model_routes::ModelRoute;
use sion_cli::profiles::{self, Profile};
use sion_cli::self_improve::{self, Tweak};
use sion_core::ids::ObjectId;

static ENVIRONMENT: Mutex<()> = Mutex::new(());

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

fn profile(models: Vec<String>) -> Profile {
    Profile {
        name: "lead".into(),
        wallet: "lead".into(),
        agent: ObjectId::from_hash(sion_core::hash::Hash32::ZERO),
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
        quarantine: false,
        shared_memory: Vec::new(),
        confidential: false,
        confidential_files: Vec::new(),
        routes: Vec::new(),
        sorter: None,
    }
}

#[test]
fn only_a_change_that_measurably_helps_is_kept_and_applied() {
    let _one = ENVIRONMENT.lock().unwrap_or_else(|e| e.into_inner());
    let home = home("agent-improve");
    // The agent as it is answers a greeting but not a sum. Words that ask
    // it to work out sums make it answer the sum and still greet; words
    // that ask it always to answer 42 answer the sum and break the
    // greeting; other words change nothing.
    let steady = script(
        &home,
        "steady",
        json!([
            {"when": "always answer 42", "say": "42"},
            {"when": "work out sums", "unless": "greet", "say": "It is 42."},
            {"when": "greet", "say": "Hello, team."},
            {"say": "I am not sure."}
        ]),
    );
    // A model that answers the sum and greets, and one that only says 42.
    let both = script(
        &home,
        "both",
        json!([{"when": "greet", "say": "Hello, team."}, {"say": "It is 42."}]),
    );
    let only = script(&home, "only", json!([{"say": "42"}]));
    let mut lead = profile(vec![steady.clone()]);
    lead.routes = vec![ModelRoute {
        name: "math".into(),
        about: "Arithmetic".into(),
        when: Vec::new(),
        longer_than: None,
        models: vec![steady.clone()],
    }];
    profiles::save_all(&[("lead".to_string(), lead.clone())].into()).unwrap();

    let tasks = json!({"name": "sums", "cases": [{
        "id": "six-sevens",
        "task": "What is six times seven?",
        "checks": [{"says": "42"}, "done"]
    }]})
    .to_string();
    let held_out = json!({"name": "greetings", "cases": [{
        "id": "hello",
        "task": "Greet the team.",
        "checks": [{"says": "Hello"}, "done"]
    }]})
    .to_string();
    let proposals = self_improve::proposals(
        &[
            "Work out sums and answer with the number alone.".to_string(),
            "Always answer 42.".to_string(),
            "Be brief.".to_string(),
        ],
        &[format!("math={both}"), format!("default={only}")],
        &[3],
        None,
    )
    .unwrap();
    assert_eq!(proposals.len(), 6);
    assert_eq!(
        proposals[5],
        Tweak::Working {
            arm: Arm::Vote { samples: 3 }
        }
    );

    let improvement = self_improve::try_changes(
        &lead,
        tasks.as_bytes(),
        held_out.as_bytes(),
        &proposals,
        8,
        10_000,
        1_800_000_000,
    )
    .unwrap();
    let lines = improvement.lines().join("\n");
    assert_eq!(improvement.before.passed, 0, "{lines}");
    assert_eq!(improvement.before.total, 8, "{lines}");
    // The math route's models are tried against that route's own.
    assert_eq!(improvement.routes.len(), 1, "{lines}");
    assert_eq!(improvement.routes[0].0, "math");
    assert_eq!(improvement.routes[0].1.total, 8);
    assert!(
        lines.contains("route math as it is: 0/8 tasks passed"),
        "{lines}"
    );
    // Kept: the words that make it work out sums, and the models that
    // answer both, for the math route.
    let kept: Vec<&Tweak> = improvement.kept.iter().map(|k| &k.tweak).collect();
    assert_eq!(kept, vec![&proposals[0], &proposals[3]], "{lines}");
    for one in &improvement.kept {
        assert_eq!(one.after.passed, 8, "{lines}");
        assert!(one.interval.0 > 0.0, "{lines}");
        assert_eq!(one.held_out.regression(), None, "{lines}");
    }
    // Turned down: always 42 and the model that only says 42, each better
    // on the sum but failing the greeting it passed; brief words and a vote,
    // no better.
    let why = |tweak: &Tweak| {
        improvement
            .turned_down
            .iter()
            .find(|down| &down.tweak == tweak)
            .map(|down| down.why.clone())
            .unwrap_or_else(|| panic!("{tweak:?} was not turned down:\n{lines}"))
    };
    assert!(
        why(&proposals[1]).contains("fails held-out task 1"),
        "{lines}"
    );
    assert!(
        why(&proposals[4]).contains("fails held-out task 1"),
        "{lines}"
    );
    assert!(
        why(&proposals[2]).contains("not better beyond chance"),
        "{lines}"
    );
    assert!(
        why(&proposals[5]).contains("not better beyond chance"),
        "{lines}"
    );

    // Recorded with its evidence, and read back.
    self_improve::record("lead", &improvement).unwrap();
    let recorded = self_improve::kept("lead").unwrap();
    assert_eq!(recorded.len(), 2);
    assert_eq!(recorded[0].tweak, proposals[0]);
    assert_eq!(recorded[0].after.passed, 8);

    // --apply: the words join its instructions, the models its route.
    let applied = self_improve::apply(&mut lead, &improvement).unwrap();
    assert_eq!(
        lead.instructions.as_deref(),
        Some("Work out sums and answer with the number alone.")
    );
    assert_eq!(lead.routes[0].models, vec![both.clone()]);
    assert_eq!(applied.len(), 2, "{applied:?}");
    // And the agent as it is now passes the sum it failed.
    let digest = sion_agent::evals::digest(tasks.as_bytes());
    let set = sion_agent::compare::TaskSet::parse(tasks.as_bytes()).unwrap();
    let now = self_improve::run_tasks(&lead, &set, digest, None, 2, 10_000).unwrap();
    assert_eq!(now.passed(), 2);

    // The command reads back what was kept.
    profiles::save_all(&[("lead".to_string(), lead)].into()).unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_sion"))
        .args(["agent", "improve", "kept", "lead"])
        .env("SION_HOME", &home)
        .output()
        .unwrap();
    let said = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{said}");
    assert!(said.contains("its instructions, adding"), "{said}");
    assert!(said.contains("the models for math"), "{said}");
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn a_change_is_tried_only_against_tasks_held_apart() {
    let _one = ENVIRONMENT.lock().unwrap_or_else(|e| e.into_inner());
    let home = home("agent-improve-apart");
    let steady = script(&home, "steady", json!([{"say": "I am not sure."}]));
    let lead = profile(vec![steady]);
    let tasks = json!({"name": "sums", "cases": [{
        "id": "six-sevens", "task": "What is six times seven?", "checks": ["done"]
    }]})
    .to_string();
    let words = [Tweak::Instructions {
        words: "Answer.".into(),
    }];
    // The same tasks held out are not held out.
    let same = self_improve::try_changes(
        &lead,
        tasks.as_bytes(),
        tasks.as_bytes(),
        &words,
        1,
        1_000,
        0,
    )
    .unwrap_err();
    assert!(same.to_string().contains("held-out"), "{same}");
    // Nothing to try, words that are empty, a vote of one: refused.
    assert!(self_improve::try_changes(&lead, tasks.as_bytes(), b"{}", &[], 1, 1_000, 0).is_err());
    assert!(self_improve::check(&Tweak::Instructions { words: " ".into() }).is_err());
    assert!(self_improve::check(&Tweak::Working {
        arm: Arm::Vote { samples: 1 }
    })
    .is_err());
    assert!(self_improve::proposals(&[], &["math".to_string()], &[], None).is_err());
    let _ = std::fs::remove_dir_all(&home);
}
