//! On a devnet: subagents that change code in git worktrees of their
//! parent's workspace, side by side, their work committed on branches of
//! their own; a program that calls the session's tools that read; and the
//! operator's hooks, one of which blocks a command; and a graph's attempts
//! at a change, each in a worktree of its own, the one picked merged.
//! Skipped where bubblewrap does not work.

use std::path::Path;
use std::sync::Arc;

use serde_json::{json, Value};
use sion_agent::sandbox::{Backend, Sandbox};
use sion_agent::{Autonomy, Block, Ended, Policy};
use sion_cli::ledger::LedgerExt;
use sion_cli::runtime::{self, Equipment, SharedLedger};
use sion_cli::session::Session;
use sion_core::ids::{AssetId, ObjectId, Pulse};
use sion_core::Amount;
use sion_exec::charter::Caveats;
use sion_exec::execute::Event;
use sion_exec::TxKind;
use sion_node::{Devnet, DevnetConfig};

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
                    may_delegate: true,
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
        .fund(wallet, AssetId::usd(), Amount::units(2))
        .unwrap();
    agent
}

fn script(home: &Path, name: &str, steps: Value) -> String {
    let path = home.join(name);
    std::fs::write(&path, steps.to_string()).unwrap();
    format!("script:{}", path.display())
}

fn results(saved: &runtime::Saved) -> Vec<(String, bool)> {
    saved
        .state
        .messages
        .iter()
        .flat_map(|m| &m.content)
        .filter_map(|b| match b {
            Block::ToolResult {
                content, is_error, ..
            } => Some((content.clone(), *is_error)),
            _ => None,
        })
        .collect()
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "git {args:?}: {output:?}");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

