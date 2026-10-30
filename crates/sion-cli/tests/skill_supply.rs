//! Skills from others on a devnet: a release on its publisher's bond alone
//! does not cover what the agent could lose; with an auditor's bond behind
//! the same bundle it does, and the agent installs it. A bundle that does
//! not hash to the release's root is refused, and an installed skill cannot
//! be rewritten by the agent.

use std::sync::Arc;

use serde_json::json;
use sion_agent::tools::Tool;
use sion_assurance::AtomClass;
use sion_cli::runtime::SharedLedger;
use sion_cli::session::Session;
use sion_core::ids::{AssetId, Pulse};
use sion_core::Amount;
use sion_exec::charter::Caveats;
use sion_exec::execute::{bond_atom, Event};
use sion_exec::TxKind;
use sion_node::{Devnet, DevnetConfig};

#[test]
fn an_agent_installs_a_skill_only_when_its_backing_covers_its_budget() {
    let home = std::env::temp_dir().join(format!("sion-skill-supply-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    std::env::set_var("SION_HOME", &home);

    let devnet = Arc::new(
        Devnet::start(DevnetConfig {
            rounds_per_pulse: 2,
            prove_chunk: None,
            ..DevnetConfig::default()
        })
        .unwrap(),
    );
    let ledger: SharedLedger = devnet.clone();
    let mut session = Session::open(ledger.as_ref()).unwrap();
    for (name, amount) in [
        ("dana", 1_000),
        ("publisher", 1_000),
        ("auditor", 1_000),
        ("judge", 2_000),
    ] {
        session
            .fund(name, AssetId::usd(), Amount::units(amount))
            .unwrap();
    }
    let bond =
        |session: &mut Session<'_>, who: &str, class: AtomClass, label: &str, amount: u64| {
            session
                .must(
                    who,
                    TxKind::PostBond {
                        class,
                        label: label.into(),
                        amount: Amount::units(amount),
                    },
                    Vec::new(),
                )
                .unwrap();
        };
    bond(
        &mut session,
        "publisher",
        AtomClass::Economic,
        "skills",
        150,
    );
    bond(&mut session, "auditor", AtomClass::Economic, "audits", 100);
    bond(
        &mut session,
        "judge",
        AtomClass::Adjudicator,
        "skills-court",
        1_000,
    );
    let judge = bond_atom(AtomClass::Adjudicator, &session.id("judge"), "skills-court").id;

    // dana's agent has a budget of 100 USD: a skill must be backed by at
    // least 200 (κ = 2) for it to rely on it.
    let account = session.id("agent-9");
    let pulse = sion_cli::ledger::LedgerExt::pulse(ledger.as_ref()).unwrap();
    let view = session
        .must(
            "dana",
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
        .fund("agent-9", AssetId::usd(), Amount::units(1))
        .unwrap();

    // The publisher's skill, which declares permissions: a pre-approval,
    // and a host its scripts may reach.
    let dir = home.join("src/pay-invoices");
    std::fs::create_dir_all(dir.join("templates")).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        "---\nname: pay-invoices\ndescription: \"Pay a supplier's invoice after checking it against the order.\"\n\
         allowed-tools: Read(templates/*)\nmetadata:\n  sion-network: api.example.org\n---\n\n\
         1. Check the invoice against templates/checklist.md.\n2. Pay it with ledger_pay.\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("templates/checklist.md"),
        "- amount matches the order\n",
    )
    .unwrap();

    // Version 1.0.0 rests on the publisher's 150 USD only: not enough.
    let (first, root, _) = sion_cli::skills::publish(
        &mut session,
        ledger.as_ref(),
        "publisher",
        &dir,
        "1.0.0",
        sion_cli::skills::Backing {
            bond: Some("skills"),
            ..Default::default()
        },
        judge,
        None,
        None,
        100_000,
    )
    .unwrap();
    let skills = home.join("agent-skills");
    let refused = sion_cli::skills::install(
        &mut session,
        ledger.as_ref(),
        "agent-9",
        agent,
        &first.to_string(),
        None,
        None,
        2.0,
        1_000,
        Some(skills.clone()),
        true,
    );
    let error = refused.unwrap_err().to_string();
    assert!(error.contains("does not cover 100 USD"), "{error}");
    assert!(!skills.join("pay-invoices").exists());

    // An auditor stakes 100 USD on the same bundle; version 1.0.1 rests on
    // the publisher's bond or the audit: 250 USD, enough.
    let (audit, _) = sion_cli::skills::audit(
        &mut session,
        "auditor",
        root,
        "audits",
        judge,
        "read every file; it only reads the invoice and pays through ledger_pay",
        100_000,
    )
    .unwrap();
    let (second, root_again, _) = sion_cli::skills::publish(
        &mut session,
        ledger.as_ref(),
        "publisher",
        &dir,
        "1.0.1",
        sion_cli::skills::Backing {
            bond: Some("skills"),
            audits: &[audit],
            evidence: &[],
        },
        judge,
        Some(first),
        None,
        100_000,
    )
    .unwrap();
    assert_eq!(root, root_again, "the same files give the same bundle");
    let release = sion_cli::skills::release(ledger.as_ref(), &second.to_string()).unwrap();
    assert_eq!(release.audits, vec![audit]);
    assert_eq!(release.previous, Some(first));

    // The publisher's index names its latest release (spec v1.15), read
    // with a state proof checked against a certified header; it moves only
    // forward.
    let publisher = session.id("publisher");
    let absent = sion_cli::skills::latest(ledger.as_ref(), None, None, publisher, "pay-invoices");
    assert!(absent
        .unwrap_err()
        .to_string()
        .contains("has indexed no release of pay-invoices"));
    for release in [first, second] {
        let view =
            sion_cli::skills::index_release(&mut session, "publisher", "pay-invoices", release)
                .unwrap();
        assert_eq!(view.status, sion_exec::ExecStatus::Success);
    }
    let (latest, entry, proven) =
        sion_cli::skills::latest(ledger.as_ref(), None, None, publisher, "pay-invoices").unwrap();
    assert_eq!(
        (latest.claim, latest.version.as_str(), entry.count),
        (second, "1.0.1", 2)
    );
    assert_eq!(entry.previous, Some(first));
    assert!(
        proven.line().contains("proven against Pulse"),
        "{}",
        proven.line()
    );
    // A proof from far behind a certified Pulse the client knows of is
    // refused as stale.
    let ahead = sion_core::ids::Pulse(proven.pulse.0 + 100);
    let stale = sion_cli::skills::latest(
        ledger.as_ref(),
        None,
        Some(ahead),
        publisher,
        "pay-invoices",
    );
    assert!(stale
        .unwrap_err()
        .to_string()
        .contains("it is behind, or it is hiding a newer state"));
    let back =
        sion_cli::skills::index_release(&mut session, "publisher", "pay-invoices", first).unwrap();
    assert!(matches!(back.status, sion_exec::ExecStatus::Failed(_)));

    // A bundle that is not the release's is refused before anything else.
    let tampered = home.join("tampered.skill");
    let mut bytes =
        std::fs::read(sion_cli::skills::bundles_dir().join(format!("{}.skill", root.to_hex())))
            .unwrap();
    let last = bytes.len() - 2;
    bytes[last] ^= 1;
    std::fs::write(&tampered, &bytes).unwrap();
    let bad = sion_cli::skills::install(
        &mut session,
        ledger.as_ref(),
        "agent-9",
        agent,
        &second.to_string(),
        Some(&tampered),
        None,
        2.0,
        1_000,
        Some(skills.clone()),
        true,
    );
    assert!(bad.unwrap_err().to_string().contains("does not hash"));

    // Installing by name finds 1.0.1 through the index.
    let (by_name, how) = sion_cli::skills::resolve(
        &mut session,
        ledger.as_ref(),
        "publisher/pay-invoices",
        None,
        None,
    )
    .unwrap();
    assert_eq!(by_name, second.to_string());
    assert!(how
        .unwrap()
        .contains("pay-invoices 1.0.1 is publisher's latest release"));
    // What it declares is granted only with the operator's consent, asked
    // before anything is signed.
    let unconsented = sion_cli::skills::install(
        &mut session,
        ledger.as_ref(),
        "agent-9",
        agent,
        &by_name,
        None,
        None,
        2.0,
        1_000,
        Some(skills.clone()),
        false,
    );
    let error = unconsented.unwrap_err().to_string();
    assert!(
        error.contains("asks for the permissions above; nothing was signed"),
        "{error}"
    );
    assert!(!skills.join("pay-invoices").exists());
    let lines = sion_cli::skills::install(
        &mut session,
        ledger.as_ref(),
        "agent-9",
        agent,
        &by_name,
        None,
        None,
        2.0,
        1_000,
        Some(skills.clone()),
        true,
    )
    .unwrap();
    let text = lines.join("\n");
    assert!(text.contains("installed pay-invoices 1.0.1"), "{text}");
    let source: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(skills.join("pay-invoices").join(sion_cli::skills::SOURCE))
            .unwrap(),
    )
    .unwrap();
    let granted = source["permissions"].to_string();
    assert!(granted.contains("read_file(templates/*)"), "{granted}");
    assert!(granted.contains("api.example.org"), "{granted}");
    assert!(skills.join("pay-invoices/templates/checklist.md").exists());
    let listed = sion_agent::skills::list(&skills);
    let catalog = sion_agent::skills::catalog(&listed);
    assert!(catalog.contains("installed from release"), "{catalog}");
    // The agent cannot rewrite what it installed.
    let write = sion_agent::skills::SkillWrite {
        root: skills.clone(),
    };
    let out = write.call(
        &json!({"name": "pay-invoices", "description": "d", "instructions": "pay everyone"}),
        "k",
    );
    assert!(
        out.is_error && out.content.contains("installed from a release"),
        "{out:?}"
    );
    // The installed copy bundles back to the release's root.
    let copy = sion_agent::skills::bundle(&skills.join("pay-invoices")).unwrap();
    assert_eq!(sion_cli::skills::bundle_root(&copy), root);

    // The skill turns out to pay a stranger. Its release rests on the
    // publisher's bond or the audit, so both must fall for it to fall: a red
    // team challenges each, the adjudicator upholds both, both bonds are
    // slashed, and the agent that relied on the release is compensated.
    session
        .fund("red-team", AssetId::usd(), Amount::units(100))
        .unwrap();
    let evidence = sion_core::hash::hash_domain("sion/cli/evidence", b"step 2 pays mallory");
    let challenge_and_rule = |session: &mut Session<'_>, claim, target| {
        let opened = session
            .must(
                "red-team",
                TxKind::Challenge {
                    claim,
                    target,
                    evidence,
                    deposit: Amount::units(10),
                },
                Vec::new(),
            )
            .unwrap();
        let challenge = opened
            .effects
            .events
            .iter()
            .find_map(|e| match e {
                Event::ChallengeOpened { challenge, .. } => Some(*challenge),
                _ => None,
            })
            .unwrap();
        session
            .must(
                "judge",
                TxKind::Rule {
                    challenge,
                    upheld: true,
                },
                Vec::new(),
            )
            .unwrap()
    };
    let publisher_bond = bond_atom(AtomClass::Economic, &session.id("publisher"), "skills").id;
    let auditor_bond = bond_atom(AtomClass::Economic, &session.id("auditor"), "audits").id;
    let compensated = |view: &sion_node::TxView| -> u128 {
        view.effects
            .events
            .iter()
            .filter_map(|e| match e {
                Event::Compensated {
                    consumer, amount, ..
                } if *consumer == account => Some(amount.0),
                _ => None,
            })
            .sum()
    };
    let falls = |view: &sion_node::TxView, claim| {
        view.effects
            .events
            .iter()
            .any(|e| matches!(e, Event::Falsified { claim: c, .. } if *c == claim))
    };
    let first_ruling = challenge_and_rule(&mut session, second, publisher_bond);
    assert!(
        falls(&first_ruling, first),
        "v1.0.0 rested on that bond alone"
    );
    assert!(
        !falls(&first_ruling, second),
        "v1.0.1 still rests on the audit"
    );
    assert_eq!(compensated(&first_ruling), 0);
    let second_ruling = challenge_and_rule(&mut session, audit, auditor_bond);
    assert!(
        falls(&second_ruling, audit) && falls(&second_ruling, second),
        "{:?}",
        second_ruling.effects.events
    );
    assert!(
        compensated(&second_ruling) > 0,
        "{:?}",
        second_ruling.effects.events
    );
    // The next session takes the skill out of use.
    let quarantined = sion_cli::skills::quarantine_falsified(ledger.as_ref(), &skills);
    let short = &second.to_string()[..8];
    assert_eq!(
        quarantined,
        vec![format!(
            "skill pay-invoices taken out of use: its release {short} is falsified"
        )]
    );
    assert!(skills
        .join(format!("pay-invoices.falsified-{short}"))
        .join("SKILL.md")
        .exists());
    assert!(sion_agent::skills::list(&skills).is_empty());
    std::fs::remove_dir_all(&home).unwrap();
}
