//! What an agent says it expects of a call, and what the runtime does with
//! it ([`sion_agent::expect`]).

use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use sion_agent::expect::{EXPECT, EXPECTING, NOTE};
use sion_agent::model::{Block, Model};
use sion_agent::scripted::Scripted;
use sion_agent::tools::FnTool;
use sion_agent::{Autonomy, Effect, Ended, Limits, Outcome, Policy, Runtime, State, Toolbox};

fn now() -> u64 {
    1_000
}

fn never() -> bool {
    false
}

/// The inputs a tool was given, in order.
type Given = Arc<Mutex<Vec<Value>>>;

/// A tool that answers `answer` and keeps the inputs it was given.
fn kept(
    name: &str,
    schema: Value,
    effect: Effect,
    answer: impl Fn(&Value) -> Outcome + Send + Sync + 'static,
) -> (FnTool<impl Fn(&Value) -> Outcome + Send + Sync>, Given) {
    let given = Arc::new(Mutex::new(Vec::new()));
    let seen = given.clone();
    let tool = FnTool::new(
        name,
        &format!("the {name} tool"),
        schema,
        effect,
        move |input| {
            seen.lock().unwrap().push(input.clone());
            answer(input)
        },
    );
    (tool, given)
}

fn runtime<'a>(
    models: Vec<&'a dyn Model>,
    tools: &'a Toolbox,
    autonomy: Autonomy,
    expect: bool,
) -> Runtime<'a> {
    Runtime {
        models,
        tools,
        policy: Policy {
            autonomy,
            ..Policy::default()
        },
        limits: Limits {
            expect,
            ..Limits::default()
        },
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
    }
}

/// The results of a session's calls as the model was given them, in order.
fn results(state: &State) -> Vec<(String, bool)> {
    state
        .messages
        .iter()
        .flat_map(|message| &message.content)
        .filter_map(|block| match block {
            Block::ToolResult {
                content, is_error, ..
            } => Some((content.clone(), *is_error)),
            _ => None,
        })
        .collect()
}

fn path_schema() -> Value {
    json!({
        "type": "object",
        "properties": {"path": {"type": "string"}},
        "additionalProperties": false
    })
}

#[test]
fn an_agent_says_what_it_expects_and_is_told_only_where_it_was_wrong() {
    let (read, read_given) = kept("read", path_schema(), Effect::Read, |input| {
        Outcome::ok(format!("{}: Hello, world", input["path"].as_str().unwrap()))
    });
    let (test, test_given) = kept(
        "run_tests",
        json!({"type": "object"}),
        Effect::Write,
        |_| Outcome::ok("9 passed, 3 failed"),
    );
    let (pay, pay_given) = kept("pay", json!({"type": "object"}), Effect::Write, |_| {
        Outcome::error("refused: over the charter's limit")
    });
    let mut tools = Toolbox::new();
    tools.add(read);
    tools.add(test);
    tools.add(pay);
    let model = Scripted::new("m")
        // Two reads side by side: one as said, one with nothing said.
        .calls_all(vec![
            (
                "read".into(),
                json!({"path": "a.txt", "expect": {"outcome": "It greets.", "contains": ["hello"]}}),
            ),
            ("read".into(), json!({"path": "b.txt"})),
        ])
        // Not as said.
        .calls(
            "run_tests",
            json!({"expect": {"outcome": "the tests pass", "contains": ["0 failed"]}}),
        )
        // A failure that was expected is as said.
        .calls(
            "pay",
            json!({"to": "carol", "expect": {"outcome": "the charter refuses carol", "ok": false}}),
        )
        // One that says nothing that can be kept is taken off all the same.
        .calls("read", json!({"path": "c.txt", "expect": {}}))
        .says("Three tests fail.");
    let rt = runtime(vec![&model], &tools, Autonomy::L6, true);
    let mut state = State::start("e1", "Check the work.", &rt.limits);
    assert_eq!(
        rt.run(&mut state),
        Ended::Done {
            text: "Three tests fail.".into()
        }
    );

    // The model was told of expectations once, and shown the property on
    // every tool; no tool was given it.
    let requests = model.requests();
    assert!(requests[0].system.ends_with(EXPECTING));
    assert!(requests[0].system.starts_with("You are a test agent."));
    for tool in &requests[0].tools {
        assert_eq!(
            tool.input_schema["properties"][EXPECT]["type"], "object",
            "{}",
            tool.name
        );
    }
    // The two reads side by side were made in either order.
    let mut read = read_given.lock().unwrap().clone();
    read.sort_by_key(|input| input["path"].as_str().unwrap_or_default().to_string());
    assert_eq!(
        read,
        [
            json!({"path": "a.txt"}),
            json!({"path": "b.txt"}),
            json!({"path": "c.txt"})
        ]
    );
    assert_eq!(*test_given.lock().unwrap(), [json!({})]);
    assert_eq!(*pay_given.lock().unwrap(), [json!({"to": "carol"})]);

    // Nothing is said where a call went as expected, or where nothing was
    // expected; where it did not, the result says how.
    let results = results(&state);
    assert_eq!(results[0].0, "a.txt: Hello, world");
    assert_eq!(results[1].0, "b.txt: Hello, world");
    assert_eq!(
        results[2].0,
        format!(
            "9 passed, 3 failed\n{NOTE} the tests pass. What happened: the result does not hold \
             \"0 failed\".]"
        )
    );
    assert_eq!(
        results[3],
        ("refused: over the charter's limit".into(), true)
    );
    assert_eq!(results[4].0, "c.txt: Hello, world");

    // The session keeps the count and the miss.
    let expected = &state.expected;
    assert_eq!((expected.stated, expected.held, expected.sharp), (3, 2, 3));
    assert_eq!(expected.missed.len(), 1);
    assert_eq!(
        expected.missed[0].line(),
        "run_tests: expected \"the tests pass\"; the result does not hold \"0 failed\""
    );
    assert!(state.expecting);

    // The conversation keeps what the model wrote, and a record of the
    // session's calls reads them as they were made.
    let wrote: Vec<&Value> = state
        .messages
        .iter()
        .flat_map(|m| m.tool_uses())
        .map(|(_, _, input)| input)
        .collect();
    assert_eq!(wrote[0][EXPECT]["outcome"], "It greets.");
    let calls = sion_agent::evals::calls_of(&state);
    assert_eq!(calls[0], ("read".to_string(), json!({"path": "a.txt"})));
    assert!(calls.iter().all(|(_, input)| input.get(EXPECT).is_none()));

    // Saved and read again, it is the same.
    let saved = serde_json::to_string(&state).unwrap();
    let again: State = serde_json::from_str(&saved).unwrap();
    assert_eq!(again.expected, state.expected);
    assert!(again.expecting);
}