#[test]
fn subagents_change_code_in_worktrees_and_programs_and_hooks_work_around_calls() {
    let sandbox = Sandbox::new(Backend::Bubblewrap);
    if let Err(e) = sandbox.probe() {
        eprintln!("skipped: bubblewrap does not work here: {e}");
        return;
    }
    let base = std::env::temp_dir().join(format!("sion-worktrees-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let home = base.join("home");
    let work = base.join("work");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&work).unwrap();
    std::env::set_var("SION_HOME", &home);
    git(&work, &["init", "--quiet", "-b", "main"]);
    std::fs::write(work.join("notes.txt"), "first\n").unwrap();
    git(&work, &["add", "notes.txt"]);
    git(
        &work,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "--quiet",
            "-m",
            "first",
        ],
    );

    let devnet = Arc::new(
        Devnet::start(DevnetConfig {
            rounds_per_pulse: 2,
            ..DevnetConfig::default()
        })
        .unwrap(),
    );
    let ledger: SharedLedger = devnet.clone();
    let mut session = Session::open(ledger.as_ref()).unwrap();
    session
        .fund("alice", AssetId::usd(), Amount::units(1_000))
        .unwrap();
    let agent = register(&mut session, &ledger, "coder");
    let policy = Policy {
        autonomy: Autonomy::L6,
        ..Policy::default()
    };

    // 1. Two subagents in one turn, each in a worktree of its own.
    let lead = script(
        &home,
        "lead.json",
        json!([
            {"calls": [
                {"call": "agent_spawn", "input": {"name": "left", "budget": "1", "task": "Write the left half.", "worktree": true}},
                {"call": "agent_spawn", "input": {"name": "right", "budget": "1", "task": "Write the right half.", "worktree": true}}
            ]},
            {"say": "Both halves are on their branches."}
        ]),
    );
    let child = script(
        &home,
        "child.json",
        json!([
            {"call": "write_file", "input": {"path": "half.txt", "content": "a half\n"}},
            {"call": "shell", "input": {"command": "cat half.txt notes.txt"}},
            {"say": "Wrote half.txt."}
        ]),
    );
    let (saved, lines) = runtime::start(
        ledger.clone(),
        "coder",
        agent,
        "Write both halves, through two subagents.",
        vec![lead],
        policy.clone(),
        Equipment {
            workspace: Some(work.clone()),
            shell: Some(sandbox.clone()),
            delegate: true,
            subagent_models: vec![child],
            ..Equipment::default()
        },
    )
    .unwrap();
    let text = lines.join("\n");
    assert!(matches!(saved.ended, Some(Ended::Done { .. })), "{text}");
    let reports = results(&saved);
    assert_eq!(reports.len(), 2, "{text}");
    for ((report, failed), name) in reports.iter().zip(["left", "right"]) {
        assert!(!failed, "{report}");
        assert!(report.contains("write_file ok, shell ok"), "{report}");
        assert!(
            report.contains(&format!(
                "Its work is on branch sion/{name} (1 file changed"
            )),
            "{report}"
        );
        assert_eq!(
            git(&work, &["show", &format!("sion/{name}:half.txt")]),
            "a half"
        );
        assert_eq!(
            git(
                &work,
                &["log", "-1", "--format=%s", &format!("sion/{name}")]
            ),
            format!("Write the {name} half.")
        );
    }
    assert!(
        !work.join("half.txt").exists(),
        "the parent's files are as they were"
    );
    assert_eq!(
        git(&work, &["worktree", "list"]).lines().count(),
        1,
        "worktrees removed"
    );
    // A subagent asked for a worktree where there is none is refused.
    let lone = base.join("lone");
    std::fs::create_dir_all(&lone).unwrap();
    let lead = script(
        &home,
        "lead-lone.json",
        json!([
            {"call": "agent_spawn", "input": {"name": "x", "budget": "1", "task": "Fix it.", "worktree": true}},
            {"say": "No."}
        ]),
    );
    let (saved, _) = runtime::start(
        ledger.clone(),
        "coder",
        agent,
        "Fix it, through a subagent.",
        vec![lead],
        policy.clone(),
        Equipment {
            workspace: Some(lone.clone()),
            shell: Some(sandbox.clone()),
            delegate: true,
            ..Equipment::default()
        },
    )
    .unwrap();
    let (refusal, failed) = results(&saved).pop().unwrap();
    assert!(failed && refusal.contains("no worktree"), "{refusal}");

    // 2. A program that reads the workspace's files through the tools.
    let programmer = script(
        &home,
        "program.json",
        json!([
            {"call": "run_code", "input": {"code": "listed = tools.list_files()\nprint('notes.txt' in listed, tools.read_file(path='notes.txt').strip())"}},
            {"say": "It holds first."}
        ]),
    );
    let (saved, lines) = runtime::start(
        ledger.clone(),
        "coder",
        agent,
        "What does notes.txt say?",
        vec![programmer],
        policy.clone(),
        Equipment {
            workspace: Some(work.clone()),
            shell: Some(sandbox.clone()),
            ..Equipment::default()
        },
    )
    .unwrap();
    let (printed, failed) = results(&saved).pop().unwrap();
    if printed.contains("python3 is not in the sandbox") {
        eprintln!("skipped the program: no python3 in the sandbox");
    } else {
        assert!(!failed, "{printed}\n{}", lines.join("\n"));
        assert!(printed.starts_with("True "), "{printed}");
        assert!(printed.contains("first"), "{printed}");
    }

    // 3. The operator's hook blocks a command that removes files, and
    // another notes each command's end.
    let guard = base.join("guard.sh");
    std::fs::write(
        &guard,
        "#!/bin/sh\nif grep -q '\"rm '; then echo 'removing files is not allowed here' >&2; exit 2; fi\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&guard, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let hooks = json!([
        {"event": "before-tool", "tools": ["shell"], "command": [guard.to_string_lossy()]},
        {"event": "after-tool", "tools": ["shell"], "command": ["/bin/sh", "-c", "cat >/dev/null; echo checked"]}
    ]);
    let hooks_file = home.join("agents").join("hooks.json");
    std::fs::create_dir_all(hooks_file.parent().unwrap()).unwrap();
    std::fs::write(&hooks_file, hooks.to_string()).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // Others may change it: refused.
        std::fs::set_permissions(&hooks_file, std::fs::Permissions::from_mode(0o666)).unwrap();
        let refused = runtime::start(
            ledger.clone(),
            "coder",
            agent,
            "Anything.",
            vec![script(&home, "noop.json", json!([{"say": "ok"}]))],
            policy.clone(),
            Equipment::default(),
        );
        assert!(
            refused.is_err_and(|e| e.to_string().contains("may be changed by others")),
            "hooks others may change were run"
        );
        std::fs::set_permissions(&hooks_file, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let commands = script(
        &home,
        "commands.json",
        json!([
            {"call": "shell", "input": {"command": "rm notes.txt"}},
            {"call": "shell", "input": {"command": "cat notes.txt"}},
            {"say": "notes.txt is still there."}
        ]),
    );
    let (saved, _) = runtime::start(
        ledger.clone(),
        "coder",
        agent,
        "Clean up.",
        vec![commands],
        policy.clone(),
        Equipment {
            workspace: Some(work.clone()),
            shell: Some(sandbox.clone()),
            ..Equipment::default()
        },
    )
    .unwrap();
    let results = results(&saved);
    assert_eq!(
        results[0],
        (
            "blocked by the operator's hook: removing files is not allowed here".into(),
            true
        )
    );
    assert!(results[1].0.contains("first"), "{:?}", results[1]);
    assert!(
        results[1].0.ends_with("[the operator's hook adds] checked"),
        "{:?}",
        results[1]
    );
    assert!(work.join("notes.txt").exists());
    let _ = std::fs::remove_file(&hooks_file);

    // 4. Three attempts at a change, each in a worktree of its own, side by
    // side; a judge picks the second, and its branch is merged.
    let team = sion_cli::profiles::Profile {
        name: "team".into(),
        wallet: "coder".into(),
        agent,
        models: vec![script(
            &home,
            "attempts.json",
            json!([
                {"when": "this is attempt 1 of 3", "call": "write_file", "input": {"path": "fix.txt", "content": "fixed once\n"}},
                {"when": "this is attempt 1 of 3", "say": "Wrote fix.txt."},
                {"when": "this is attempt 2 of 3", "call": "write_file", "input": {"path": "fix.txt", "content": "fixed well\n"}},
                {"when": "this is attempt 2 of 3", "say": "Wrote fix.txt, and read it back."},
                {"when": "this is attempt 3 of 3", "say": "Nothing needed changing."},
                {"when": "you judge attempts", "say": "{\"pick\": 2, \"why\": \"it read its work back\"}"}
            ]),
        )],
        autonomy: Autonomy::L6,
        memory: None,
        skills: None,
        workspace: Some(work.clone()),
        web: false,
        delegate: false,
        subagent_models: Vec::new(),
        adjudicator: None,
        reflect: false,
        shell: Some(sandbox),
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
        quarantine: false,
        shared_memory: Vec::new(),
        confidential: false,
        confidential_files: Vec::new(),
        routes: Vec::new(),
        sorter: None,
    };
    let graph = sion_agent::graph::template("attempts").unwrap();
    let (kept, lines) = sion_cli::graphs::run(ledger.clone(), &team, graph, "Fix it.").unwrap();
    let text = lines.join("\n");
    let sion_agent::graph::Standing::Done(answer) = &kept.run.standing else {
        panic!("{text}");
    };
    let short: String = kept.id.chars().take(8).collect();
    let second = format!("sion/graph-{short}-attempt-2");
    assert!(
        answer
            .text
            .starts_with("Wrote fix.txt, and read it back.\n\n[What it changed, on branch"),
        "{text}"
    );
    assert!(
        answer.text.ends_with(&format!(
            "[Picked answer 2 of 3: it read its work back. Adopted: merged branch {second} into the workspace.]"
        )),
        "{text}"
    );
    // The judge was shown each attempt's change, numbered.
    let pick = &kept.run.ran[3].activation;
    assert_eq!(pick.inputs.len(), 3, "{text}");
    assert!(
        pick.inputs
            .iter()
            .any(|(_, said)| said.contains("+fixed well")),
        "{text}"
    );
    assert!(
        pick.inputs
            .iter()
            .any(|(_, said)| said.ends_with("[It changed no files.]")),
        "{text}"
    );
    // The second attempt's work is the workspace's now; the first's stays
    // on its branch, and every worktree is gone.
    assert_eq!(
        std::fs::read_to_string(work.join("fix.txt")).unwrap(),
        "fixed well\n"
    );
    assert_eq!(
        git(
            &work,
            &["show", &format!("sion/graph-{short}-attempt-1:fix.txt")]
        ),
        "fixed once"
    );
    assert_eq!(
        git(&work, &["worktree", "list"]).lines().count(),
        1,
        "worktrees removed"
    );
    let _ = std::fs::remove_dir_all(&base);
}
