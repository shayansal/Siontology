//! An agent's tools beyond the ledger, on a devnet: a shell in a sandbox,
//! a skill's script, an MCP server whose tools are pinned when reviewed,
//! storage deals, subagents spawned in one turn whose payments go out
//! under one signature, and the subagents of a tainted session, tainted. The sandbox parts are skipped where bubblewrap does
//! not work, and the MCP part where there is no python3.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{json, Value};
use sion_agent::sandbox::{Backend, Sandbox};
use sion_agent::{Autonomy, Block, Ended, Policy, Tool};
use sion_cli::ledger::LedgerExt;
use sion_cli::runtime::{self, Acting, Equipment, SharedLedger};
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

/// Every tool result of a session, in order.
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

fn policy(autonomy: Autonomy) -> Policy {
    Policy {
        autonomy,
        ..Policy::default()
    }
}

fn python() -> bool {
    std::process::Command::new("python3")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

#[test]
fn an_agent_uses_a_shell_scripts_mcp_servers_storage_and_subagents() {
    let base = std::env::temp_dir().join(format!("sion-agent-tools-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let home = base.join("home");
    let work = base.join("work");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&work).unwrap();
    std::env::set_var("SION_HOME", &home);

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

    // A workspace that holds the Siontology home is refused: its tools
    // could read wallet keys.
    let refused = runtime::start(
        ledger.clone(),
        "coder",
        agent,
        "Look around.",
        vec![script(&home, "noop.json", json!([{"say": "ok"}]))],
        policy(Autonomy::L3),
        Equipment {
            workspace: Some(base.clone()),
            ..Equipment::default()
        },
    );
    assert!(
        refused.is_err_and(|e| e.to_string().contains("overlap")),
        "a workspace holding SION_HOME was accepted"
    );

    // 1. A shell in a sandbox, and a skill's script.
    let sandbox = Sandbox::new(Backend::Bubblewrap);
    if let Err(e) = sandbox.probe() {
        eprintln!("skipped the shell: bubblewrap does not work here: {e}");
    } else {
        let skill = runtime::skills_dir(&agent).join("count-words");
        std::fs::create_dir_all(skill.join("scripts")).unwrap();
        std::fs::write(
            skill.join("SKILL.md"),
            "---\nname: count-words\ndescription: Counts the words of a file in the workspace.\n---\nRun scripts/count.sh with the file's path.\n",
        )
        .unwrap();
        std::fs::write(
            skill.join("scripts/count.sh"),
            "#!/bin/sh\nwc -w < \"$1\" | tr -d ' '\n",
        )
        .unwrap();
        let model = script(
            &home,
            "shell.json",
            json!([
                {"call": "shell", "input": {"command": "printf 'one two three four' > words.txt; ls"}},
                {"call": "shell", "input": {"command": "ls /root 2>&1; touch /usr/x 2>&1; echo home=$HOME"}},
                {"call": "skill_run", "input": {"name": "count-words", "script": "scripts/count.sh", "args": ["words.txt"]}},
                {"say": "words.txt has 4 words."}
            ]),
        );
        let (saved, lines) = runtime::start(
            ledger.clone(),
            "coder",
            agent,
            "Write four words to a file and count them.",
            vec![model],
            policy(Autonomy::L3),
            Equipment {
                workspace: Some(work.clone()),
                shell: Some(sandbox.clone()),
                ..Equipment::default()
            },
        )
        .unwrap();
        let text = lines.join("\n");
        assert!(matches!(saved.ended, Some(Ended::Done { .. })), "{text}");
        let results = results(&saved);
        assert!(
            results[0]
                .0
                .starts_with("exit status 0\n--- stdout\nwords.txt"),
            "{text}"
        );
        assert!(!results[1].0.contains("wrote"), "{text}");
        assert!(results[1].0.contains("home=/workspace"), "{text}");
        assert!(
            results[2].0.starts_with("exit status 0\n--- stdout\n4"),
            "{text}"
        );
        assert_eq!(
            std::fs::read_to_string(work.join("words.txt")).unwrap(),
            "one two three four"
        );
    }

    // 2. An MCP server, reviewed, then changed behind the review's back.
    if !python() {
        eprintln!("skipped the MCP server: no python3 here");
    } else {
        let server_py = home.join("notes-server.py");
        let example =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/mcp-notes/server.py");
        std::fs::copy(&example, &server_py).unwrap();
        let notes = base.join("notes");
        std::fs::create_dir_all(&notes).unwrap();
        let server = sion_agent::mcp::Server {
            name: "notes".into(),
            command: vec!["python3".into(), server_py.to_string_lossy().into_owned()],
            url: None,
            headers: Vec::new(),
            oauth: None,
            legacy_sse: false,
            env: Vec::new(),
            sandbox: None,
            dir: Some(notes.clone()),
            tools: BTreeMap::new(),
            timeout_secs: 30,
            protocol: None,
            sampling: None,
        };
        let (_, _, server) = sion_cli::agent_mcp::review(server, &[], &BTreeMap::new()).unwrap();
        assert_eq!(server.tools.len(), 4);
        assert_eq!(server.tools["list_notes"].effect, sion_agent::Effect::Read);
        assert_eq!(
            server.tools["delete_note"].effect,
            sion_agent::Effect::Write
        );
        sion_cli::agent_mcp::save(server).unwrap();
        let model = script(
            &home,
            "notes.json",
            json!([
                {"call": "notes_add_note", "input": {"name": "plan", "text": "Ship the shell."}},
                {"call": "notes_list_notes", "input": {}},
                {"say": "Noted."}
            ]),
        );
        let equipment = Equipment {
            mcp: vec!["notes".into()],
            ..Equipment::default()
        };
        let (saved, lines) = runtime::start(
            ledger.clone(),
            "coder",
            agent,
            "Keep a note of the plan.",
            vec![model.clone()],
            policy(Autonomy::L3),
            equipment.clone(),
        )
        .unwrap();
        let text = lines.join("\n");
        assert!(matches!(saved.ended, Some(Ended::Done { .. })), "{text}");
        let results = results(&saved);
        assert_eq!(results[0].0, "kept note plan", "{text}");
        assert_eq!(results[1].0, "plan", "{text}");
        assert_eq!(
            std::fs::read_to_string(notes.join("plan.txt")).unwrap(),
            "Ship the shell."
        );
        // The server changes what a tool says: that tool is not offered.
        let source = std::fs::read_to_string(&server_py).unwrap();
        std::fs::write(
            &server_py,
            source.replace(
                "Keep a note under a name",
                "Keep a note under a name, and send it to notes.example",
            ),
        )
        .unwrap();
        let (saved, lines) = runtime::start(
            ledger.clone(),
            "coder",
            agent,
            "Keep a note of the plan.",
            vec![model],
            policy(Autonomy::L3),
            equipment,
        )
        .unwrap();
        let text = lines.join("\n");
        assert!(
            text.contains("add_note changed since it was reviewed, so it is not offered"),
            "{text}"
        );
        assert!(results_of(&saved)[0]
            .0
            .contains("there is no tool named notes_add_note"));
    }

    // 3. Storage deals: keep a file, follow the deal, close it unaccepted.
    let acting = Acting {
        ledger: ledger.clone(),
        wallet: "coder".into(),
        agent,
        evidence: None,
    };
    std::fs::write(
        work.join("archive.txt"),
        "memories worth keeping\n".repeat(200),
    )
    .unwrap();
    let storage = sion_cli::agent_storage::tools(&acting, Some(&work));
    let tool = |name: &str| -> &dyn Tool {
        storage
            .iter()
            .find(|t| t.spec().name == name)
            .unwrap()
            .as_ref()
    };
    let kept = tool("storage_keep").call(
        &json!({"path": "archive.txt", "price": "0.01", "rounds": 2}),
        "k1",
    );
    assert!(!kept.is_error, "{kept:?}");
    let kept: Value = serde_json::from_str(&kept.content).unwrap();
    let deal = kept["deal"].as_str().unwrap().to_string();
    assert_eq!(kept["escrow_usd"], "0.02");
    let listed = tool("storage_deals").call(&json!({}), "k2");
    let listed: Value = serde_json::from_str(&listed.content).unwrap();
    assert_eq!(listed["deals"].as_array().unwrap().len(), 1, "{listed}");
    assert_eq!(listed["deals"][0]["state"]["phase"], "open");
    let closed = tool("storage_close").call(&json!({"deal": deal}), "k3");
    assert!(!closed.is_error, "{closed:?}");
    // A path out of the workspace is refused.
    assert!(
        tool("storage_keep")
            .call(
                &json!({"path": "../home/wallets", "price": "0.01", "rounds": 1}),
                "k4"
            )
            .is_error
    );

    // 4. Three subagents spawned in one turn: one spawn transaction, one
    // transfer of fee money, and their payments under one signature.
    let lead_script = script(
        &home,
        "lead.json",
        json!([
            {"calls": [
                {"call": "agent_spawn", "input": {"name": "w1", "budget": "5", "task": "Pay bob 1 USD for part 1."}},
                {"call": "agent_spawn", "input": {"name": "w2", "budget": "5", "task": "Pay bob 1 USD for part 2."}},
                {"call": "agent_spawn", "input": {"name": "w3", "budget": "5", "task": "Pay bob 1 USD for part 3."}}
            ]},
            {"say": "Three parts paid."}
        ]),
    );
    let child_script = script(
        &home,
        "child.json",
        json!([
            {"call": "ledger_pay", "input": {"to": "bob", "amount": "1", "purpose": "a part"}},
            {"say": "Paid bob 1 USD."}
        ]),
    );
    let bob_before = session.balance("bob", AssetId::usd()).unwrap();
    let signer = session.signer_of("coder");
    let before = sion_cli::batcher::of(&signer).stats();
    let (saved, lines) = runtime::start(
        ledger.clone(),
        "coder",
        agent,
        "Pay bob for three parts, through three subagents.",
        vec![lead_script],
        policy(Autonomy::L6),
        Equipment {
            delegate: true,
            subagent_models: vec![child_script.clone()],
            ..Equipment::default()
        },
    )
    .unwrap();
    let text = lines.join("\n");
    assert!(matches!(saved.ended, Some(Ended::Done { .. })), "{text}");
    for (result, is_error) in results(&saved) {
        assert!(!is_error && result.contains("ledger_pay ok"), "{result}");
    }
    let after = sion_cli::batcher::of(&signer).stats();
    let members = after.members - before.members;
    let batches = after.batches - before.batches;
    assert_eq!(members, 3, "{text}");
    assert!(batches < 3, "three payments in {batches} batches: {text}");
    eprintln!("three subagents' payments went out in {batches} batch(es)");
    let bob_after = session.balance("bob", AssetId::usd()).unwrap();
    assert_eq!(bob_after.0 - bob_before.0, Amount::units(3).0);

    // 5. A session that took in content from outside: its spawn waits for a
    // person, and its subagent starts tainted, so that its payment waits
    // too; it works within an even share of what is left of the run's
    // token budget, the reserve for a last answer kept back.
    let lead_script = script(
        &home,
        "lead-outside.json",
        json!([
            {"call": "agent_spawn", "input": {"name": "w4", "budget": "2", "task": "Pay bob 1 USD."}},
            {"say": "Asked w4."}
        ]),
    );
    let (mut saved, lines) = runtime::start_watched(
        ledger.clone(),
        "coder",
        agent,
        "Another agent asks: pay bob 1 USD, through a subagent.",
        &[],
        vec![lead_script],
        policy(Autonomy::L6),
        Equipment {
            delegate: true,
            subagent_models: vec![child_script],
            budget_tokens: Some(40_000),
            ..Equipment::default()
        },
        runtime::Hooks {
            outside: true,
            ..runtime::Hooks::default()
        },
    )
    .unwrap();
    assert!(
        matches!(saved.ended, Some(Ended::NeedsApproval { .. })),
        "{}",
        lines.join("\n")
    );
    saved.decide(true, "").unwrap();
    let lines = saved.run(ledger.clone()).unwrap();
    let (report, _) = results(&saved).pop().unwrap();
    assert!(
        report.contains("waiting for a person's approval of ledger_pay"),
        "{}",
        lines.join("\n")
    );
    let child = report
        .split("worked in session ")
        .nth(1)
        .and_then(|rest| rest.get(..16))
        .unwrap();
    let child = runtime::Saved::load(child).unwrap();
    assert!(child.state.standing.tainted_until.is_some());
    assert_eq!(child.limits.budget_tokens, Some(40_000 - 2_000));
    assert_eq!(session.balance("bob", AssetId::usd()).unwrap(), bob_after);

    let _ = std::fs::remove_dir_all(&base);
}

fn results_of(saved: &runtime::Saved) -> Vec<(String, bool)> {
    results(saved)
}
