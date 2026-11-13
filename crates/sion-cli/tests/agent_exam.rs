//! Sealed exams, on a devnet, with scripted models: an author seals a set
//! of tasks and splits its key among trustees; agents are entered while
//! nobody but the author can read the tasks; the trustees open it; anyone
//! then runs an entry and says what it found. An entry made after the
//! exam opened does not count, a share that is not its trustee's opens
//! nothing, and with exams by two authors an agent's results are set side
//! by side.

use std::sync::{Arc, Mutex};

use serde_json::json;
use sion_agent::Autonomy;
use sion_cli::ledger::LedgerExt;
use sion_cli::notary::{self, Entry, ExamCommand, ExamFile, Outcome, ShareFile};
use sion_cli::profiles::{self, Profile};
use sion_cli::runtime::SharedLedger;
use sion_cli::session::Session;
use sion_core::hash::Hash32;
use sion_core::ids::{AssetId, ClaimId, ObjectId, Pulse};
use sion_core::Amount;
use sion_exec::charter::Caveats;
use sion_exec::execute::Event;
use sion_exec::TxKind;
use sion_node::{Devnet, DevnetConfig};

static ENVIRONMENT: Mutex<()> = Mutex::new(());

fn home(name: &str) -> std::path::PathBuf {
    let home = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    std::env::set_var("SION_HOME", &home);
    home
}

fn script(home: &std::path::Path, name: &str, steps: serde_json::Value) -> String {
    let path = home.join(format!("{name}.json"));
    std::fs::write(&path, steps.to_string()).unwrap();
    format!("script:{}", path.display())
}

fn devnet() -> (Arc<Devnet>, SharedLedger) {
    let devnet = Arc::new(
        Devnet::start(DevnetConfig {
            rounds_per_pulse: 2,
            prove_chunk: None,
            // What these tests read back is in the node's events.
            history: 100_000,
            ..DevnetConfig::default()
        })
        .unwrap(),
    );
    let ledger: SharedLedger = devnet.clone();
    (devnet, ledger)
}

