//! Agents as colleagues, on a devnet, with scripted models: a project its
//! agent works on within a budget its payments are held to, reporting at
//! its check-ins; facts shared between owners, vouched for and revoked,
//! a revoked one and what rests on it leaving a reader's prompt; and the
//! best agent for a task found among those named.

use std::sync::{Arc, Mutex};

use serde_json::json;
use sion_agent::{Autonomy, Ended, Policy};
use sion_cli::ledger::LedgerExt;
use sion_cli::memory::{self, MemoryCommand};
use sion_cli::profiles::{self, Profile};
use sion_cli::projects::{self, ProjectCommand, ProjectLink};
use sion_cli::rentals::{self, EnclaveKeys, Listing, ModelAccount};
use sion_cli::reputation::{self, FindArgs, Need};
use sion_cli::runtime::{self, Equipment, Hooks, Reply, SharedLedger};
use sion_cli::session::Session;
use sion_cli::vouching::{self, Standing};
use sion_core::ids::{AssetId, ObjectId, Pulse};
use sion_core::Amount;
use sion_enclave::simulated::SimulatedPlatform;
use sion_exec::charter::Caveats;
use sion_exec::execute::Event;
use sion_exec::TxKind;
use sion_memory::atom::{Link, LinkKind, MemoryKind, Statement, Value};
use sion_memory::{AtomClass, Interval, LogStore, MemoryAtom};
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
            ..DevnetConfig::default()
        })
        .unwrap(),
    );
    let ledger: SharedLedger = devnet.clone();
    (devnet, ledger)
}

/// alice registers `wallet` as an agent: any payee, at most 10 USD a
/// payment, a budget of 100 USD; its account gets 1 USD for fees.
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

fn start(profile: &str, name: &str, budget: &str, returns: bool, deliver: &std::path::Path) {
    projects::run_local(ProjectCommand::Start {
        profile: profile.into(),
        name: name.into(),
        goal: "Get the launch page reviewed".into(),
        budget: budget.into(),
        lasts: "30d".into(),
        check_in: "1d".into(),
        milestones: vec!["Draft reviewed".into()],
        deliver: Some(format!("file:{}", deliver.display())),
        work_at_check_in: returns,
    })
    .unwrap();
}

