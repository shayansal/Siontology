//! The app core against a devnet served over HTTP, as a shell uses it.

use std::sync::Arc;
use std::time::{Duration, Instant};

use sion_app_core::AppCore;
use sion_node::{Devnet, DevnetConfig};

#[test]
fn an_install_verifies_the_chain_pays_and_delegates() {
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
    let paid = app.send("bob", "1.5").unwrap();
    assert_eq!(paid["status"], "Success", "{paid}");
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
