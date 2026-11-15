//! Keeping a long conversation short, measured and improved, with scripted
//! models and no node: an agent that loses whom to pay when its
//! conversation is shortened; the improvement loop, its tasks held to a
//! short context, shown what the shortening cost and keeping only the
//! change to how the conversation is kept that sealed tasks confirm; the
//! change made the profile's, reaching its sessions, and taken back; a
//! session's context shown; recorded sessions played again under two ways
//! of keeping them.

use std::sync::Mutex;

use serde_json::{json, Value};
use sion_agent::compare::TaskSet;
use sion_agent::earlier::{Keeping, LOOK_BACK};
use sion_agent::evals::{Answer, Case, Check, Fixture};
use sion_agent::scripted::Scripted;
use sion_agent::tools::FnTool;
use sion_agent::{Autonomy, Effect, Ended, Limits, Outcome, Policy, Runtime, State, Toolbox};
use sion_cli::context::{self, How};
use sion_cli::improve_loop::{self, Problem, Settings, Short, Sinks, Variant};
use sion_cli::profiles::{self, Profile};
use sion_cli::runtime::Saved;
use sion_cli::sealed;
use sion_cli::self_improve::{self, ImproveCommand};
use sion_core::ids::ObjectId;

static ENVIRONMENT: Mutex<()> = Mutex::new(());

fn home(name: &str) -> std::path::PathBuf {
    let home = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    std::env::set_var("SION_HOME", &home);
    home
}

fn script(home: &std::path::Path, name: &str, steps: Value) -> String {
    let path = home.join(format!("{name}.json"));
    std::fs::write(&path, steps.to_string()).unwrap();
    format!("script:{}", path.display())
}