/// An agent of `owner`'s that may pay at most 10 USD at a time.
fn register(
    session: &mut Session<'_>,
    ledger: &SharedLedger,
    owner: &str,
    wallet: &str,
) -> ObjectId {
    let pulse = ledger.as_ref().pulse().unwrap();
    let account = session.id(wallet);
    let view = session
        .must(
            owner,
            TxKind::RegisterAgent {
                agent: account,
                card: Hash32::ZERO,
                caveats: Caveats {
                    payees: None,
                    max_per_payment: Amount::units(10),
                    burst: Amount::units(200),
                    rate: Amount(500_000),
                    expires_at: Pulse(pulse.0 + 100_000),
                    evidence_above: None,
                    evidence_policy: sion_assurance::Policy::standard(),
                    receipts: true,
                    may_delegate: false,
                },
                funding: Amount::units(300),
                agent_bond: Amount::ZERO,
            },
            Vec::new(),
        )
        .unwrap();
    session
        .fund(wallet, AssetId::usd(), Amount::units(1))
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

fn profile(name: &str, agent: ObjectId, models: Vec<String>, expect: bool) -> Profile {
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
        instructions: Some("You keep the books.".into()),
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
        expect,
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

/// What the careful agent has been told and the hasty one has not.
const LESSON: &str = "get_total answers in cents: divide by 100 for USD.";

/// Twelve tasks, six of which ask for a total in USD of a tool that
/// answers in cents, as `ask` words the question for invoice `n`.
fn books(name: &str, ask: impl Fn(u32) -> String) -> Vec<u8> {
    let total = json!({
        "name": "get_total",
        "description": "The total of an invoice.",
        "otherwise": "1250 (cents)"
    });
    let count = json!({
        "name": "count_open",
        "description": "How many invoices are open.",
        "otherwise": "3"
    });
    let mut cases = Vec::new();
    for n in 7..13 {
        cases.push(json!({
            "id": format!("total-{n}"),
            "task": ask(n),
            "tools": [total],
            "checks": [{"says": "12.50"}, "done"]
        }));
    }
    for (i, who) in ["Acme", "Birch", "Cedar", "Dune", "Elm", "Fjord"]
        .iter()
        .enumerate()
    {
        cases.push(json!({
            "id": format!("open-{i}"),
            "task": format!("How many invoices of {who} are open?"),
            "tools": [count],
            "checks": [{"says": "3"}, "done"]
        }));
    }
    json!({"name": name, "cases": cases})
        .to_string()
        .into_bytes()
}

fn exam(path: &std::path::Path) -> ExamFile {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn share(path: &std::path::Path) -> ShareFile {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn an_exam_is_sealed_before_agents_enter_opened_by_its_trustees_and_run_by_anyone() {
    let _one = ENVIRONMENT.lock().unwrap_or_else(|e| e.into_inner());
    let home = home("agent-exam");
    let out = std::env::temp_dir().join(format!("agent-exam-out-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(&out).unwrap();
    let (_devnet, ledger) = devnet();
    let mut session = Session::open(ledger.as_ref()).unwrap();
    for who in [
        "alice", "bob", "carol", "dave", "erin", "frank", "grace", "heidi", "mallory",
    ] {
        session
            .fund(who, AssetId::usd(), Amount::units(1_000))
            .unwrap();
    }
    let careful_agent = register(&mut session, &ledger, "erin", "careful");
    let hasty_agent = register(&mut session, &ledger, "frank", "hasty");

    // Both agents run on the same model. It takes the tool's answer for
    // USD unless it has been told otherwise.
    let keeper = script(
        &home,
        "keeper",
        json!([
            {"when": "how many invoices", "call": "count_open", "input": {}},
            {"when": "how many invoices", "say": "3 invoices are open."},
            {"when": "get_total answers in cents", "call": "get_total", "input": {"invoice": "7"}},
            {"when": "get_total answers in cents", "say": "The total is 12.50 USD."},
            {"call": "get_total", "input": {"invoice": "7"}},
            {"say": "The total is 1250 USD."}
        ]),
    );
    let careful = Profile {
        instructions: Some(format!("You keep the books. {LESSON}")),
        ..profile("careful", careful_agent, vec![keeper.clone()], false)
    };
    let hasty = profile("hasty", hasty_agent, vec![keeper.clone()], false);
    profiles::save_all(
        &[
            ("careful".to_string(), careful.clone()),
            ("hasty".to_string(), hasty.clone()),
        ]
        .into(),
    )
    .unwrap();

    // Alice seals an exam: three trustees, any two of whom open it.
    let tasks = books("books", |n| {
        format!("What is the total of invoice {n}, in USD?")
    });
    let tasks_file = out.join("tasks.json");
    std::fs::write(&tasks_file, &tasks).unwrap();
    notary::command(
        &mut session,
        ledger.as_ref(),
        ExamCommand::Seal {
            name: "books".into(),
            tasks: tasks_file.clone(),
            about: "Keeping   the books.".into(),
            trustees: vec!["bob".into(), "carol".into(), "dave".into()],
            threshold: 2,
            runs: 2,
            budget_tokens: 20_000,
            wallet: "alice".into(),
            out: out.clone(),
        },
    )
    .unwrap();
    let exam_file = out.join("books.exam.json");
    let file = exam(&exam_file);
    // The file anyone may hold says what the exam is and nothing of its
    // tasks.
    assert_eq!(file.exam.cases, 12);
    assert_eq!(file.exam.about, "Keeping the books.");
    let text = std::fs::read_to_string(&exam_file).unwrap();
    assert!(!text.contains("get_total") && !text.contains("total of invoice"));
    let (claim, author, _) = notary::on_ledger(ledger.as_ref(), &file).unwrap();
    assert_eq!(author, session.id("alice"));
    let shares: Vec<ShareFile> = (1..=3)
        .map(|x| share(&out.join(format!("books.share-{x}.json"))))
        .collect();

    // A file that is not what the claim holds is not the exam.
    let mut forged = file.clone();
    forged.exam.cases = 3;
    assert!(notary::on_ledger(ledger.as_ref(), &forged)
        .unwrap_err()
        .to_string()
        .contains("not the exam its claim holds"));
    let mut swapped = file.clone();
    swapped.sealed = hex::encode(b"other tasks");
    assert!(notary::on_ledger(ledger.as_ref(), &swapped)
        .unwrap_err()
        .to_string()
        .contains("sealed tasks"));
    // An exam of too many trustees' shares for its trustees is refused.
    assert!(notary::seal("books", "", &tasks, &[session.id("bob")], 2, 1, 1_000).is_err());
    assert!(notary::seal("Books!", "", &tasks, &[session.id("bob")], 1, 1, 1_000).is_err());

    // The agents are entered while the exam is sealed.
    for (profile, owner) in [("careful", "erin"), ("hasty", "frank")] {
        notary::command(
            &mut session,
            ledger.as_ref(),
            ExamCommand::Enter {
                exam: exam_file.clone(),
                profile: profile.into(),
                wallet: owner.into(),
            },
        )
        .unwrap();
    }
    let all = notary::found(ledger.as_ref(), claim, &file.exam).unwrap();
    assert_eq!(all.entries.len(), 2);
    let entry_of = |all: &notary::Found, name: &str| -> (ClaimId, Entry) {
        all.entries
            .iter()
            .find(|(_, _, _, entry)| entry.name == name)
            .map(|(claim, _, _, entry)| (*claim, entry.clone()))
            .unwrap()
    };
    let (careful_entry, careful_says) = entry_of(&all, "careful");
    let (hasty_entry, hasty_says) = entry_of(&all, "hasty");
    assert!(careful_says.instructions.contains(LESSON));
    assert_eq!(careful_says.models, vec![keeper.clone()]);
    assert_ne!(careful_says.agent(), hasty_says.agent());

    // Sealed, it cannot be run or read: not by one who holds no share, not
    // with one trustee's share.
    let sealed = notary::command(
        &mut session,
        ledger.as_ref(),
        ExamCommand::Run {
            exam: exam_file.clone(),
            entry: careful_entry.as_hash().to_hex(),
            wallet: None,
        },
    )
    .unwrap_err()
    .to_string();
    assert!(sealed.contains("is sealed: 0 of the 2 shares"), "{sealed}");
    // A share released by someone who is not its trustee opens nothing.
    assert!(
        notary::release(&mut session, "mallory", claim, &file.exam, &shares[0].share)
            .unwrap_err()
            .to_string()
            .contains("not a trustee")
    );
    assert!(
        notary::release(&mut session, "carol", claim, &file.exam, &shares[0].share)
            .unwrap_err()
            .to_string()
            .contains("not the share")
    );
    let release = |session: &mut Session<'_>, trustee: &str, x: usize| {
        notary::command(
            session,
            ledger.as_ref(),
            ExamCommand::Release {
                exam: exam_file.clone(),
                share: out.join(format!("books.share-{x}.json")),
                wallet: trustee.into(),
            },
        )
        .unwrap();
    };
    release(&mut session, "bob", 1);
    // The same trustee's share twice is one share.
    release(&mut session, "bob", 1);
    let all = notary::found(ledger.as_ref(), claim, &file.exam).unwrap();
    assert_eq!(all.shares.len(), 1);
    assert_eq!(all.opened_at(file.exam.threshold), None);
    assert!(notary::opened(&file, &all).unwrap().is_none());

    // The second trustee's share opens it.
    release(&mut session, "carol", 2);
    let all = notary::found(ledger.as_ref(), claim, &file.exam).unwrap();
    let opened_at = all.opened_at(file.exam.threshold).unwrap();
    let (set, read) = notary::opened(&file, &all).unwrap().unwrap();
    assert_eq!(read, tasks);
    assert_eq!(set.cases.len(), 12);
    assert!(all.entries.iter().all(|(_, _, at, _)| *at < opened_at));
    let read_file = out.join("opened.json");
    notary::command(
        &mut session,
        ledger.as_ref(),
        ExamCommand::Tasks {
            exam: exam_file.clone(),
            out: read_file.clone(),
        },
    )
    .unwrap();
    assert_eq!(std::fs::read(&read_file).unwrap(), tasks);

    // An agent entered now has seen the tasks: the command refuses, and an
    // entry made anyway does not count.
    let late = notary::command(
        &mut session,
        ledger.as_ref(),
        ExamCommand::Enter {
            exam: exam_file.clone(),
            profile: "careful".into(),
            wallet: "mallory".into(),
        },
    )
    .unwrap_err()
    .to_string();
    assert!(late.contains("an entry made now does not count"), "{late}");
    let fitted = Entry {
        exam: file.claim.clone(),
        name: "fitted".into(),
        models: vec![keeper.clone()],
        instructions: "Every total is 12.50 USD.".into(),
        learned: String::new(),
        expect: false,
    };
    let fitted_entry = notary::enter(&mut session, "mallory", &fitted).unwrap();
    let refused = notary::command(
        &mut session,
        ledger.as_ref(),
        ExamCommand::Run {
            exam: exam_file.clone(),
            entry: fitted_entry.as_hash().to_hex(),
            wallet: Some("mallory".into()),
        },
    )
    .unwrap_err()
    .to_string();
    assert!(refused.contains("after the exam opened"), "{refused}");

    // Grace, who entered nothing, runs both entries; erin runs her own.
    for (runner, entry) in [
        ("grace", careful_entry),
        ("grace", hasty_entry),
        ("erin", careful_entry),
    ] {
        notary::command(
            &mut session,
            ledger.as_ref(),
            ExamCommand::Run {
                exam: exam_file.clone(),
                // A prefix of the entry's id names it.
                entry: entry.as_hash().to_hex()[..12].to_string(),
                wallet: Some(runner.into()),
            },
        )
        .unwrap();
    }
    // A result for an entry that is not this exam's is no result of it.
    let stray = Outcome {
        exam: file.claim.clone(),
        entry: Hash32::ZERO.to_hex(),
        passed: 24,
        total: 24,
        tasks: vec![true; 12],
        sion: "0".into(),
    };
    notary::report(&mut session, "mallory", &stray).unwrap();

    let all = notary::found(ledger.as_ref(), claim, &file.exam).unwrap();
    assert_eq!(all.results.len(), 3, "{:?}", all.results);
    let found_by = |runner: &str, entry: ClaimId, session: &mut Session<'_>| -> Outcome {
        let runner = session.id(runner);
        all.results
            .iter()
            .find(|(_, by, o)| *by == runner && o.entry == entry.as_hash().to_hex())
            .map(|(_, _, o)| o.clone())
            .unwrap()
    };
    let careful_found = found_by("grace", careful_entry, &mut session);
    assert_eq!((careful_found.passed, careful_found.total), (24, 24));
    assert!(careful_found.tasks.iter().all(|passed| *passed));
    let hasty_found = found_by("grace", hasty_entry, &mut session);
    assert_eq!((hasty_found.passed, hasty_found.total), (12, 24));
    assert_eq!(hasty_found.tasks.iter().filter(|p| **p).count(), 6);
    assert_eq!(
        notary::by_others(&file.exam, &all, &careful_says.agent()),
        Some((24, 24))
    );
    assert_eq!(notary::by_others(&file.exam, &all, &fitted.agent()), None);

    let grace = session.id("grace").short();
    let erin = session.id("erin").short();
    let lines = notary::lines(&file.exam, &author, &all).join("\n");
    assert!(lines.contains(&format!("opened at Pulse {opened_at}: 2 of its 3 trustees")));
    assert!(
        lines.contains(&format!("    run by others: 24/24 by {grace}")),
        "{lines}"
    );
    assert!(
        lines.contains(&format!("    run by its entrant: 24/24 by {erin}")),
        "{lines}"
    );
    assert!(
        lines.contains(&format!("    run by others: 12/24 by {grace}")),
        "{lines}"
    );
    assert!(
        lines.contains("after the exam opened: it does not count"),
        "{lines}"
    );

    // A second exam, by another author who has not read alice's tasks.
    // Only the careful agent is entered in it.
    let other = books("ledgers", |n| {
        format!("Invoice {n}: what is its total in USD?")
    });
    let other_tasks = out.join("ledgers-tasks.json");
    std::fs::write(&other_tasks, &other).unwrap();
    notary::command(
        &mut session,
        ledger.as_ref(),
        ExamCommand::Seal {
            name: "ledgers".into(),
            tasks: other_tasks,
            about: "More books.".into(),
            trustees: vec!["dave".into()],
            threshold: 1,
            runs: 1,
            budget_tokens: 20_000,
            wallet: "heidi".into(),
            out: out.clone(),
        },
    )
    .unwrap();
    let other_file_path = out.join("ledgers.exam.json");
    notary::command(
        &mut session,
        ledger.as_ref(),
        ExamCommand::Enter {
            exam: other_file_path.clone(),
            profile: "careful".into(),
            wallet: "erin".into(),
        },
    )
    .unwrap();
    notary::command(
        &mut session,
        ledger.as_ref(),
        ExamCommand::Release {
            exam: other_file_path.clone(),
            share: out.join("ledgers.share-1.json"),
            wallet: "dave".into(),
        },
    )
    .unwrap();
    let other_file = exam(&other_file_path);
    let (other_claim, _, _) = notary::on_ledger(ledger.as_ref(), &other_file).unwrap();
    let other_all = notary::found(ledger.as_ref(), other_claim, &other_file.exam).unwrap();
    // A share of one exam is no share of another, and neither is an entry.
    assert_eq!(other_all.shares.len(), 1);
    assert_eq!(other_all.entries.len(), 1);
    let entered = other_all.entries[0].0;
    notary::command(
        &mut session,
        ledger.as_ref(),
        ExamCommand::Run {
            exam: other_file_path.clone(),
            entry: entered.as_hash().to_hex(),
            wallet: Some("grace".into()),
        },
    )
    .unwrap();
    notary::command(
        &mut session,
        ledger.as_ref(),
        ExamCommand::Results {
            exams: vec![exam_file.clone(), other_file_path.clone()],
        },
    )
    .unwrap();
    let other_all = notary::found(ledger.as_ref(), other_claim, &other_file.exam).unwrap();
    let side_by_side = notary::across(&[
        (file.exam.clone(), all),
        (other_file.exam.clone(), other_all),
    ])
    .join("\n");
    assert!(
        side_by_side.contains("(careful), as others ran it: books: 24/24; ledgers: 12/12"),
        "{side_by_side}"
    );
    assert!(
        side_by_side.contains("(hasty), as others ran it: books: 12/24; ledgers: not entered"),
        "{side_by_side}"
    );
    assert!(
        side_by_side.contains(
            "(fitted), as others ran it: books: entered after it opened; ledgers: not entered"
        ),
        "{side_by_side}"
    );
    let _ = (Pulse(0), ObjectId::from_hash(Hash32::ZERO));
}