#[test]
fn an_expectation_waits_with_its_call_for_a_person() {
    let (write, given) = kept("write", path_schema(), Effect::Write, |_| {
        Outcome::error("the disk is full")
    });
    let mut tools = Toolbox::new();
    tools.add(write);
    let model = Scripted::new("m")
        .calls(
            "write",
            json!({"path": "a.txt", "expect": {"outcome": "a.txt is written"}}),
        )
        .says("It could not be written.");
    let rt = runtime(vec![&model], &tools, Autonomy::L2, true);
    let mut state = State::start("e2", "Write a.txt", &rt.limits);
    // The person is asked about the call as the tool will be given it.
    let Ended::NeedsApproval { input, .. } = rt.run(&mut state) else {
        panic!("expected a question");
    };
    assert_eq!(input, json!({"path": "a.txt"}));
    assert_eq!(state.expected.stated, 0);
    // Across a pause.
    let saved = serde_json::to_string(&state).unwrap();
    let mut state: State = serde_json::from_str(&saved).unwrap();
    assert_eq!(
        state
            .pending
            .as_ref()
            .unwrap()
            .expect
            .as_ref()
            .unwrap()
            .outcome,
        "a.txt is written"
    );
    assert!(rt.approve(&mut state));
    assert_eq!(
        rt.run(&mut state),
        Ended::Done {
            text: "It could not be written.".into()
        }
    );
    assert_eq!(*given.lock().unwrap(), [json!({"path": "a.txt"})]);
    let said = results(&state);
    assert_eq!(
        said[0].0,
        format!("the disk is full\n{NOTE} a.txt is written. What happened: the call failed.]")
    );
    assert_eq!((state.expected.stated, state.expected.held), (1, 0));

    // A call a person declines was never made: there is nothing to
    // compare, and nothing is counted.
    let (write, given) = kept("write", path_schema(), Effect::Write, |_| {
        Outcome::ok("written")
    });
    let mut tools = Toolbox::new();
    tools.add(write);
    let model = Scripted::new("m")
        .calls(
            "write",
            json!({"path": "a.txt", "expect": {"outcome": "a.txt is written"}}),
        )
        .says("They said no.");
    let rt = runtime(vec![&model], &tools, Autonomy::L2, true);
    let mut state = State::start("e3", "Write a.txt", &rt.limits);
    assert!(matches!(rt.run(&mut state), Ended::NeedsApproval { .. }));
    assert!(rt.decline(&mut state, "not now"));
    assert!(matches!(rt.run(&mut state), Ended::Done { .. }));
    assert!(given.lock().unwrap().is_empty());
    assert!(state.expected.is_empty());
    assert!(results(&state).iter().all(|(said, _)| !said.contains(NOTE)));
}

