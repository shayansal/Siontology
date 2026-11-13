//! An agent's record, on a devnet, with scripted models: what the ledger
//! shows of it, recomputed by a third party from the ledger alone; what
//! its operator says of its sessions, kept apart; a record that was
//! altered, found out; and a warranty on the operator's bond that a
//! counterparty relies on and is compensated from when a challenge to it
//! is upheld.

use std::sync::{Arc, Mutex};

use serde_json::json;
use sion_agent::Autonomy;
use sion_cli::ledger::LedgerExt;
use sion_cli::profiles::Profile;
use sion_cli::record::{self, Warranty};
use sion_cli::runtime::SharedLedger;
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

#[test]
fn an_agents_record_is_recomputed_from_the_ledger_and_its_warranty_pays() {
    let _one = ENVIRONMENT.lock().unwrap_or_else(|e| e.into_inner());
    let home = home("agent-record");
    let (_devnet, ledger) = devnet();
    let mut session = Session::open(ledger.as_ref()).unwrap();
    for owner in ["alice", "bob", "carol", "dana"] {
        session
            .fund(owner, AssetId::usd(), Amount::units(2_000))
            .unwrap();
    }
    let agent = register(&mut session, &ledger, "alice", "clerk");

    // Two sessions: a payment within the charter, and one over it, which
    // the ledger refuses. The agent says what it expects of each.
    let pays = |name: &str, amount: &str| {
        script(
            &home,
            name,
            json!([
                {"call": "ledger_pay", "input": {
                    "to": "bob", "amount": amount, "purpose": "invoice",
                    "expect": {"outcome": "bob is paid"}
                }},
                {"say": "Done."}
            ]),
        )
    };
    for (name, amount) in [("pays-5", "5"), ("pays-50", "50")] {
        profile("clerk", agent, vec![pays(name, amount)], true)
            .start(ledger.clone(), &format!("Pay bob {amount} USD."))
            .unwrap();
    }

    // The record: what the ledger shows, and what the operator's sessions
    // show, apart.
    let written = record::write(ledger.as_ref(), agent, 0, None, true).unwrap();
    let shown = &written.ledger;
    assert!(shown.registered_at.is_some());
    assert_eq!(
        (shown.payments, shown.paid, shown.largest, shown.payees),
        (1, Amount::units(5), Amount::units(5), 1)
    );
    assert_eq!(
        (shown.suspended, shown.challenges, shown.slashes),
        (0, 0, 0)
    );
    assert_eq!(written.account, session.id("clerk"));
    assert_eq!(written.owner, Some(session.id("alice")));
    assert_eq!(written.charter["max_per_payment_usd"], "10");
    let said = written.reported.as_ref().unwrap();
    assert_eq!((said.sessions, said.ended["done"]), (2, 2));
    assert_eq!(
        (said.calls, said.calls_failed, said.payments_refused),
        (2, 1, 1)
    );
    assert_eq!((said.expected, said.expected_held), (2, 1));
    let lines = record::lines(&written).join("\n");
    eprintln!("{lines}");
    assert!(
        lines.contains("1 payment(s) to 1 account(s), 5 USD in all"),
        "{lines}"
    );
    assert!(lines.contains("1 payment(s) the ledger refused"), "{lines}");

    // Alice commits to it. Anyone given the record recomputes what the
    // ledger shows and finds her commitment; what she says of her sessions
    // is shown as her word.
    let claim = record::commit(&mut session, "alice", &written).unwrap();
    let (lines, holds) = record::check(ledger.as_ref(), &written).unwrap();
    let checked = lines.join("\n");
    eprintln!("{checked}");
    assert!(holds, "{checked}");
    assert!(
        checked.contains("holds: what the record says the ledger shows"),
        "{checked}"
    );
    assert!(
        checked.contains(&format!(
            "holds: its owner committed to this record in claim {}",
            claim.short()
        )),
        "{checked}"
    );
    assert!(checked.contains("its operator's word, which the ledger cannot check"));

    // A record altered after the fact does not hold, and is not the one
    // she committed to.
    let mut altered = written.clone();
    altered.ledger.payments = 9;
    altered.ledger.falsified = 0;
    let (lines, holds) = record::check(ledger.as_ref(), &altered).unwrap();
    let checked = lines.join("\n");
    assert!(!holds);
    assert!(
        checked.contains("differs: payments: the record says 9, the ledger shows 1"),
        "{checked}"
    );
    assert!(
        checked.contains("not on the ledger: no claim of its owner's commits"),
        "{checked}"
    );

    // What happens afterwards does not change what the record says of its
    // span; a change to the charter's standing is said as that.
    profile("clerk", agent, vec![pays("pays-3", "3")], true)
        .start(ledger.clone(), "Pay bob 3 USD.")
        .unwrap();
    session
        .must("alice", TxKind::SuspendAgent { charter: agent }, Vec::new())
        .unwrap();
    let (lines, holds) = record::check(ledger.as_ref(), &written).unwrap();
    let checked = lines.join("\n");
    assert!(holds, "{checked}");
    assert!(
        checked.contains("changed since: the charter is no longer"),
        "{checked}"
    );
    let later = record::write(ledger.as_ref(), agent, 0, None, false).unwrap();
    assert_eq!((later.ledger.payments, later.ledger.suspended), (2, 1));
    assert_eq!(later.ledger.paid, Amount::units(8));
    assert!(later.reported.is_none());

    // The warranty: alice stands behind the agent on a bond, under carol.
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
    record::run(
        &mut session,
        ledger.as_ref(),
        record::RecordCommand::Warrant {
            wallet: "alice".into(),
            agent: agent.as_hash().to_hex(),
            covers: "It pays only for invoices its principal was sent, once each.".into(),
            adjudicator: adjudicator.0.to_hex(),
            bond: Some("200".into()),
            valid_for: 1_000_000,
        },
    )
    .unwrap();
    // It is found under the agent's warranty key, names the record alice
    // committed to, and is what dana relies on for 20 USD.
    let mut feed = sion_cli::feeds::Feed::default();
    feed.scan(ledger.as_ref(), &[&record::warranty_key(&agent)])
        .unwrap();
    let warranty_claim = feed.entries[0].claim;
    let read =
        sion_cli::feeds::read::<Warranty>(ledger.as_ref(), warranty_claim, record::WARRANTY_SCHEMA)
            .unwrap()
            .unwrap();
    assert!(read.active && read.issuer == session.id("alice"));
    assert_eq!(read.says.record, Some(claim.as_hash().to_hex()));
    let until = Pulse(ledger.as_ref().pulse().unwrap().0 + 1_000);
    let view = session
        .must(
            "dana",
            TxKind::Accept {
                claim: warranty_claim,
                value_at_risk: Amount::units(20),
                policy: sion_assurance::Policy::standard(),
                until,
                nonce: 0,
            },
            Vec::new(),
        )
        .unwrap();
    let reserved = view
        .effects
        .events
        .iter()
        .find_map(|e| match e {
            Event::Reserved { amount, .. } => Some(*amount),
            _ => None,
        })
        .unwrap();
    assert!(reserved >= Amount::units(20), "{reserved}");

    // The agent paid an invoice twice, dana says: she challenges the bond
    // the warranty rests on, carol upholds it, and dana is compensated
    // from alice's bond without anyone's leave.
    let bond = sion_exec::execute::bond_atom(
        sion_assurance::AtomClass::Economic,
        &session.id("alice"),
        record::WARRANTY_BOND,
    )
    .id;
    let view = session
        .must(
            "dana",
            TxKind::Challenge {
                claim: warranty_claim,
                target: bond,
                evidence: sion_core::hash::hash_domain("test/evidence", b"paid twice"),
                deposit: Amount::units(10),
            },
            Vec::new(),
        )
        .unwrap();
    let challenge = view
        .effects
        .events
        .iter()
        .find_map(|e| match e {
            Event::ChallengeOpened { challenge, .. } => Some(*challenge),
            _ => None,
        })
        .unwrap();
    let before = session.balance("dana", AssetId::usd()).unwrap();
    let view = session
        .must(
            "carol",
            TxKind::Rule {
                challenge,
                upheld: true,
            },
            Vec::new(),
        )
        .unwrap();
    let compensated = view
        .effects
        .events
        .iter()
        .find_map(|e| match e {
            Event::Compensated { amount, .. } => Some(*amount),
            _ => None,
        })
        .unwrap();
    assert!(compensated >= Amount::units(20), "{compensated}");
    assert!(session.balance("dana", AssetId::usd()).unwrap() > before);

    // And the agent's record says so from then on, for whoever asks next.
    let after = record::write(ledger.as_ref(), agent, 0, None, false).unwrap();
    assert_eq!(
        (
            after.ledger.bonds,
            after.ledger.challenges,
            after.ledger.upheld
        ),
        (1, 1, 1)
    );
    assert!(after.ledger.slashes >= 1 && after.ledger.slashed > Amount::ZERO);
    let lines = record::lines(&after).join("\n");
    eprintln!("{lines}");
    assert!(
        lines.contains("1 posted, 1 challenge(s), 1 upheld"),
        "{lines}"
    );
}
