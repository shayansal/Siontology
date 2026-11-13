//! A dataset of checked work on a devnet, with scripted models: a curator
//! opens it with its terms; an owner contributes sessions of its agent,
//! one checked by a Task Charter the ledger settled and one by its own
//! check, with a name taken out; the curator builds a version and someone
//! about to train on it checks it against the ledger. A session nothing
//! checked, a copy contributed by someone else, a changed example and a
//! withdrawn one are left out.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use serde_json::json;
use sion_agent::Autonomy;
use sion_assurance::{AtomClass, Policy};
use sion_cli::dataset::{self, Asked, Card, DatasetCommand, Evidence, ExampleFile, Kind, Manifest};
use sion_cli::ledger::LedgerExt;
use sion_cli::profiles::Profile;
use sion_cli::runtime::SharedLedger;
use sion_cli::session::Session;
use sion_core::hash::Hash32;
use sion_core::ids::{AssetId, ObjectId, Pulse};
use sion_core::Amount;
use sion_exec::charter::Caveats;
use sion_exec::claim::ClaimBody;
use sion_exec::execute::{bond_atom, Event};
use sion_exec::task::{result_formula, Acceptance, TaskResult, RESULT_SCHEMA};
use sion_exec::{ExecStatus, TxKind};
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

const TERMS: &str = "You let anyone train models on what you contribute, and you say it is yours \
                     to give.";
const SUMMARY: &str = "The contract runs a year and renews unless either side gives notice.";