#[test]
fn a_projects_agent_works_within_its_budget_and_reports_at_its_check_ins() {
    let _one = ENVIRONMENT.lock().unwrap_or_else(|e| e.into_inner());
    let home = home("agent-projects");
    let (_devnet, ledger) = devnet();
    let mut session = Session::open(ledger.as_ref()).unwrap();
    session
        .fund("alice", AssetId::usd(), Amount::units(1_000))
        .unwrap();
    let agent = register(&mut session, &ledger, "lead");
    let bob_before = session.balance("bob", AssetId::usd()).unwrap();

    // Each project's session plays its own part of the script: the launch
    // asks for two reviews, of which its budget holds one; the weekly
    // check looks at the page.
    let model = script(
        &home,
        "lead",
        json!([
            {"when": "a project, launch", "call": "project_note", "input": {"what": "Asked bob to review the draft"}},
            {"when": "a project, launch", "call": "ledger_pay", "input": {"to": "bob", "amount": "5", "purpose": "a review"}},
            {"when": "a project, launch", "call": "ledger_pay", "input": {"to": "bob", "amount": "5", "purpose": "a second review"}},
            {"when": "a project, launch", "call": "project_milestone", "input": {"what": "Draft reviewed"}},
            {"when": "a project, launch", "say": "Bob reviewed the draft; a second review would pass the budget."},
            {"when": "a project, weekly", "call": "project_note", "input": {"what": "Checked the launch page"}},
            {"say": "Checked the launch page."}
        ]),
    );
    let lead = profile("lead", agent, vec![model]);
    profiles::save_all(&[("lead".to_string(), lead.clone())].into()).unwrap();
    let reports = home.join("reports.jsonl");
    start("lead", "launch", "8", false, &reports);
    let launch = ProjectLink {
        profile: "lead".into(),
        name: "launch".into(),
    };

    // The agent works on it: one review is paid, the second is refused by
    // the project's budget before it is sent, and the milestone is marked.
    let lines = projects::work(ledger.clone(), &lead, &launch, Some("the review")).unwrap();
    let text = lines.join("\n");
    assert!(text.contains("has 3 USD left, not 5"), "{text}");
    let bob_after = session.balance("bob", AssetId::usd()).unwrap();
    assert_eq!(bob_after.0 - bob_before.0, Amount::units(5).0, "{text}");
    let project = projects::get(&launch).unwrap();
    assert_eq!(project.spent, Amount::units(5));
    assert!(project.milestones[0].reached.is_some());
    let log: Vec<&str> = project.log.iter().map(|e| e.what.as_str()).collect();
    assert!(log.contains(&"Asked bob to review the draft"), "{log:?}");
    assert!(
        log.iter()
            .any(|w| w.starts_with("paid 5 USD to bob: a review")),
        "{log:?}"
    );
    assert!(log.iter().any(|w| w.starts_with("session ")), "{log:?}");

    // Its report says what was done since the last one, and is sent; the
    // next says plainly that nothing was.
    let now = projects::now();
    let first = projects::report(&launch, now).unwrap().join("\n");
    assert!(first.contains("Asked bob to review the draft"), "{first}");
    assert!(first.contains("5 of 8 USD spent"), "{first}");
    assert!(first.contains("(sent to file:"), "{first}");
    let second = projects::report(&launch, now + 1).unwrap().join("\n");
    assert!(second.contains("nothing since the last report"), "{second}");

    // At the check-ins a day later: a project that returns is worked on
    // first, and both are reported.
    start("lead", "weekly", "1", true, &reports);
    let checked = projects::check_ins(ledger.clone(), now + 2 * 86_400)
        .unwrap()
        .join("\n");
    assert!(checked.contains("Checked the launch page"), "{checked}");
    let sent = std::fs::read_to_string(&reports).unwrap();
    assert_eq!(sent.lines().count(), 4, "{sent}");
    assert!(sent.contains("Checked the launch page"), "{sent}");

    // Paused, it takes no work: neither a session nor a payment.
    projects::run_local(ProjectCommand::Pause {
        profile: "lead".into(),
        name: "launch".into(),
    })
    .unwrap();
    let refused = projects::work(ledger.clone(), &lead, &launch, None).unwrap_err();
    assert!(
        refused.to_string().contains("takes no more work"),
        "{refused}"
    );
    assert!(projects::hold(
        &launch,
        projects::now(),
        Amount::units(1),
        "anything",
        "call-x"
    )
    .is_err());
    projects::run_local(ProjectCommand::Resume {
        profile: "lead".into(),
        name: "launch".into(),
    })
    .unwrap();
    assert!(projects::get(&launch)
        .unwrap()
        .may_work(projects::now())
        .is_ok());
    let _ = std::fs::remove_dir_all(&home);
}

/// A preference of `writer`'s, the `seq`th atom of its log, resting on
/// `rests_on`.
fn preference(
    writer: sion_core::ids::AccountId,
    seq: u64,
    subject: &str,
    predicate: &str,
    text: &str,
    rests_on: Option<sion_memory::MemoryAtomId>,
) -> MemoryAtom {
    let mut atom = MemoryAtom::builder(
        writer,
        seq,
        MemoryKind::Preference,
        AtomClass::Assertion,
        Statement::claim(subject, predicate, Value::Text(text.into())),
    )
    .valid(Interval::starting(1_000))
    .observed_at(1_000)
    .build()
    .unwrap();
    if let Some(target) = rests_on {
        atom.links = vec![Link {
            kind: LinkKind::DerivedFrom,
            target,
            strength: 1_000,
        }];
    }
    atom
}

