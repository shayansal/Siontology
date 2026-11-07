//! A Siontology agent in ERC-8004's registries: its registration file,
//! written from its profile with the DID the node gives it, read back as
//! the spec requires.

use std::sync::Arc;

use sion_agent::Autonomy;
use sion_cli::erc8004::{self, Erc8004Command};
use sion_cli::profiles::{self, Profile};
use sion_cli::runtime::SharedLedger;
use sion_core::ids::ObjectId;
use sion_node::{Devnet, DevnetConfig};

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
        instructions: Some("You pay Acme's invoices.".into()),
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

#[test]
fn the_registration_file_names_the_agents_did_and_its_entry() {
    let home = std::env::temp_dir().join(format!("erc8004-{}", std::process::id()));
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
    let agent = ObjectId::from_hash(sion_core::hash::Hash32([7; 32]));
    profiles::save_all(&[("lead".to_string(), profile("lead", agent, Vec::new()))].into()).unwrap();
    let out = home.join("registration.json");
    erc8004::run(
        ledger.as_ref(),
        Erc8004Command::Card {
            profile: "lead".into(),
            description: Some("Pays Acme's invoices.".into()),
            a2a: Some("https://lead.example/.well-known/agent-card.json".into()),
            mcp: None,
            web: None,
            image: String::new(),
            x402: true,
            agent_id: Some(22),
            chain: 11_155_111,
            registry: None,
            out: Some(out.clone()),
            data_uri: false,
        },
    )
    .unwrap();
    let file = erc8004::read_registration(&std::fs::read(&out).unwrap()).unwrap();
    assert_eq!(file.name, "lead");
    let did = &file
        .services
        .iter()
        .find(|s| s.name == "DID")
        .unwrap()
        .endpoint;
    assert!(
        did.starts_with("did:sion:") && did.ends_with(&agent.to_string()),
        "{did}"
    );
    assert_eq!(file.registrations[0].agent, 22);
    assert_eq!(
        file.registrations[0].registry,
        format!("eip155:11155111:{}", erc8004::IDENTITY_TESTNET)
    );
    assert!(file.x402 && file.trust.contains(&"crypto-economic".to_string()));
    let _ = std::fs::remove_dir_all(&home);
}
