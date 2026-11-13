//! Lessons shared between owners, on a devnet, with scripted models: one
//! owner's agent keeps a lesson on its own sealed tasks and its owner
//! shares it on a bond; another owner's agent finds it, tries it on tasks
//! of its own, keeps it only because its own sealed tasks confirm it, says
//! whose lesson it was and pays what the lesson asks. A lesson that does
//! nothing there is turned down, and a copy of someone's lesson earns its
//! copier nothing.

use std::sync::{Arc, Mutex};

use serde_json::json;
use sion_agent::compare::TaskSet;
use sion_agent::Autonomy;
use sion_cli::improve_loop;
use sion_cli::ledger::LedgerExt;
use sion_cli::lessons::{self, Counts, Lesson};
use sion_cli::profiles::{self, Profile};
use sion_cli::runtime::SharedLedger;
use sion_cli::sealed;
use sion_cli::self_improve::{self, ImproveCommand};
use sion_cli::session::Session;
use sion_core::hash::Hash32;
use sion_core::ids::{AssetId, ObjectId, Pulse};
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

/// What the proposer finds, and what both agents' script follows.
const LESSON: &str = "get_total answers in cents: divide by 100 for USD.";

/// Tasks half of which ask for a total in USD of a tool that answers in
/// cents, as `ask` words the question for invoice `n`.
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

/// How many of a set's tasks ask for a total.
fn totals(set: &TaskSet) -> usize {
    set.cases
        .iter()
        .filter(|c| c.id.starts_with("total-"))
        .count()
}

/// Seals `tasks` for `profile`, half held apart, with a seed that leaves
/// tasks asking for a total on both sides.
fn seal(profile: &Profile, tasks: &[u8]) -> sealed::Seal {
    let mut seed = 1;
    loop {
        let seal = sealed::seal(profile, tasks, 50, 4, 2, seed, 2).unwrap();
        let (train, _) = seal.train_set().unwrap();
        let (held, _) = seal.sealed_set().unwrap();
        if (2..=4).contains(&totals(&train)) && (2..=4).contains(&totals(&held)) {
            return seal;
        }
        seed += 1;
    }
}