#[test]
fn without_the_switch_nothing_changes_and_a_tools_own_property_stays_its_own() {
    let own = json!({
        "type": "object",
        "properties": {"expect": {"type": "string"}, "actual": {"type": "string"}}
    });
    // Off: no property, no word in the system prompt, and an input called
    // `expect` is the tool's.
    let (assert_tool, given) = kept("assert_equal", own.clone(), Effect::Read, |_| {
        Outcome::ok("equal")
    });
    let (read, read_given) = kept("read", path_schema(), Effect::Read, |_| Outcome::ok("x"));
    let mut tools = Toolbox::new();
    tools.add(assert_tool);
    tools.add(read);
    let script = || {
        Scripted::new("m")
            .calls("assert_equal", json!({"expect": "4", "actual": "4"}))
            .calls(
                "read",
                json!({"path": "a.txt", "expect": {"outcome": "it says y", "contains": ["y"]}}),
            )
            .says("Done.")
    };
    let model = script();
    let rt = runtime(vec![&model], &tools, Autonomy::L6, false);
    let mut state = State::start("e4", "Compare.", &rt.limits);
    rt.run(&mut state);
    let requests = model.requests();
    assert_eq!(requests[0].system, "You are a test agent.");
    let shown = |name: &str| {
        requests[0]
            .tools
            .iter()
            .find(|tool| tool.name == name)
            .unwrap()
            .input_schema
            .clone()
    };
    assert_eq!(shown("assert_equal"), own);
    assert_eq!(shown("read"), path_schema());
    assert_eq!(
        *given.lock().unwrap(),
        [json!({"expect": "4", "actual": "4"})]
    );
    // The read's input does not fit its schema with a property it never
    // had: answered, not passed on.
    assert!(read_given.lock().unwrap().is_empty());
    assert!(state.expected.is_empty() && !state.expecting);

    // On: the tool with a property of that name keeps its own and is given
    // it; the other's expectation is the runtime's.
    given.lock().unwrap().clear();
    let model = script();
    let rt = runtime(vec![&model], &tools, Autonomy::L6, true);
    let mut state = State::start("e5", "Compare.", &rt.limits);
    rt.run(&mut state);
    let requests = model.requests();
    let shown = |name: &str| {
        requests[0]
            .tools
            .iter()
            .find(|tool| tool.name == name)
            .unwrap()
            .input_schema
            .clone()
    };
    assert_eq!(shown("assert_equal"), own);
    assert_eq!(shown("read")["properties"][EXPECT]["type"], "object");
    assert_eq!(
        *given.lock().unwrap(),
        [json!({"expect": "4", "actual": "4"})]
    );
    assert_eq!(*read_given.lock().unwrap(), [json!({"path": "a.txt"})]);
    assert_eq!((state.expected.stated, state.expected.held), (1, 0));
    assert_eq!(state.expected.missed[0].tool, "read");
}

#[test]
fn a_tool_called_by_name_takes_an_expectation_beside_its_name_or_with_its_input() {
    // More tools than are shown at once: the rest are called by name.
    let mut tools = Toolbox::new();
    let mut given = None;
    for i in 0..=sion_agent::tools::META_THRESHOLD {
        let (tool, kept_inputs) = kept(
            &format!("mcp_{i:02}"),
            path_schema(),
            Effect::Read,
            |input| Outcome::ok(format!("{}: Hello", input["path"].as_str().unwrap())),
        );
        tools.add_deferred(Box::new(tool));
        if i == 7 {
            given = Some(kept_inputs);
        }
    }
    let given = given.unwrap();
    let model = Scripted::new("m")
        .calls(
            "tools_call",
            json!({
                "name": "mcp_07",
                "input": {"path": "a"},
                "expect": {"outcome": "it says goodbye", "contains": ["goodbye"]}
            }),
        )
        .calls(
            "tools_call",
            json!({
                "name": "mcp_07",
                "input": {"path": "b", "expect": {"outcome": "it greets", "contains": ["hello"]}}
            }),
        )
        .says("Done.");
    let rt = runtime(vec![&model], &tools, Autonomy::L6, true);
    let mut state = State::start("e6", "Read them.", &rt.limits);
    assert!(matches!(rt.run(&mut state), Ended::Done { .. }));
    // The tool is given neither; its schema allows nothing else.
    assert_eq!(
        *given.lock().unwrap(),
        [json!({"path": "a"}), json!({"path": "b"})]
    );
    let said = results(&state);
    assert_eq!(
        said[0].0,
        format!(
            "a: Hello\n{NOTE} it says goodbye. What happened: the result does not hold \
             \"goodbye\".]"
        )
    );
    assert_eq!(said[1], ("b: Hello".into(), false));
    assert_eq!((state.expected.stated, state.expected.held), (2, 1));
    assert_eq!(state.expected.missed[0].tool, "mcp_07");
    // A record of its calls reads them as the tool was given them.
    assert_eq!(
        sion_agent::evals::calls_of(&state),
        [
            ("mcp_07".to_string(), json!({"path": "a"})),
            ("mcp_07".to_string(), json!({"path": "b"}))
        ]
    );
    // The tool that calls by name is shown with the property.
    let requests = model.requests();
    let by_name = requests[0]
        .tools
        .iter()
        .find(|tool| tool.name == "tools_call")
        .unwrap();
    assert_eq!(by_name.input_schema["properties"][EXPECT]["type"], "object");
}