fn read<T: serde::de::DeserializeOwned>(path: &std::path::Path) -> T {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn checked_sessions_are_contributed_on_terms_and_built_into_a_version_a_trainer_checks() {
    let _one = ENVIRONMENT.lock().unwrap_or_else(|e| e.into_inner());
    let home = home("agent-dataset");
    let out = std::env::temp_dir().join(format!("agent-dataset-out-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);
    let (given, built) = (out.join("given"), out.join("built"));
    std::fs::create_dir_all(&given).unwrap();
    let (_devnet, ledger) = devnet();
    let mut session = Session::open(ledger.as_ref()).unwrap();
    for who in ["alice", "bob", "carol", "cora", "mallory"] {
        session
            .fund(who, AssetId::usd(), Amount::units(1_000))
            .unwrap();
    }
    let agent = register(&mut session, &ledger, "alice", "clerk");
    session
        .fund("clerk", AssetId::usd(), Amount::units(200))
        .unwrap();
    let (alice, clerk, mallory) = (
        session.id("alice"),
        session.id("clerk"),
        session.id("mallory"),
    );

    // Three sessions of alice's agent.
    let says = |name: &str, text: &str| script(&home, name, json!([{ "say": text }]));
    let run = |name: &str, text: &str, task: &str| {
        profile("clerk", agent, vec![says(name, text)], false)
            .start(ledger.clone(), task)
            .unwrap()
            .0
    };
    let summary = run("s1", SUMMARY, "Summarize contract 7 for Dana Ruiz.");
    let mut counted = run("s2", "3 invoices are open.", "How many invoices are open?");
    let idle = run("s3", "Nothing to add.", "Anything else?");
    // The second ran with a check, which passed: a session ends done only
    // when its check does.
    counted.verify = Some("grep -q 3 answer.txt".into());
    counted.save().unwrap();

    // The first was work for bob: a Task Charter, settled on the ledger
    // with the summary as its output.
    session
        .must(
            "carol",
            TxKind::PostBond {
                class: AtomClass::Adjudicator,
                label: "adjudicator".into(),
                amount: Amount::units(100),
            },
            Vec::new(),
        )
        .unwrap();
    let adjudicator = bond_atom(AtomClass::Adjudicator, &session.id("carol"), "adjudicator").id;
    session
        .must(
            "clerk",
            TxKind::PostBond {
                class: AtomClass::Economic,
                label: sion_exec::task::WORKER_BOND_LABEL.into(),
                amount: Amount::units(100),
            },
            Vec::new(),
        )
        .unwrap();
    let posted = session
        .must(
            "bob",
            TxKind::PostTask {
                spec: sion_cli::tasks::spec_commitment(b"Summarize contract 7."),
                acceptance: Acceptance {
                    policy: Policy::standard(),
                    requires: BTreeSet::new(),
                    verifiers: BTreeSet::new(),
                    must_attest: BTreeSet::new(),
                    min_attestations: 0,
                    adjudicator: Some(adjudicator),
                    detection_permille: 900,
                    reliance: 1_000,
                },
                worker: Some(clerk),
                amount: Amount::units(2),
                deadline: Pulse(ledger.as_ref().pulse().unwrap().0 + 2_000),
                charter: None,
            },
            Vec::new(),
        )
        .unwrap();
    let task = posted
        .effects
        .events
        .iter()
        .find_map(|e| match e {
            Event::TaskPosted { task, .. } => Some(*task),
            _ => None,
        })
        .unwrap();
    let result = ClaimBody {
        schema: RESULT_SCHEMA.into(),
        subject: "summary of contract 7".into(),
        payload: TaskResult {
            task,
            output: sion_cli::tasks::output_commitment(SUMMARY.as_bytes()),
        }
        .payload(),
        formula: result_formula(&clerk, &[], &BTreeSet::new()),
        adjudicator: Some(adjudicator),
        detection_permille: 900,
        valid_for: 5_000,
        challenge_period: 2_000,
    };
    session
        .must(
            "clerk",
            TxKind::IssueClaim {
                claim: result.clone(),
            },
            Vec::new(),
        )
        .unwrap();
    let settled = session
        .send(
            "clerk",
            TxKind::SettleTask {
                task,
                result,
                attestations: Vec::new(),
            },
            Vec::new(),
        )
        .unwrap();
    assert!(matches!(settled.effects.status, ExecStatus::Success));

    // Cora opens a dataset: work checked by a settled task or a check.
    let card = Card {
        name: "bookkeeping".into(),
        about: "Bookkeeping work that was checked.".into(),
        terms: TERMS.into(),
        takes: vec![Kind::Task, Kind::Check],
        outside: false,
    };
    let claim = dataset::open(&mut session, "cora", &card).unwrap();
    let hex = claim.as_hash().to_hex();
    let (_, read_card, curator) = dataset::card(ledger.as_ref(), &hex).unwrap();
    assert_eq!((&read_card, curator), (&card, session.id("cora")));

    // Without agreeing, nothing is contributed.
    let contribute = |session: &mut Session<'_>,
                      id: &str,
                      agree: bool,
                      task: Option<ObjectId>,
                      redact: &[&str]| {
        dataset::command(
            session,
            ledger.as_ref(),
            DatasetCommand::Contribute {
                dataset: hex.clone(),
                session: id.to_string(),
                agree,
                task: task.map(|t| t.0.to_hex()),
                output: None,
                redact: redact.iter().map(|s| s.to_string()).collect(),
                withhold_system: false,
                out: given.clone(),
                wallet: "alice".into(),
            },
        )
    };
    contribute(&mut session, &summary.state.id, false, Some(task), &[]).unwrap();
    assert_eq!(std::fs::read_dir(&given).unwrap().count(), 0);

    // What the example of the first session is: the customer's name taken
    // out, checked by the task, whose output is in the session and which
    // paid alice's agent.
    let asked = Asked {
        redact: vec!["Dana Ruiz".into()],
        task: Some(task),
        ..Asked::default()
    };
    let made = dataset::make(ledger.as_ref(), &claim, &card, &summary, &alice, &asked).unwrap();
    assert!(!made.line.contains("Dana Ruiz") && made.line.contains(dataset::REDACTED));
    assert!(made.line.contains(SUMMARY));
    assert_eq!(made.contribution.redactions, 1);
    assert_eq!(
        made.contribution.evidence,
        Evidence::Task {
            task,
            result: match sion_cli::tasks::load_task(ledger.as_ref(), task)
                .unwrap()
                .state
            {
                sion_exec::task::TaskState::Settled { result, .. } => result,
                other => panic!("{other:?}"),
            },
            output_in_session: true,
            agent: Some(agent),
        }
    );
    assert_eq!(made.contribution.terms, card.terms_digest());
    assert_eq!(made.contribution.models, vec!["scripted".to_string()]);
    // Someone the task did not pay cannot give it as what checked their
    // session, and another output is not the one it was settled with.
    assert!(
        dataset::make(ledger.as_ref(), &claim, &card, &summary, &mallory, &asked)
            .unwrap_err()
            .to_string()
            .contains("neither yours nor that of an agent you registered")
    );
    assert!(
        dataset::make(ledger.as_ref(), &claim, &card, &counted, &alice, &asked)
            .unwrap_err()
            .to_string()
            .contains("settled with another output")
    );
    // A session nothing checked is not contributed; one only marked good
    // is not what this dataset takes.
    let nothing = dataset::make(
        ledger.as_ref(),
        &claim,
        &card,
        &idle,
        &alice,
        &Asked::default(),
    );
    assert!(nothing
        .unwrap_err()
        .to_string()
        .contains("nothing checked it"));
    sion_cli::sealed::mark(&idle.state.id, true, "fine", 1).unwrap();
    let marked = dataset::make(
        ledger.as_ref(),
        &claim,
        &card,
        &idle,
        &alice,
        &Asked::default(),
    );
    assert!(marked
        .unwrap_err()
        .to_string()
        .contains("checked only by its operator's mark"));

    // Alice contributes the two.
    contribute(
        &mut session,
        &summary.state.id,
        true,
        Some(task),
        &["Dana Ruiz"],
    )
    .unwrap();
    contribute(&mut session, &counted.state.id, true, None, &[]).unwrap();
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(&given)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .collect();
    files.sort();
    assert_eq!(files.len(), 2);
    let examples: Vec<ExampleFile> = files.iter().map(|path| read(path)).collect();
    let (of_task, of_check) = match examples[0].contribution.evidence.kind() {
        Kind::Task => (&examples[0], &examples[1]),
        _ => (&examples[1], &examples[0]),
    };
    assert_eq!(of_task.contributor, alice);
    assert!(matches!(
        of_check.contribution.evidence,
        Evidence::Check { .. }
    ));

    // Mallory has both files and contributes them as her own. And an
    // example is changed on its way to the curator.
    for (name, file) in [("copy-task", of_task), ("copy-check", of_check)] {
        let mut copy = file.clone();
        copy.contributor = mallory;
        copy.claim = dataset::contribute(&mut session, "mallory", &copy.contribution)
            .unwrap()
            .as_hash()
            .to_hex();
        std::fs::write(
            given.join(format!("{name}.example.json")),
            serde_json::to_string(&copy).unwrap(),
        )
        .unwrap();
    }
    let mut changed = of_check.clone();
    changed.line = changed.line.replace("3 invoices", "30 invoices");
    std::fs::write(
        given.join("changed.example.json"),
        serde_json::to_string(&changed).unwrap(),
    )
    .unwrap();

    // Cora builds version 1. Only she can.
    let build = |session: &mut Session<'_>, wallet: &str| {
        dataset::command(
            session,
            ledger.as_ref(),
            DatasetCommand::Build {
                dataset: hex.clone(),
                from: given.clone(),
                out: built.clone(),
                wallet: wallet.into(),
            },
        )
    };
    assert!(build(&mut session, "mallory").is_err());
    build(&mut session, "cora").unwrap();
    let manifest: Manifest = read(&built.join("bookkeeping-v1.manifest.json"));
    let text = std::fs::read_to_string(built.join("bookkeeping-v1.jsonl")).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!((manifest.version.number, manifest.version.count), (1, 2));
    assert_eq!(lines.len(), 2);
    assert_eq!(
        manifest.version.credit,
        [(alice.as_hash().to_hex(), 2)].into()
    );
    assert_eq!(
        manifest.version.by_kind,
        [("check".to_string(), 1), ("task".to_string(), 1)].into()
    );
    assert!(manifest.examples.iter().all(|l| l.contributor == alice));
    assert!(!text.contains("Dana Ruiz") && !text.contains("30 invoices"));
    // Why each of the others was left out.
    let all: Vec<ExampleFile> = std::fs::read_dir(&given)
        .unwrap()
        .flatten()
        .map(|e| read(&e.path()))
        .collect();
    let left = dataset::build(ledger.as_ref(), &claim, &card, 1, &all)
        .unwrap()
        .left
        .join("\n");
    assert!(
        left.contains("not the one its contribution commits to"),
        "{left}"
    );
    assert_eq!(
        left.matches("contributed it after someone else had")
            .count(),
        2,
        "{left}"
    );
    // Alone, mallory's copy of the task's example is refused for what it
    // is: the task did not pay her.
    let hers: Vec<ExampleFile> = all
        .iter()
        .filter(|f| f.contributor == mallory && f.contribution.evidence.kind() == Kind::Task)
        .cloned()
        .collect();
    let alone = dataset::build(ledger.as_ref(), &claim, &card, 1, &hers).unwrap();
    let (taken, left) = (alone.examples, alone.left);
    assert!(taken.is_empty());
    assert!(
        left[0].contains("paid an account that is neither its contributor's"),
        "{left:?}"
    );

    // Someone about to train on it checks it against the ledger.
    let (report, holds) = dataset::check(ledger.as_ref(), &manifest, &lines).unwrap();
    let report = report.join("\n");
    assert!(holds, "{report}");
    assert!(
        report.contains("holds: 2 example(s), each the one the version lists"),
        "{report}"
    );
    assert!(
        report.contains("holds: version 1 of dataset bookkeeping is its curator's"),
        "{report}"
    );
    assert!(
        report.contains("which agreed to the dataset's terms"),
        "{report}"
    );
    assert!(
        report.contains("holds: 1 checked by a task: each task is settled on the ledger"),
        "{report}"
    );
    assert!(
        report.contains("1 checked by the session's own check: their contributors' word"),
        "{report}"
    );
    assert!(report.contains(&format!(
        "credit: {} gave 2",
        &alice.as_hash().to_hex()[..8]
    )));
    // A line changed, one left out, the order changed, or the count of
    // who gave what: none of them checks.
    let edited = lines[0].replacen("assistant", "Assistant", 1);
    assert_ne!(edited, lines[0]);
    for wrong in [
        vec![edited.as_str(), lines[1]],
        vec![lines[0]],
        vec![lines[1], lines[0]],
    ] {
        assert!(
            !dataset::check(ledger.as_ref(), &manifest, &wrong)
                .unwrap()
                .1
        );
    }
    let mut miscredited = manifest.clone();
    miscredited.version.credit = [(mallory.as_hash().to_hex(), 2)].into();
    assert!(
        !dataset::check(ledger.as_ref(), &miscredited, &lines)
            .unwrap()
            .1
    );

    // Alice withdraws the one her own check stood behind. Mallory cannot
    // withdraw what she never contributed, and her copy does not take its
    // place. The version already built still checks, and says so.
    let example_of_check = files
        .iter()
        .find(|path| {
            read::<ExampleFile>(path).contribution.example == of_check.contribution.example
        })
        .unwrap()
        .clone();
    assert!(dataset::withdraw(&mut session, "bob", &hex, &of_check.contribution.example).is_err());
    dataset::command(
        &mut session,
        ledger.as_ref(),
        DatasetCommand::Withdraw {
            example: example_of_check,
            wallet: "alice".into(),
        },
    )
    .unwrap();
    let (report, holds) = dataset::check(ledger.as_ref(), &manifest, &lines).unwrap();
    assert!(holds);
    assert!(report
        .join("\n")
        .contains("note: 1 example(s) were withdrawn after this version was built"));
    build(&mut session, "cora").unwrap();
    let second: Manifest = read(&built.join("bookkeeping-v2.manifest.json"));
    assert_eq!((second.version.number, second.version.count), (2, 1));
    assert_eq!(second.examples[0].example, of_task.contribution.example);
    let (_, latest) = dataset::latest(ledger.as_ref(), &curator, &claim)
        .unwrap()
        .unwrap();
    assert_eq!(latest, second.version);
    let _ = (SharedLedger::clone, Hash32::ZERO, Arc::new(0), Autonomy::L6);
    let _: Option<(Profile, Caveats)> = None;
}