#[test]
fn a_revoked_shared_fact_leaves_the_readers_prompt_with_what_rests_on_it() {
    let _one = ENVIRONMENT.lock().unwrap_or_else(|e| e.into_inner());
    let home = home("shared-memory");
    let (_devnet, ledger) = devnet();
    let mut session = Session::open(ledger.as_ref()).unwrap();
    session
        .fund("alice", AssetId::usd(), Amount::units(1_000))
        .unwrap();
    for wallet in ["curator", "eve"] {
        session
            .fund(wallet, AssetId::usd(), Amount::units(1))
            .unwrap();
    }
    session
        .fund("dana", AssetId::usd(), Amount::units(20))
        .unwrap();

    // The curator keeps a log: metric units, a fact drawn from it, and one
    // of its own.
    let (_, log) = memory::open(&mut session, "curator", None).unwrap();
    let log = log.unwrap();
    let writer = session.id("curator");
    let metric = preference(writer, 0, "reports", "use", "metric units", None);
    let kilograms = preference(
        writer,
        1,
        "weights",
        "are given in",
        "kilograms",
        Some(metric.id()),
    );
    let on_time = preference(writer, 2, "meetings", "start", "on time", None);
    {
        let mut store = LogStore::open(memory::store_dir(&log)).unwrap();
        for atom in [&metric, &kilograms, &on_time] {
            store.append(atom.clone()).unwrap();
        }
    }
    memory::commit(&mut session, ledger.as_ref(), "curator", &log, false).unwrap();

    // A reader's agent keeps the log in mind.
    let reader = register(&mut session, &ledger, "reader");
    let model = script(
        &home,
        "reader",
        json!([
            {"say": "Noted."},
            {"unless": "metric units", "say": "The revoked fact is gone."},
            {"say": "The revoked fact is still here."}
        ]),
    );
    let (mut saved, _) = runtime::start(
        ledger.clone(),
        "reader",
        reader,
        "What do you keep in mind?",
        vec![model],
        Policy {
            autonomy: Autonomy::L3,
            ..Policy::default()
        },
        Equipment {
            shared_memory: vec![log],
            ..Equipment::default()
        },
    )
    .unwrap();
    let prompt = saved.system_prompt();
    for fact in ["metric units", "kilograms", "on time"] {
        assert!(prompt.contains(fact), "{prompt}");
    }

    // dana vouches for meetings starting on time, on a bond of her own,
    // under alice as adjudicator; a vouching under an adjudicator the
    // ledger does not know is refused.
    let log_hex = log.0.to_hex();
    let unknown = sion_exec::execute::bond_atom(
        sion_assurance::AtomClass::Adjudicator,
        &session.id("nobody"),
        "adjudicator",
    )
    .id;
    let refused = memory::run(
        &mut session,
        ledger.as_ref(),
        MemoryCommand::Vouch {
            wallet: "dana".into(),
            log: log_hex.clone(),
            seq: 2,
            adjudicator: unknown.0.to_hex(),
            bond: None,
            note: "they do".into(),
            valid_for: 100_000,
        },
    )
    .unwrap_err();
    assert!(refused.to_string().contains("did not issue"), "{refused}");
    session
        .must(
            "alice",
            TxKind::PostBond {
                class: sion_assurance::AtomClass::Adjudicator,
                label: "adjudicator".into(),
                amount: Amount::units(10),
            },
            Vec::new(),
        )
        .unwrap();
    let adjudicator = sion_exec::execute::bond_atom(
        sion_assurance::AtomClass::Adjudicator,
        &session.id("alice"),
        "adjudicator",
    )
    .id;
    memory::run(
        &mut session,
        ledger.as_ref(),
        MemoryCommand::Vouch {
            wallet: "dana".into(),
            log: log_hex.clone(),
            seq: 2,
            adjudicator: adjudicator.0.to_hex(),
            bond: Some("10".into()),
            note: "they do".into(),
            valid_for: 100_000,
        },
    )
    .unwrap();
    // eve, who neither wrote it nor vouched for it, is refused by the
    // command; a revocation she publishes anyway counts for nothing.
    let refused = memory::run(
        &mut session,
        ledger.as_ref(),
        MemoryCommand::Revoke {
            wallet: "eve".into(),
            log: log_hex.clone(),
            seq: 0,
            note: "spite".into(),
            valid_for: 100_000,
        },
    )
    .unwrap_err();
    assert!(
        refused.to_string().contains("only the log's writer"),
        "{refused}"
    );
    let about = vouching::about_atom(ledger.as_ref(), &log, 0, "spite").unwrap();
    vouching::revoke(&mut session, "eve", &about, 100_000).unwrap();
    let found = vouching::refresh(ledger.as_ref(), &log).unwrap();
    let atoms = [metric.clone(), kilograms.clone(), on_time.clone()];
    assert_eq!(found.standings(&atoms)[0], Standing::Plain);

    // The curator revokes metric units: it is out, the fact drawn from it
    // too, and the vouched one says who vouched.
    memory::run(
        &mut session,
        ledger.as_ref(),
        MemoryCommand::Revoke {
            wallet: "curator".into(),
            log: log_hex.clone(),
            seq: 0,
            note: "the reports moved to imperial".into(),
            valid_for: 100_000,
        },
    )
    .unwrap();
    let found = vouching::refresh(ledger.as_ref(), &log).unwrap();
    let standings = found.standings(&atoms);
    assert_eq!(
        standings[0],
        Standing::Revoked {
            by: writer,
            why: "the reports moved to imperial".into()
        }
    );
    assert_eq!(standings[1], Standing::RestsOnRevoked { atom: metric.id() });
    assert_eq!(
        standings[2],
        Standing::Vouched {
            by: vec![session.id("dana")]
        }
    );
    let shared = sion_cli::agent_memory::shared_core(&log);
    assert!(!shared.contains("metric units"), "{shared}");
    assert!(!shared.contains("kilograms"), "{shared}");
    assert!(shared.contains("\"on time\" (vouched for by"), "{shared}");
    memory::run(
        &mut session,
        ledger.as_ref(),
        MemoryCommand::Standing { log: log_hex },
    )
    .unwrap();

    // The reader's session, taken up again, has a prompt without them: its
    // model is no longer shown the revoked fact.
    let (mut again, lines) = runtime::resume_watched(
        ledger.clone(),
        &saved.state.id,
        Some(Reply::Say("And now?".into())),
        Hooks::default(),
    )
    .unwrap();
    let text = lines.join("\n");
    assert!(
        matches!(&again.ended, Some(Ended::Done { text }) if text == "The revoked fact is gone."),
        "{text}"
    );
    let prompt = again.system_prompt();
    assert!(!prompt.contains("metric units"), "{prompt}");
    assert!(prompt.contains("on time"), "{prompt}");
    saved = again;
    assert!(saved.path().exists());

    // dana withdraws her vouching: the fact is its writer's word again,
    // still kept in mind; her revocation pulls nothing.
    memory::run(
        &mut session,
        ledger.as_ref(),
        MemoryCommand::Revoke {
            wallet: "dana".into(),
            log: log.0.to_hex(),
            seq: 2,
            note: "not sure any more".into(),
            valid_for: 100_000,
        },
    )
    .unwrap();
    let found = vouching::refresh(ledger.as_ref(), &log).unwrap();
    assert_eq!(found.standings(&atoms)[2], Standing::Plain);
    let shared = sion_cli::agent_memory::shared_core(&log);
    assert!(shared.contains("\"on time\""), "{shared}");
    assert!(!shared.contains("vouched for by"), "{shared}");
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn the_best_agent_for_a_task_is_found_among_those_named() {
    let _one = ENVIRONMENT.lock().unwrap_or_else(|e| e.into_inner());
    let home = home("agent-find");
    let (_devnet, ledger) = devnet();
    let mut session = Session::open(ledger.as_ref()).unwrap();
    session
        .fund("alice", AssetId::usd(), Amount::units(1_000))
        .unwrap();
    session
        .fund("owner", AssetId::usd(), Amount::units(1))
        .unwrap();
    let translator = register(&mut session, &ledger, "translator");
    let coder = register(&mut session, &ledger, "coder");

    // An owner lists an agent that summarizes, in a simulated enclave.
    let owner = session.id("owner");
    let rented = ObjectId::derive(b"summarizer");
    let keys = EnclaveKeys::generate();
    let terms = rentals::terms_digest(
        &rented,
        &owner,
        Amount(3_000),
        Amount(15_000),
        ModelAccount::Owner,
    );
    let platform = SimulatedPlatform::generate().unwrap();
    let attestation = platform
        .attest(keys.binding(&rented, terms, 100_000), [0x5A; 48])
        .unwrap();
    let listing = Listing {
        name: "summarizer".into(),
        agent: rented,
        payee: owner,
        per_1k_input: Amount(3_000),
        per_1k_output: Amount(15_000),
        model_account: ModelAccount::Owner,
        unit: Amount(1_000),
        min_deposit: Amount::units(1),
        description: "summarize long reports into one page".into(),
        attestation,
    };
    rentals::publish(&mut session, "owner", &listing, 100_000).unwrap();

    let args = |task: &str, enclave: bool, max: Option<&str>, simulated: bool| FindArgs {
        task: task.split(' ').map(String::from).collect(),
        listings: vec!["owner/summarizer".into()],
        agents: vec![
            format!(
                "{}=translate contracts and reports into French",
                translator.0.to_hex()
            ),
            format!("{}=fix failing Rust tests", coder.0.to_hex()),
        ],
        enclave,
        max_per_1k_output: max.map(String::from),
        measurement: None,
        simulated_platform: simulated.then(|| hex::encode(platform.public_key())),
        from: 0,
        limit: 5,
    };
    let ranked = |found: &FindArgs| {
        let (candidates, need) = reputation::candidates_of(ledger.as_ref(), found).unwrap();
        let records = reputation::records_since(ledger.as_ref(), Pulse(0)).unwrap();
        let task = found.task.join(" ");
        reputation::rank(&task, &candidates, &records, &need)
            .into_iter()
            .map(|m| m.candidate.agent)
            .collect::<Vec<_>>()
    };
    // By the words: the summarizer for reports to summarize, then the
    // translator, whose offer shares a word; the coder shares none.
    let found = ranked(&args(
        "summarize these reports into a page",
        false,
        None,
        true,
    ));
    assert_eq!(found, vec![rented, translator]);
    // In an enclave that checks out: the summarizer only, and only when its
    // platform is the one trusted.
    let found = ranked(&args(
        "summarize these reports into a page",
        true,
        None,
        true,
    ));
    assert_eq!(found, vec![rented]);
    let found = ranked(&args(
        "summarize these reports into a page",
        true,
        None,
        false,
    ));
    assert_eq!(found, Vec::<ObjectId>::new());
    // At most 0.01 USD per 1k output tokens: the summarizer costs more, and
    // the others have no price.
    let found = ranked(&args(
        "summarize these reports into a page",
        false,
        Some("0.01"),
        true,
    ));
    assert_eq!(found, Vec::<ObjectId>::new());
    // The command prints them, and a need it was not given is no need.
    reputation::find_command(
        ledger.as_ref(),
        args("translate the contract into French", false, None, true),
    )
    .unwrap();
    assert_eq!(
        Need::default(),
        Need {
            attested_only: false,
            max_per_1k_output: None
        }
    );
    // Naming no one is refused.
    let none = FindArgs {
        listings: Vec::new(),
        agents: Vec::new(),
        ..args("anything", false, None, false)
    };
    assert!(reputation::candidates_of(ledger.as_ref(), &none).is_err());
    let _ = std::fs::remove_dir_all(&home);
}
