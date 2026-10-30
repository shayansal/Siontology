//! The app core against a devnet served over HTTP, as a shell uses it.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use sion_app_core::AppCore;
use sion_node::{Devnet, DevnetConfig};

/// Each test runs a whole devnet; two at once on a small machine starve
/// each other's validators past the tests' deadlines, so they take turns.
fn one_at_a_time() -> MutexGuard<'static, ()> {
    static DEVNETS: Mutex<()> = Mutex::new(());
    DEVNETS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[test]
fn an_install_verifies_the_chain_pays_and_delegates() {
    let _turn = one_at_a_time();
    let devnet = Arc::new(
        Devnet::start(DevnetConfig {
            rounds_per_pulse: 2,
            prove_chunk: None,
            ..DevnetConfig::default()
        })
        .unwrap(),
    );
    let api = sion_node::api::serve(devnet.clone(), "127.0.0.1:0").unwrap();
    let dir = std::env::temp_dir().join(format!("sion-app-devnet-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let app = AppCore::open(&dir).unwrap();
    app.set_node(&format!("http://{}", api.address)).unwrap();

    // The first verification pins the chain identity the node serves.
    let deadline = Instant::now() + Duration::from_secs(120);
    let first = loop {
        let verified = app.verify_latest().unwrap();
        if verified["ok"] == true {
            break verified;
        }
        assert!(Instant::now() < deadline, "no Pulse verified: {verified}");
        std::thread::sleep(Duration::from_millis(500));
    };
    assert!(first["pulse"].as_u64().is_some(), "{first}");
    assert_eq!(first["pinned_now"], true, "{first}");
    let lines: Vec<String> = serde_json::from_value(first["lines"].clone()).unwrap();
    assert!(
        lines.iter().any(|l| l.starts_with("certificate:")),
        "{lines:?}"
    );
    assert!(
        lines.iter().any(|l| l.starts_with("availability:")),
        "{lines:?}"
    );
    let pinned = app.settings()["chain"].as_str().unwrap().to_string();
    assert_eq!(pinned.len(), 64);

    // Money: the faucet, then a payment the node executes.
    app.faucet("20000").unwrap();
    let deadline = Instant::now() + Duration::from_secs(60);
    while app.balance().unwrap()["usd"] == "0" {
        assert!(Instant::now() < deadline, "the faucet never paid");
        std::thread::sleep(Duration::from_millis(300));
    }
    let bob = sion_exec::wallet::Wallet::named("bob").id;
    let paid = app.send(&bob.0.to_hex(), "1.5").unwrap();
    assert_eq!(paid["status"], "Success", "{paid}");
    // Its sentences call the devnet's money what it is.
    let receipt = paid["receipt"].as_str().unwrap();
    assert!(receipt.contains("1.5 test USD"), "{paid}");
    assert!(!receipt.contains("test test"), "{paid}");
    // The device proves its own receipt against the certified header.
    assert_eq!(paid["proof"]["ok"], true, "{paid}");
    assert_eq!(paid["proof"]["pinned"], true, "{paid}");

    // Pooled stake: delegate to the first validator, see the delegation.
    let pools = app.pools().unwrap();
    let validator = pools["validators"][0]["validator"]
        .as_str()
        .unwrap()
        .to_string();
    let delegated = app.delegate(&validator[..8], "5000").unwrap();
    assert_eq!(delegated["status"], "Success", "{delegated}");
    let mine = app.delegations().unwrap();
    assert_eq!(mine["delegations"].as_array().unwrap().len(), 1, "{mine}");
    let delegation = mine["delegations"][0]["delegation"]
        .as_str()
        .unwrap()
        .to_string();
    let undelegated = app.delegation_action("undelegate", &delegation).unwrap();
    assert_eq!(undelegated["status"], "Success", "{undelegated}");

    // A later verification still matches the pinned chain.
    let again = app.verify_latest().unwrap();
    assert_eq!(again["ok"], true, "{again}");
    assert_eq!(again["pinned_now"], false, "{again}");
    assert_eq!(app.settings()["chain"], pinned.as_str());
    assert_eq!(app.history()["verified"], 2);

    // A node of another chain fails against the pin.
    app.pin_chain(Some(&"00".repeat(32))).unwrap();
    let wrong = app.verify_latest().unwrap();
    assert_eq!(wrong["ok"], false, "{wrong}");

    api.stop();
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_device_follows_new_epochs_without_starting_over() {
    let _turn = one_at_a_time();
    let devnet = Arc::new(
        Devnet::start(DevnetConfig {
            rounds_per_pulse: 2,
            epoch_length: 6,
            prove_chunk: None,
            ..DevnetConfig::default()
        })
        .unwrap(),
    );
    let api = sion_node::api::serve(devnet.clone(), "127.0.0.1:0").unwrap();
    let dir = std::env::temp_dir().join(format!("sion-app-epochs-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let app = AppCore::open(&dir).unwrap();
    app.set_node(&format!("http://{}", api.address)).unwrap();

    let epoch_of = |verified: &serde_json::Value| -> Option<String> {
        let lines: Vec<String> = serde_json::from_value(verified["lines"].clone()).ok()?;
        lines.into_iter().find(|l| l.starts_with("committees:"))
    };

    // Verify until a Pulse past the first boundary checks out.
    let deadline = Instant::now() + Duration::from_secs(180);
    let first = loop {
        let verified = app.verify_latest().unwrap();
        if verified["ok"] == true && epoch_of(&verified).is_some() {
            break verified;
        }
        assert!(
            Instant::now() < deadline,
            "no Pulse past epoch 0: {verified}"
        );
        std::thread::sleep(Duration::from_millis(500));
    };

    // Two epochs later, only the new boundaries are followed.
    let first_pulse = first["pulse"].as_u64().unwrap();
    let deadline = Instant::now() + Duration::from_secs(180);
    let later = loop {
        let verified = app.verify_latest().unwrap();
        if verified["ok"] == true && verified["pulse"].as_u64().unwrap() >= first_pulse + 12 {
            break verified;
        }
        assert!(Instant::now() < deadline, "the chain stopped: {verified}");
        std::thread::sleep(Duration::from_millis(500));
    };
    // "committees: followed from the pinned epoch 0 to epoch E; N new …"
    let line = epoch_of(&later).unwrap();
    assert!(line.contains("kept from previous verifications"), "{line}");
    let numbers: Vec<u64> = line
        .split(|c: char| !c.is_ascii_digit())
        .filter_map(|word| word.parse().ok())
        .collect();
    let (epoch, new) = (numbers[1], numbers[2]);
    assert!(epoch >= 2 && new < epoch, "started over: {line}");

    api.stop();
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_device_provides_storage_and_proves_its_rounds() {
    let _turn = one_at_a_time();
    use sion_cli::ledger::HttpLedger;
    use sion_cli::session::Session;
    use sion_exec::storage::DataTree;
    use sion_exec::TxKind;

    let devnet = Arc::new(
        Devnet::start(DevnetConfig {
            rounds_per_pulse: 2,
            prove_chunk: None,
            ..DevnetConfig::default()
        })
        .unwrap(),
    );
    let api = sion_node::api::serve(devnet.clone(), "127.0.0.1:0").unwrap();
    let node = format!("http://{}", api.address);
    let dir = std::env::temp_dir().join(format!("sion-app-storage-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let app = AppCore::open(dir.join("app")).unwrap();
    app.set_node(&node).unwrap();
    app.faucet("50").unwrap();
    let deadline = Instant::now() + Duration::from_secs(60);
    while app.balance().unwrap()["usd"] == "0" {
        assert!(Instant::now() < deadline, "the faucet never paid");
        std::thread::sleep(Duration::from_millis(300));
    }
    // Off by default: a tick does nothing.
    assert_eq!(app.storage_tick().unwrap()["enabled"], false);
    app.set_storage(true, 16).unwrap();
    // The data is a file on this machine: the device must be told it may
    // read local sources (strangers' deals cannot make it).
    app.set_storage_local(true).unwrap();

    // A client posts an open deal for a file.
    let data: Vec<u8> = (0..30_000u32).map(|i| (i % 251) as u8).collect();
    let file = dir.join("data.bin");
    std::fs::write(&file, &data).unwrap();
    let tree = DataTree::of_data(&data);
    let ledger = HttpLedger::new(&node);
    let mut client = Session::open(&ledger).unwrap();
    let alice = client.id("alice");
    sion_cli::ledger::Ledger::faucet(
        &ledger,
        alice,
        sion_core::ids::AssetId::usd(),
        sion_core::Amount::units(10),
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(60);
    while client
        .balance("alice", sion_core::ids::AssetId::usd())
        .unwrap()
        == sion_core::Amount::ZERO
    {
        assert!(Instant::now() < deadline, "the faucet never paid alice");
        std::thread::sleep(Duration::from_millis(300));
    }
    let pulse = sion_cli::ledger::LedgerExt::pulse(&ledger).unwrap();
    let posted = client
        .send(
            "alice",
            TxKind::PostDeal {
                root: tree.root(),
                size: tree.size(),
                challenges: 2,
                price: sion_core::Amount(250_000),
                rounds: 3,
                // A proof can go out only once its round's first Pulse has
                // executed; this devnet's Pulses are short, so rounds of 16
                // leave time for one to land.
                round_length: 16,
                collateral: sion_core::Amount::units(1),
                provider: None,
                accept_by: sion_core::ids::Pulse(pulse.0 + 100),
                locator: format!("file://{}", file.display()),
            },
            Vec::new(),
        )
        .unwrap();
    assert_eq!(posted.status, sion_exec::ExecStatus::Success);

    // The device takes it and proves rounds until the deal is closed;
    // turned off after the first proof, it takes nothing new but keeps
    // proving the deal it holds. Three rounds of 16 Pulses: a deadline for
    // a slow machine (one without SHA extensions signs slowly).
    let deadline = Instant::now() + Duration::from_secs(600);
    let mut on = true;
    let mut log: Vec<String> = Vec::new();
    loop {
        let tick = app.storage_tick().unwrap();
        log.extend(
            tick["lines"]
                .as_array()
                .unwrap()
                .iter()
                .map(|line| line.as_str().unwrap_or_default().to_string()),
        );
        let status = app.storage_status().unwrap();
        let deals = status["deals"].as_array().unwrap().clone();
        if let Some(deal) = deals.first() {
            if on && deal["proven"].as_u64().unwrap_or(0) >= 1 {
                app.set_storage(false, 16).unwrap();
                on = false;
            }
            if deal["phase"] == "closed" {
                assert_eq!(deal["proven"], 3, "{status}\n{}", log.join("\n"));
                assert_eq!(status["collected"], "0.75", "{status}");
                break;
            }
        }
        assert!(
            Instant::now() < deadline,
            "the deal never closed: {status}\n{}",
            log.join("\n")
        );
        std::thread::sleep(Duration::from_millis(400));
    }
    api.stop();
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn an_install_watches_the_agents_it_backs_and_stops_them() {
    let _turn = one_at_a_time();
    let devnet = Arc::new(
        Devnet::start(DevnetConfig {
            rounds_per_pulse: 2,
            prove_chunk: None,
            ..DevnetConfig::default()
        })
        .unwrap(),
    );
    let api = sion_node::api::serve(devnet.clone(), "127.0.0.1:0").unwrap();
    let dir = std::env::temp_dir().join(format!("sion-app-agents-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let app = AppCore::open(&dir).unwrap();
    let node = format!("http://{}", api.address);
    app.set_node(&node).unwrap();
    let deadline = Instant::now() + Duration::from_secs(120);
    while app.verify_latest().unwrap()["ok"] != true {
        assert!(Instant::now() < deadline, "no Pulse verified");
        std::thread::sleep(Duration::from_millis(500));
    }
    app.faucet("1000").unwrap();
    let deadline = Instant::now() + Duration::from_secs(60);
    while app.balance().unwrap()["usd"] == "0" {
        assert!(Instant::now() < deadline, "the faucet never paid");
        std::thread::sleep(Duration::from_millis(300));
    }

    // The device's key registers an agent, as the command line would.
    let seed = hex::decode(
        std::fs::read_to_string(dir.join("key.seed"))
            .unwrap()
            .trim(),
    )
    .unwrap();
    let device = sion_exec::wallet::Wallet::from_seed(&seed.try_into().unwrap());
    let ledger = sion_cli::ledger::HttpLedger::new(&node);
    let mut session = sion_cli::session::Session::open(&ledger).unwrap();
    session.add_wallet("device", device);
    let pulse = sion_cli::ledger::LedgerExt::pulse(&ledger).unwrap();
    let view = session
        .must(
            "device",
            sion_exec::TxKind::RegisterAgent {
                agent: sion_exec::wallet::Wallet::named("helper").id,
                card: sion_core::hash::Hash32::ZERO,
                caveats: sion_exec::charter::Caveats {
                    payees: None,
                    max_per_payment: sion_core::Amount::units(10),
                    burst: sion_core::Amount::units(50),
                    rate: sion_core::Amount(50_000),
                    expires_at: sion_core::ids::Pulse(pulse.0 + 100_000),
                    evidence_above: None,
                    evidence_policy: sion_assurance::Policy::standard(),
                    receipts: true,
                    may_delegate: false,
                },
                funding: sion_core::Amount::units(100),
                agent_bond: sion_core::Amount::ZERO,
            },
            Vec::new(),
        )
        .unwrap();
    let agent = view
        .effects
        .events
        .iter()
        .find_map(|e| match e {
            sion_exec::execute::Event::AgentRegistered { agent, .. } => Some(*agent),
            _ => None,
        })
        .unwrap();
    let hex = agent.0.to_hex();

    // The device finds it among the registrations, and checks its standing.
    let found = app.find_agents().unwrap();
    assert_eq!(found["added"], serde_json::json!([hex]), "{found}");
    let standing = |app: &AppCore| app.agents().unwrap()["agents"][0].clone();
    let first = standing(&app);
    assert_eq!(first["status"], "active", "{first}");
    assert_eq!(first["mine"], true, "{first}");
    assert_eq!(first["oversees"], true, "the device backs it: {first}");
    assert_eq!(first["budget"]["remaining"], "100", "{first}");
    let lines = first["verified"].to_string();
    assert!(lines.contains("proven against Pulse"), "{first}");
    assert_eq!(first["pinned"], true, "{first}");
    // Found again, nothing is added twice.
    assert_eq!(app.find_agents().unwrap()["added"], serde_json::json!([]));

    // The kill switch, from the device: suspend, resume, revoke.
    for (action, status) in [
        ("suspend", "suspended"),
        ("resume", "active"),
        ("revoke", "revoked"),
    ] {
        let answer = app.agent_action(action, &hex).unwrap();
        assert_eq!(answer["status"], "Success", "{action}: {answer}");
        assert_eq!(answer["proof"]["ok"], true, "{action}: {answer}");
        let now = standing(&app);
        assert_eq!(now["status"], status, "{action}: {now}");
        assert_eq!(now["good_standing"], status == "active", "{now}");
        if status != "active" {
            assert_eq!(
                now["budget"]["can_spend_next_pulse"], "0",
                "a stopped agent spends nothing: {now}"
            );
        }
    }
    assert!(app.agent_action("pause", &hex).is_err());
    app.unwatch_agent(&hex).unwrap();
    assert_eq!(app.agents().unwrap()["agents"], serde_json::json!([]));
    std::fs::remove_dir_all(&dir).ok();
}