fn profile(name: &str, agent: ObjectId, models: Vec<String>) -> Profile {
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
        instructions: Some("You pay invoices once you have read their pages.".into()),
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
        expect: false,
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

const PAYEES: [&str; 4] = ["carol", "dave", "erin", "frank"];

/// The small print an invoice begins and ends with: longer than mechanical
/// notes keep of either end of a result, so that whom to pay is between.
fn small_print() -> String {
    "Terms and small print. ".repeat(16)
}

/// A task: read an invoice, look at eight pages, then pay whom it names.
fn invoice(n: usize) -> Case {
    let payee = PAYEES[n % PAYEES.len()];
    let fixture =
        |name: &str, effect: Effect, says: Option<String>, otherwise: Option<&str>| Fixture {
            name: name.into(),
            description: format!("The {name} tool."),
            effect,
            answers: says
                .map(|says| Answer {
                    when: Default::default(),
                    says,
                    error: false,
                    outside: false,
                    confidential: false,
                })
                .into_iter()
                .collect(),
            otherwise: otherwise.map(str::to_string),
            command_field: None,
        };
    Case {
        id: format!("invoice-{n}"),
        task: format!("Read invoice {n}, look at its eight pages, then settle it."),
        tools: vec![
            fixture(
                "read_invoice",
                Effect::Read,
                Some(format!(
                    "{0} Pay {payee} {1} USD. {0}",
                    small_print(),
                    5 + n
                )),
                None,
            ),
            fixture("look", Effect::Read, None, Some("a page")),
            fixture("pay", Effect::Write, None, Some("paid")),
        ],
        checks: vec![
            Check::Done,
            Check::Called {
                tool: "pay".into(),
                with: json!({"to": payee}).as_object().unwrap().clone(),
            },
        ],
        person: None,
    }
}

fn tasks(name: &str, cases: std::ops::Range<usize>) -> TaskSet {
    TaskSet {
        name: name.into(),
        about: "Invoices to read, then pay.".into(),
        system: None,
        cases: cases.map(invoice).collect(),
    }
}

/// The agent: it reads the invoice and its pages, then pays whoever its
/// conversation still says to pay, and someone else if it no longer says.
/// It never thinks to look back.
fn agent_steps() -> Value {
    let mut steps = vec![json!({"call": "read_invoice", "input": {}})];
    for i in 0..8 {
        steps.push(json!({"call": "look", "input": {"i": i}}));
    }
    for payee in PAYEES {
        steps.push(json!({"when": format!("pay {payee}"), "call": "pay", "input": {"to": payee}}));
    }
    steps.push(json!({"unless": "paid", "call": "pay", "input": {"to": "someone"}}));
    steps.push(json!({"say": "Done."}));
    Value::Array(steps)
}

/// The writer of its notes: it says whom to pay only when it is asked
/// something other than the runtime's own question, which does not ask.
fn writer_steps() -> Value {
    let mut steps = Vec::new();
    for _ in 0..12 {
        for payee in PAYEES {
            steps.push(json!({
                "when": format!("pay {payee}"),
                "unless": "facts needed later",
                "say": format!("Read the invoice: pay {payee}. Pages looked at.")
            }));
        }
    }
    for _ in 0..12 {
        steps.push(json!({"say": "Read the invoice. Pages looked at."}));
    }
    Value::Array(steps)
}

const ASKED: &str = "Write the notes. Say who is to be paid and how much.";

/// The proposer: it changes how the conversation is kept only when it is
/// shown what shortening it cost.
fn proposer_steps() -> Value {
    json!([
        {"when": "what shortening its conversation cost it", "calls": [
            {"call": "propose_change", "input": {"context": {"keep_last": 100}, "why": "keep everything"}},
            {"call": "propose_change", "input": {"context": {"asked": ASKED}, "why": "its notes lost who was to be paid"}}
        ]},
        {"say": "Done."}
    ])
}

fn settings(short: Option<Short>) -> Settings {
    Settings {
        rounds: 1,
        runs: 2,
        budget_tokens: 50_000,
        total_tokens: 0,
        models: Vec::new(),
        seed: 3,
        offered: Vec::new(),
        lessons_only: false,
        short,
    }
}

#[test]
fn the_loop_changes_how_a_conversation_is_kept_only_on_what_a_short_context_shows() {
    let _one = ENVIRONMENT.lock().unwrap_or_else(|e| e.into_inner());
    let home = home("agent-context-loop");
    let agent = script(&home, "agent", agent_steps());
    let writer = script(&home, "writer", writer_steps());
    let clerk = profile("clerk", ObjectId::derive(b"clerk"), vec![agent]);
    profiles::save_all(&[("clerk".to_string(), clerk.clone())].into()).unwrap();
    let train = tasks("train", 0..6);
    let held = tasks("held", 6..12);
    let (train_digest, held_digest) = (
        sion_cli::practice::digest_of(&train).unwrap(),
        sion_cli::practice::digest_of(&held).unwrap(),
    );
    let problem = Problem {
        train: &train,
        train_digest,
        sealed: &held,
        sealed_digest: held_digest,
        z: sion_agent::compare::z_for(sealed::ALPHA / 2.0),
        reusable: false,
        practice: None,
        challenger: None,
    };
    let start = Variant::of(&clerk);
    assert_eq!(start.context, None);

    // As its tasks are, the agent passes them all: its conversation is
    // never shortened, so nothing shows what shortening would cost it.
    let plain = improve_loop::score(&clerk, &start, &train, train_digest, 2, 50_000, None).unwrap();
    assert_eq!((plain.outcome.passed, plain.outcome.total), (12, 12));
    assert!(plain.result.trials.iter().all(|t| t.shortened.is_empty()));

    // Held to a short context it loses whom to pay, and every run says
    // what the shortening cost.
    let short = Short::of(8, Some(writer.clone())).unwrap();
    let tight = improve_loop::score(
        &clerk,
        &start,
        &train,
        train_digest,
        2,
        50_000,
        Some(&short),
    )
    .unwrap();
    assert_eq!((tight.outcome.passed, tight.outcome.total), (0, 12));
    let cost = &tight.result.trials[0].shortened;
    assert!(
        cost[0].starts_with("its conversation was shortened ") && cost[0].contains("time(s)"),
        "{cost:?}"
    );
    let (shown, lines) = improve_loop::shortened_shown(&train, &tight.result, 12);
    assert!(lines >= 6 && shown.contains("- Task invoice-0 (in 2 runs, all failed): its"));

    // The loop, under the short context: a way no session takes is turned
    // down, and the change that has the notes say whom to pay is kept, on
    // tasks the proposer was never shown.
    let run = |short: Option<Short>| {
        let proposer =
            sion_cli::runtime::model(&script(&home, "proposer", proposer_steps()), 0).unwrap();
        let mut used = 0u32;
        let mut spend = || -> Result<(u32, u32), sion_cli::error::CliError> {
            used += 1;
            Ok((used, 2))
        };
        let mut ask = |_: f64, _: f64| -> Result<improve_loop::Asked, sion_cli::error::CliError> {
            Err(sion_cli::error::CliError::Usage(
                "not a reusable holdout".into(),
            ))
        };
        let mut kept =
            |_: &mut improve_loop::Step| -> Result<Vec<String>, sion_cli::error::CliError> {
                Ok(Vec::new())
            };
        let mut sinks = Sinks {
            spend: &mut spend,
            ask: &mut ask,
            kept: &mut kept,
            stopped: &|| false,
            now: &|| 1_000,
        };
        improve_loop::improve(
            &clerk,
            &problem,
            proposer.as_ref(),
            &improve_loop::Improver::default(),
            &settings(short),
            None,
            &mut sinks,
        )
        .unwrap()
    };
    let without = run(None);
    assert!(without.kept.is_empty(), "{:#?}", without.lines);
    assert!(
        without
            .stopped
            .as_deref()
            .unwrap_or_default()
            .contains("passes every task"),
        "{:#?}",
        without.lines
    );
    let report = run(Some(short.clone()));
    let said = report.lines.join("\n");
    assert!(
        said.contains("what shortening its conversation cost it:"),
        "{said}"
    );
    assert!(
        said.contains("turned down: keeps its last 100 messages as they are (it kept 12): it keeps between 2 and 64 messages as they are"),
        "{said}"
    );
    assert_eq!(report.kept.len(), 1, "{said}");
    let step = &report.kept[0];
    let kept = Keeping {
        asked: ASKED.into(),
        ..Keeping::default()
    };
    assert_eq!(step.to.context, Some(kept.clone()));
    assert_eq!(step.change, format!("asks it {ASKED:?}"));
    assert_eq!(
        (
            step.train_before.passed,
            step.train_after.passed,
            step.train_after.total
        ),
        (0, 12, 12)
    );
    assert_eq!(
        (
            step.sealed_before.passed,
            step.sealed_after.passed,
            step.sealed_after.total
        ),
        (0, 12, 12)
    );
    assert!(step.confirm.0 > 0.0, "{:?}", step.confirm);

    // Made the profile's, it is how its sessions keep their conversations.
    assert!(profiles::get("clerk")
        .unwrap()
        .equipment()
        .keeping
        .is_none());
    let lines = improve_loop::apply("clerk", step, 2_000).unwrap();
    assert_eq!(
        lines,
        vec![format!(
            "profile clerk now keeps its conversations as step {} kept",
            step.id
        )]
    );
    assert_eq!(improve_loop::context("clerk").unwrap().keeping, kept);
    assert_eq!(
        profiles::get("clerk").unwrap().equipment().keeping,
        Some(kept.clone())
    );
    assert_eq!(Variant::of(&clerk).context, Some(kept.clone()));
    // Taken back, they keep them the runtime's own way again.
    let lines = improve_loop::undo("clerk", 3_000).unwrap();
    assert!(
        lines[0].contains("how it keeps its conversations as they were before it"),
        "{lines:?}"
    );
    assert!(improve_loop::context("clerk").is_none());
    assert_eq!(Variant::of(&clerk).context, None);

    // The same from the command, on a seal: `--short-context` with the
    // notes' writer named, the change kept and made the profile's.
    let all = serde_json::to_vec_pretty(&tasks("invoices", 0..12)).unwrap();
    let seal = sealed::seal(&clerk, &all, 50, 2, 0, 1, 4_000).unwrap();
    assert_eq!(seal.train_cases + seal.sealed_cases, 12);
    self_improve::run(ImproveCommand::Run {
        profile: "clerk".into(),
        proposer: script(&home, "proposer-2", proposer_steps()),
        rounds: 1,
        runs: 2,
        budget_tokens: 50_000,
        total_tokens: 0,
        models: Vec::new(),
        seed: Some(3),
        practice: None,
        challenger: None,
        short_context: Some(8),
        notes_model: Some(writer),
        apply: true,
        wallet: None,
    })
    .unwrap();
    let lineage = improve_loop::lineage("clerk").unwrap();
    assert_eq!(lineage.len(), 1, "{lineage:#?}");
    assert_eq!(lineage[0].to.context, Some(kept.clone()));
    assert_eq!(improve_loop::context("clerk").unwrap().keeping, kept);
    assert_eq!(
        improve_loop::context("clerk").unwrap().step.as_deref(),
        Some(lineage[0].id.as_str())
    );
}

fn tool(
    name: &str,
    effect: Effect,
    answer: impl Fn(&Value) -> Outcome + Send + Sync + 'static,
) -> FnTool<impl Fn(&Value) -> Outcome + Send + Sync> {
    FnTool::new(
        name,
        &format!("the {name} tool"),
        json!({"type": "object"}),
        effect,
        answer,
    )
}

/// A session as it is kept on disk, around `state`.
fn saved(state: &State, limits: &Limits) -> Saved {
    serde_json::from_value(json!({
        "wallet": "clerk",
        "agent": ObjectId::derive(b"clerk"),
        "models": [],
        "policy": Policy::default(),
        "limits": limits,
        "system": "",
        "workspace": null,
        "web": false,
        "state": state,
        "ended": null,
    }))
    .unwrap()
}

/// A session that reads an invoice, looks at eight pages, and pays it by
/// its account: under `limits`, with `model`.
fn worked(id: &str, model: &Scripted, limits: &Limits) -> State {
    let mut tools = Toolbox::new();
    tools.add(tool("read_invoice", Effect::Read, |_| {
        Outcome::ok(format!(
            "{0} Pay carol 8 USD to account acct_00417. {0}",
            "Terms and small print. ".repeat(16)
        ))
    }));
    tools.add(tool("look", Effect::Read, |input| {
        Outcome::ok(format!("page {}", input["i"]))
    }));
    tools.add(tool("pay", Effect::Write, |_| Outcome::ok("paid")));
    let runtime = Runtime {
        models: vec![model],
        tools: &tools,
        policy: Policy {
            autonomy: Autonomy::L6,
            ..Policy::default()
        },
        limits: limits.clone(),
        system: "You pay invoices.".into(),
        now: &|| 1_000,
        stopped: &|| false,
        watch: None,
        steer: None,
        checkpoint: None,
        verify: None,
        code: None,
        hooks: None,
        extras: Default::default(),
    };
    let mut state = State::start(
        id,
        "Settle invoice INV-7731 once you read its pages.",
        limits,
    );
    assert!(matches!(runtime.run(&mut state), Ended::Done { .. }));
    state
}

fn reading() -> Scripted {
    let mut model = Scripted::new("m").calls("read_invoice", json!({}));
    for i in 0..8 {
        model = model.calls("look", json!({ "i": i }));
    }
    model
}

#[test]
fn a_session_s_context_is_shown_and_recorded_sessions_are_played_again() {
    let _one = ENVIRONMENT.lock().unwrap_or_else(|e| e.into_inner());
    let home = home("agent-context-shown");

    // A session never shortened.
    let limits = Limits::default();
    let model = reading()
        .calls(
            "pay",
            json!({"to": "carol", "account": "acct_00417", "invoice": "INV-7731"}),
        )
        .says("Paid.");
    let long = saved(&worked("long-1", &model, &limits), &limits);
    assert_eq!(
        context::lines(&long),
        vec!["session long-1: 22 messages; its conversation was never shortened"]
    );

    // The same work under a short context: it looks back for the account.
    let tight = Limits {
        compact_messages: 8,
        keep_last: 3,
        ..Limits::default()
    };
    let model = reading()
        .calls(LOOK_BACK, json!({"query": "account to pay"}))
        .unless("acct_00417 ")
        .calls(
            "pay",
            json!({"to": "carol", "account": "acct_00417", "invoice": "INV-7731"}),
        )
        .says("Paid.");
    let short = saved(&worked("short-1", &model, &tight), &tight);
    short.save().unwrap();
    let shown = context::lines(&Saved::load("short-1").unwrap());
    assert!(
        shown[0].starts_with("session short-1: ")
            && shown[0].contains(" let go and kept as they were ("),
        "{shown:#?}"
    );
    assert!(
        shown[1].starts_with("  1. let go messages 1 to ")
            && shown[1].contains("notes by the runtime (mechanical)"),
        "{shown:#?}"
    );
    assert!(
        shown.contains(&"looked back 1 time:".to_string()),
        "{shown:#?}"
    );
    assert!(
        shown.contains(&"  searched for \"account to pay\": found in message 2".to_string()),
        "{shown:#?}"
    );
    assert_eq!(
        shown.last().unwrap(),
        "the whole conversation can still be read: 24 messages"
    );
    context::run(context::ContextCommand::Show {
        session: "short-1".into(),
    })
    .unwrap();

    // The first session played again under a short context, as the
    // runtime keeps one: of the names its agent went on to use, the
    // invoice is in the task; the account is only listed as let go.
    let how = How {
        at: Some(8),
        keeping: Keeping {
            keep_last: 3,
            ..Keeping::default()
        },
        notes_model: None,
    };
    let replayed = context::measure(&long, &how).unwrap();
    assert!(replayed.shortenings >= 3, "{replayed:?}");
    // Every shortening before the payment is one at which both names were
    // still to be used.
    let last = replayed.needed / 2;
    assert!(last >= 3 && replayed.needed == 2 * last, "{replayed:?}");
    assert_eq!(
        (replayed.held, replayed.listed),
        (last, last),
        "{replayed:?}"
    );
    assert_eq!(replayed.lost[0].name, "acct_00417");
    assert_eq!(replayed.lost[0].used_at, 19);
    let lines = context::measured_lines("session long-1", &replayed);
    assert!(
        lines[1].contains(&format!(
            "of {} names its agent went on to use from what came before, the shortened \
             conversation still held {last} (50%); {last} more it listed as let go",
            2 * last
        )),
        "{lines:#?}"
    );
    // With no names listed, the account is nowhere in view.
    let bare = context::measure(
        &long,
        &How {
            keeping: Keeping {
                names: 0,
                ..how.keeping.clone()
            },
            ..how.clone()
        },
    )
    .unwrap();
    assert_eq!((bare.held, bare.listed), (last, 0), "{bare:?}");
    // With a writer asked to keep accounts, the notes hold it.
    let writer_steps: Vec<Value> = (0..12)
        .map(|_| json!({"say": "Read the invoice: pay carol 8 USD to account acct_00417. Pages looked at."}))
        .collect();
    let writer = script(&home, "accounts", Value::Array(writer_steps));
    let noted = context::measure(
        &long,
        &How {
            notes_model: Some(writer),
            keeping: Keeping {
                asked: "Write the notes. Keep every account.".into(),
                ..how.keeping.clone()
            },
            ..how.clone()
        },
    )
    .unwrap();
    assert_eq!(
        (noted.needed, noted.held),
        (2 * last, 2 * last),
        "{noted:?}"
    );
    // At its own limits it would never have been shortened.
    let own = context::measure(
        &long,
        &How {
            at: None,
            ..how.clone()
        },
    )
    .unwrap();
    assert_eq!(own.shortenings, 0);
    // A session whose conversation is not all kept is passed over.
    let mut lost = short.clone();
    lost.state.earlier.dropped = 1;
    assert!(context::measure(&lost, &how).is_err());
}