#[test]
fn a_lesson_one_agent_kept_is_kept_by_another_only_on_its_own_sealed_tasks() {
    let _one = ENVIRONMENT.lock().unwrap_or_else(|e| e.into_inner());
    let home = home("agent-lessons");
    let (_devnet, ledger) = devnet();
    let mut session = Session::open(ledger.as_ref()).unwrap();
    for owner in ["alice", "bob", "carol", "eve"] {
        session
            .fund(owner, AssetId::usd(), Amount::units(2_000))
            .unwrap();
    }
    let clerk_agent = register(&mut session, &ledger, "alice", "clerk");
    let teller_agent = register(&mut session, &ledger, "bob", "teller");

    // Both agents take the tool's answer for USD unless they have learned
    // otherwise; either way they say what they expect of the call.
    let books_keeper = script(
        &home,
        "keeper",
        json!([
            {"when": "how many invoices", "call": "count_open", "input": {}},
            {"when": "how many invoices", "say": "3 invoices are open."},
            {"when": "get_total answers in cents", "call": "get_total", "input": {
                "invoice": "7",
                "expect": {"outcome": "the total in cents", "contains": ["cents"]}
            }},
            {"when": "get_total answers in cents", "say": "The total is 12.50 USD."},
            {"call": "get_total", "input": {
                "invoice": "7",
                "expect": {"outcome": "the total in USD", "contains": ["usd"]}
            }},
            {"say": "The total is 1250 USD."}
        ]),
    );
    let clerk = profile("clerk", clerk_agent, vec![books_keeper.clone()], true);
    let teller = profile("teller", teller_agent, vec![books_keeper], true);
    profiles::save_all(
        &[
            ("clerk".to_string(), clerk.clone()),
            ("teller".to_string(), teller.clone()),
        ]
        .into(),
    )
    .unwrap();

    // Alice's agent learns the lesson from its own work: sealed, proposed
    // from what it did not expect, confirmed, published and made its own.
    let clerk_tasks = books("books", |n| {
        format!("What is the total of invoice {n}, in USD?")
    });
    seal(&clerk, &clerk_tasks);
    sealed::publish(&mut session, "alice", "clerk").unwrap();
    let proposer = script(
        &home,
        "proposer",
        json!([
            {"when": "the result does not hold \"usd\"", "call": "propose_change", "input": {
                "learned": LESSON, "why": "it expected USD of get_total and was given cents"
            }},
            {"say": "Done."}
        ]),
    );
    self_improve::run_with(
        &mut session,
        ledger.as_ref(),
        ImproveCommand::Run {
            profile: "clerk".into(),
            proposer,
            rounds: 1,
            runs: 4,
            budget_tokens: 20_000,
            total_tokens: 0,
            models: Vec::new(),
            seed: Some(3),
            practice: None,
            challenger: None,
            apply: true,
            wallet: Some("alice".into()),
        },
    )
    .unwrap();
    let kept = improve_loop::lineage("clerk").unwrap();
    assert_eq!(kept.len(), 1, "{kept:?}");
    assert_eq!(kept[0].to.learned, LESSON);
    assert!(kept[0].lesson.is_none() && kept[0].claim.is_some());

    // An adjudicator: carol, on a bond of her own.
    session
        .must(
            "carol",
            TxKind::PostBond {
                class: sion_assurance::AtomClass::Adjudicator,
                label: "adjudicator".into(),
                amount: Amount::units(100),
            },
            Vec::new(),
        )
        .unwrap();
    let adjudicator = sion_exec::execute::bond_atom(
        sion_assurance::AtomClass::Adjudicator,
        &session.id("carol"),
        "adjudicator",
    )
    .id;

    // Alice shares it: on her bond, asking 2 USD of whoever keeps it.
    self_improve::run_with(
        &mut session,
        ledger.as_ref(),
        ImproveCommand::Share {
            profile: "clerk".into(),
            step: None,
            wallet: "alice".into(),
            adjudicator: adjudicator.0.to_hex(),
            bond: Some("50".into()),
            price: "2".into(),
            valid_for: 1_000_000,
        },
    )
    .unwrap();
    let found = lessons::refresh(ledger.as_ref()).unwrap();
    assert_eq!(found.len(), 1, "{found:?}");
    let shared = &found[0];
    assert!(shared.active);
    assert_eq!(shared.author, session.id("alice"));
    assert_eq!(shared.lesson.text, LESSON);
    assert_eq!(shared.lesson.tools, ["count_open", "get_total"]);
    assert_eq!(shared.lesson.step, kept[0].id);
    assert_eq!(shared.lesson.step_claim, kept[0].claim);
    assert_eq!(shared.lesson.price(), Amount::units(2));
    assert_eq!(shared.lesson.sealed.after, (24, 24));
    assert!(shared.lesson.sealed.before.0 < 24);
    assert!(shared.adoptions.is_empty());
    // Its evidence is on the ledger: the change, published by alice when
    // it was kept, on tasks she sealed before.
    assert!(
        matches!(shared.evidence, lessons::Evidence::Published { kept_at, sealed_at: Some(sealed_at) } if sealed_at < kept_at),
        "{:?}",
        shared.evidence
    );
    let alices = shared.claim;

    // Eve shares two of her own, with evidence nobody can check: a lesson
    // that does nothing, and a copy of alice's words, asking more.
    session
        .must(
            "eve",
            TxKind::PostBond {
                class: sion_assurance::AtomClass::Economic,
                label: lessons::AUTHOR_BOND.into(),
                amount: Amount::units(50),
            },
            Vec::new(),
        )
        .unwrap();
    let eves = |text: &str| Lesson {
        text: text.into(),
        why: "it helps".into(),
        tools: vec!["get_total".into()],
        agent: ObjectId::from_hash(Hash32([7; 32])),
        step: "0000000000000000".into(),
        step_claim: None,
        seal_claim: None,
        chosen: Counts {
            before: (0, 24),
            after: (24, 24),
        },
        sealed: Counts {
            before: (0, 24),
            after: (24, 24),
        },
        confirm: (0.9, 1.0),
        price_micros: 9_000_000,
    };
    let french = lessons::share(
        &mut session,
        "eve",
        &eves("Answer in French."),
        adjudicator,
        1_000_000,
    )
    .unwrap();
    let copy = lessons::share(&mut session, "eve", &eves(LESSON), adjudicator, 1_000_000).unwrap();
    // And she names a claim of hers under an adoption's key, to look as if
    // alice's lesson were hers to be thanked for: it is no adoption.
    session
        .must(
            "eve",
            TxKind::IndexClaim {
                key: lessons::adopted_key(&alices),
                claim: french,
            },
            Vec::new(),
        )
        .unwrap();

    // Bob's agent has tasks of its own, worded otherwise, sealed by bob.
    let teller_tasks = books("ledgers", |n| format!("How much is invoice {n}, in USD?"));
    seal(&teller, &teller_tasks);
    sealed::publish(&mut session, "bob", "teller").unwrap();
    let learn = |session: &mut Session<'_>, only: Vec<String>| {
        self_improve::run_with(
            session,
            ledger.as_ref(),
            ImproveCommand::Learn {
                profile: "teller".into(),
                lessons: only,
                by: Vec::new(),
                adoptions: 0,
                most: 4,
                runs: 4,
                budget_tokens: 20_000,
                seed: Some(3),
                apply: true,
                wallet: Some("bob".into()),
                pay_up_to: Some("5".into()),
            },
        )
        .unwrap();
    };

    // The lesson that does nothing is tried, and not kept: nothing is
    // published and nobody is paid.
    let eve_before = session.balance("eve", AssetId::usd()).unwrap();
    learn(
        &mut session,
        vec![french.as_hash().to_hex()[..10].to_string()],
    );
    assert!(improve_loop::lineage("teller").unwrap().is_empty());
    assert_eq!(
        lessons::tried("teller", &sealed::current("teller").unwrap().sealed),
        [french.as_hash().to_hex()].into()
    );
    assert_eq!(sealed::current("teller").unwrap().used, 0);

    // Then the rest: alice's lesson, the older, is tried first and kept on
    // bob's own sealed tasks; eve's copy of it changes nothing more.
    let alice_before = session.balance("alice", AssetId::usd()).unwrap();
    learn(&mut session, Vec::new());
    let steps = improve_loop::lineage("teller").unwrap();
    assert_eq!(steps.len(), 1, "{steps:?}");
    let step = &steps[0];
    assert_eq!(step.to.learned, LESSON);
    assert_eq!(step.lesson, Some(alices.as_hash().to_hex()));
    assert!(step.confirm.0 > 0.0 && step.sealed_after.passed > step.sealed_before.passed);
    assert_eq!(sealed::current("teller").unwrap().used, 1);
    // Alice is paid what her lesson asks, and eve nothing.
    assert_eq!(
        session.balance("alice", AssetId::usd()).unwrap(),
        Amount(alice_before.0 + Amount::units(2).0)
    );
    assert_eq!(session.balance("eve", AssetId::usd()).unwrap(), eve_before);
    let tried = lessons::tried("teller", &sealed::current("teller").unwrap().sealed);
    assert!(tried.contains(&alices.as_hash().to_hex()) && tried.contains(&copy.as_hash().to_hex()));

    // The ledger says who kept it since: bob, with what his sealed tasks
    // showed and what he paid. Eve's claim under the key is no adoption,
    // and her lessons were kept by no one.
    let found = lessons::refresh(ledger.as_ref()).unwrap();
    assert_eq!(found.len(), 3);
    let shared = found.iter().find(|f| f.claim == alices).unwrap();
    assert_eq!(shared.adoptions.len(), 1, "{:?}", shared.adoptions);
    let (by, adoption) = &shared.adoptions[0];
    assert_eq!(*by, session.id("bob"));
    assert_eq!(adoption.agent, teller_agent);
    assert_eq!(adoption.step, step.id);
    assert_eq!(adoption.step_claim, step.claim);
    assert_eq!(adoption.paid_micros, 2_000_000);
    assert_eq!(adoption.sealed.after, (24, 24));
    assert!(found
        .iter()
        .filter(|f| f.claim != alices)
        .all(|f| f.adoptions.is_empty() && matches!(f.evidence, lessons::Evidence::WordAlone(_))));
    let shown = lessons::lines(&found).join("\n");
    eprintln!("{shown}");
    assert!(
        shown.contains("kept by 1 other owner(s)' agents since:"),
        "{shown}"
    );
    assert!(shown.contains("paid 2 USD"), "{shown}");

    // Its own lesson is not offered to the agent that wrote it, and bob's
    // agent has none left to try.
    let train = sealed::current("clerk").unwrap().train_set().unwrap().0;
    assert!(lessons::offered(
        &found,
        &clerk_agent,
        &lessons::tools_of(&train),
        &Default::default(),
        &lessons::Choice {
            most: 9,
            ..Default::default()
        }
    )
    .iter()
    .all(|o| o.claim != alices.as_hash().to_hex()));
    learn(&mut session, Vec::new());
    assert_eq!(improve_loop::lineage("teller").unwrap().len(), 1);
}
