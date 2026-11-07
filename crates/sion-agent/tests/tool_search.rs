//! A toolbox too large to show whole: its own tools stay in view, and what
//! a search finds is loaded, to be called directly by name.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use serde_json::json;
use sion_agent::scripted::Scripted;
use sion_agent::tools::{FnTool, META_THRESHOLD, TOOLS_CALL, TOOLS_SEARCH};
use sion_agent::{Autonomy, Effect, Ended, Limits, Outcome, Policy, Runtime, State, Toolbox};

fn now() -> u64 {
    1_000
}

fn never() -> bool {
    false
}

#[test]
fn what_a_search_finds_is_loaded_and_called_by_name() {
    let mut tools = Toolbox::new();
    for name in ["read_file", "write_file"] {
        tools.add(FnTool::new(
            name,
            "a file of the workspace",
            json!({"type": "object"}),
            Effect::Read,
            |_| Outcome::ok("ok"),
        ));
    }
    for i in 0..META_THRESHOLD {
        tools.add_deferred(Box::new(FnTool::new(
            &format!("crm_{i:02}"),
            "a record of the customer database",
            json!({"type": "object"}),
            Effect::Read,
            |_| Outcome::ok("ok"),
        )));
    }
    let sent = Arc::new(AtomicUsize::new(0));
    let counted = sent.clone();
    tools.add_deferred(Box::new(FnTool::new(
        "mail_send",
        "Sends an email message to a recipient",
        json!({"type": "object", "properties": {"to": {"type": "string", "description": "the recipient's address"}}}),
        Effect::Read,
        move |_| {
            counted.fetch_add(1, Ordering::SeqCst);
            Outcome::ok("sent")
        },
    )));
    let model = Scripted::new("m")
        .calls(
            TOOLS_SEARCH,
            json!({"query": "email a recipient", "limit": 2}),
        )
        .calls("mail_send", json!({"to": "ana@example.org"}))
        .says("Sent.");
    let rt = Runtime {
        models: vec![&model],
        tools: &tools,
        policy: Policy {
            autonomy: Autonomy::L3,
            ..Policy::default()
        },
        limits: Limits::default(),
        system: "You are a test agent.".into(),
        now: &now,
        stopped: &never,
        watch: None,
        steer: None,
        checkpoint: None,
        verify: None,
        code: None,
        hooks: None,
        extras: Default::default(),
    };
    let mut state = State::start("s-search", "Email Ana", &rt.limits);
    assert_eq!(
        rt.run(&mut state),
        Ended::Done {
            text: "Sent.".into()
        }
    );
    assert_eq!(sent.load(Ordering::SeqCst), 1, "called directly, by name");
    let requests = model.requests();
    let shown = |at: usize| -> Vec<String> {
        requests[at]
            .tools
            .iter()
            .map(|spec| spec.name.clone())
            .collect()
    };
    assert_eq!(
        shown(0),
        ["read_file", "write_file", TOOLS_SEARCH, TOOLS_CALL],
        "its own tools in view, and the way to the rest"
    );
    assert_eq!(
        shown(1),
        [
            "read_file",
            "write_file",
            TOOLS_SEARCH,
            TOOLS_CALL,
            "mail_send"
        ],
        "what the search found, loaded after them"
    );
    assert_eq!(state.loaded_tools, ["mail_send"]);
    // The search's answer says so, and gives the schema.
    let answer = state.messages[2].content[0].clone();
    let sion_agent::Block::ToolResult { content, .. } = answer else {
        panic!("a result: {answer:?}");
    };
    assert!(content.starts_with("Loaded, and callable by name from now on: mail_send."));
    assert!(content.contains("the recipient's address"));
    // A session kept and resumed keeps what it loaded.
    let kept: State = serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
    assert_eq!(kept.loaded_tools, ["mail_send"]);
    // A search that finds nothing says so, and loads nothing.
    let model = Scripted::new("m2")
        .calls(TOOLS_SEARCH, json!({"query": "quantum chromodynamics"}))
        .says("None.");
    let rt = Runtime {
        models: vec![&model],
        ..rt
    };
    let mut state = State::start("s-none", "Find a physics tool", &rt.limits);
    rt.run(&mut state);
    assert!(state.loaded_tools.is_empty());
    let sion_agent::Block::ToolResult { content, .. } = &state.messages[2].content[0] else {
        panic!("a result");
    };
    assert!(content.starts_with("No tool matches"));
}
