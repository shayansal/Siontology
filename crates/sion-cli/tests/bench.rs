//! A benchmark run on a devnet: run with a stand-in model, kept, published
//! as a claim signed by a wallet, and checked against its log and suite.

use std::sync::Arc;

use sion_cli::runtime::SharedLedger;
use sion_cli::session::Session;
use sion_core::ids::AssetId;
use sion_core::Amount;
use sion_node::config::DevnetConfig;
use sion_node::devnet::Devnet;

#[test]
fn a_run_is_published_and_checked_against_its_log() {
    let home = std::env::temp_dir().join(format!("sion-bench-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    // The only test in this binary, so the environment is its own.
    std::env::set_var("SION_HOME", &home);

    // A stand-in model that only says it cannot help.
    let script = home.join("refuses.json");
    std::fs::write(&script, r#"[{"say": "I cannot do this."}]"#).unwrap();
    let models = vec![format!("script:{}", script.display())];
    let (record, at) = sion_cli::bench_cli::run("payments-confirmation", &models, 2, None).unwrap();
    assert_eq!((record.passed(), record.total()), (0, 12));
    assert!(at.join("log.jsonl").is_file() && at.join("record.json").is_file());

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
    session
        .fund("alice", AssetId::usd(), Amount::units(1_000))
        .unwrap();
    let run = at.file_name().unwrap().to_string_lossy().into_owned();
    let claim = sion_cli::bench_cli::publish(&mut session, "alice", &run[..20]).unwrap();
    assert!(at.join("published.json").is_file());

    // Checked with the log kept here and the suite built in.
    let lines = sion_cli::bench_cli::check(ledger.as_ref(), &claim, None, None).unwrap();
    let text = lines.join("\n");
    assert!(
        text.contains("payments-confirmation: 0 of 12 runs passed"),
        "{text}"
    );
    assert!(text.contains("pass^2 0.0%"), "{text}");
    assert!(
        text.contains("checked: the log is the one the claim names"),
        "{text}"
    );
    assert!(
        text.contains(&format!("account {}", session.id("alice"))),
        "{text}"
    );

    // A log changed after it was published does not check.
    let changed = home.join("changed.jsonl");
    let mut bytes = std::fs::read(at.join("log.jsonl")).unwrap();
    let at_byte = bytes.len() / 2;
    bytes[at_byte] ^= 1;
    std::fs::write(&changed, &bytes).unwrap();
    let lines = sion_cli::bench_cli::check(ledger.as_ref(), &claim, Some(&changed), None).unwrap();
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("does not check: the log's digest")),
        "{lines:?}"
    );

    // Without the log, the claim is shown, unchecked.
    std::fs::remove_dir_all(&at).unwrap();
    let lines = sion_cli::bench_cli::check(ledger.as_ref(), &claim, None, None).unwrap();
    assert!(lines.last().unwrap().contains("unchecked"), "{lines:?}");

    let _ = std::fs::remove_dir_all(&home);
}
