//! The receipts a session keeps of its models' answers
//! ([`sion_agent::receipt`]).

use serde_json::json;
use sion_agent::model::{Model, ModelError};
use sion_agent::receipt::{self, Purpose};
use sion_agent::scripted::Scripted;
use sion_agent::session::Extras;
use sion_agent::tools::FnTool;
use sion_agent::{Autonomy, Effect, Ended, Limits, Outcome, Policy, Runtime, State, Toolbox};

fn now() -> u64 {
    1_000
}

fn never() -> bool {
    false
}

fn books() -> Toolbox {
    let mut tools = Toolbox::new();
    tools.add(FnTool::new(
        "count_open",
        "how many invoices are open",
        json!({"type": "object"}),
        Effect::Read,
        |_| Outcome::ok("3"),
    ));
    tools
}

fn run(models: Vec<&dyn Model>) -> State {
    let tools = books();
    let rt = Runtime {
        models,
        tools: &tools,
        policy: Policy {
            autonomy: Autonomy::L6,
            ..Policy::default()
        },
        limits: Limits::default(),
        system: "You keep the books.".into(),
        now: &now,
        stopped: &never,
        watch: None,
        steer: None,
        checkpoint: None,
        verify: None,
        code: None,
        hooks: None,
        extras: Extras::default(),
    };
    let mut state = State::start("r1", "How many invoices are open?", &rt.limits);
    assert!(matches!(rt.run(&mut state), Ended::Done { .. }));
    state
}

#[test]
fn every_answer_of_a_model_has_a_receipt_in_a_chain() {
    let big = Scripted::new("big-1")
        .calls("count_open", json!({}))
        .using(120, 8)
        .says("3 invoices are open.")
        .using(140, 6);
    let state = run(vec![&big]);
    let receipts = &state.receipts;
    assert_eq!(receipts.count, 2);
    receipts.check().unwrap();
    // What was sent is what the model was sent, and the tokens are the
    // session's.
    let sent = big.requests();
    assert_eq!(receipts.kept[0].request, receipt::request_digest(&sent[0]));
    assert_eq!(receipts.kept[1].request, receipt::request_digest(&sent[1]));
    assert_ne!(receipts.kept[0].request, receipts.kept[1].request);
    assert_eq!(receipts.kept[0].purpose, Purpose::Turn);
    assert_eq!(
        (
            receipts.kept[0].asked.as_str(),
            receipts.kept[0].answered.as_str()
        ),
        // A scripted model says that it is one.
        ("big-1", "scripted")
    );
    assert_eq!(
        receipts.kept[1].line(),
        "1: scripted (asked for big-1), 140 in and 6 out"
    );
    assert!(receipts.kept[0].endpoint.is_empty());
    assert_eq!(receipts.tallies.len(), 1);
    assert_eq!(receipts.tallies[0].calls, 2);
    assert_eq!(receipts.tallies[0].usage, state.usage);
    assert_eq!((state.usage.input, state.usage.output), (260, 14));

    // Kept with the session, they read back as they were.
    let saved = serde_json::to_string(&state).unwrap();
    let back: State = serde_json::from_str(&saved).unwrap();
    assert_eq!(back.receipts, state.receipts);
    back.receipts.check().unwrap();
}

#[test]
fn a_receipt_names_the_model_that_answered_when_the_first_did_not() {
    // The model the agent is set up with is busy; the next one answers.
    let big = Scripted::new("big-1").fails(ModelError::Busy("overloaded".into()));
    let small = Scripted::new("small-1")
        .calls("count_open", json!({}))
        .says("3 invoices are open.");
    let state = run(vec![&big, &small]);
    let receipts = &state.receipts;
    receipts.check().unwrap();
    assert!(receipts.count >= 2);
    assert!(receipts
        .kept
        .iter()
        .all(|r| r.asked == "small-1" && r.answered == "scripted"));
    // A model that gave no answer has no receipt.
    assert!(receipts.tallies.iter().all(|t| t.asked != "big-1"));
}
