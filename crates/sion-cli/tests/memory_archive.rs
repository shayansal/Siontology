//! A memory log's archive on a devnet: encrypted for its writer, named on
//! the ledger as the log's manifest, kept by a storage provider under a
//! deal, retrieved only by the deal's client, and restored only if the
//! ledger vouches for it.

use std::sync::Arc;

use sion_cli::archive;
use sion_cli::memory::{self, Remember};
use sion_cli::runtime::SharedLedger;
use sion_cli::session::Session;
use sion_cli::storage::{self, DealTerms, Provider, ProviderPolicy, Store};
use sion_core::ids::AssetId;
use sion_core::Amount;
use sion_exec::storage::DealState;
use sion_memory::{LogStore, MemoryKind, Value};
use sion_node::{Devnet, DevnetConfig};

fn remember(
    session: &mut Session<'_>,
    ledger: &SharedLedger,
    log: &sion_core::ids::ObjectId,
    subject: &str,
    value: &str,
) {
    memory::remember(
        session,
        ledger.as_ref(),
        "alice",
        log,
        Remember {
            subject: subject.into(),
            predicate: "terms".into(),
            value: Value::Text(value.into()),
            kind: MemoryKind::Semantic,
            place: None,
            from_s: None,
            seconds: None,
            confidence: 1000,
        },
    )
    .unwrap();
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

#[test]
fn a_memory_log_is_archived_with_a_provider_and_restored() {
    let home = std::env::temp_dir().join(format!("sion-memory-archive-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    // The only test in this binary, so the environment is its own.
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
    for name in ["alice", "bob", "mallory"] {
        session
            .fund(name, AssetId::usd(), Amount::units(100))
            .unwrap();
    }
    let (_, log) = memory::open(&mut session, "alice", None).unwrap();
    let log = log.unwrap();
    for (subject, value) in [
        ("acme", "net 30"),
        ("globex", "net 45"),
        ("initech", "net 60"),
    ] {
        remember(&mut session, &ledger, &log, subject, value);
    }

    // Archiving commits the three atoms, then names the archive.
    let made = archive::make(&mut session, ledger.as_ref(), "alice", &log, None).unwrap();
    assert_eq!(made.header.size, 3);
    assert_eq!(made.receipts.len(), 2, "{:?}", made.receipts);
    let head = memory::committed(ledger.as_ref(), &log).unwrap();
    assert_eq!((head.size, head.manifest), (3, made.tree.root()));
    let bytes = std::fs::read(&made.path).unwrap();
    assert!(
        !String::from_utf8_lossy(&bytes).contains("net 30"),
        "the archive is encrypted"
    );

    // A provider on this machine keeps it under a deal.
    let terms = DealTerms {
        price: "0.05".into(),
        rounds: 20,
        round_length: 8,
        collateral: None,
        challenges: 4,
        provider: Some("bob".into()),
        accept_within: 200,
        locator: None,
        local: true,
    };
    let (_, deal) = storage::post_deal(
        &mut session,
        ledger.as_ref(),
        "alice",
        &made.path,
        &bytes,
        &terms,
    )
    .unwrap();
    let deal = deal.unwrap();
    let store = Store::new(storage::storage_dir().join("bob"));
    let mut bob = Provider::new(
        "bob",
        store.clone(),
        ProviderPolicy {
            allow_local: true,
            ..ProviderPolicy::default()
        },
    );
    for _ in 0..400 {
        bob.tick(&mut session, ledger.as_ref()).unwrap();
        if matches!(
            storage::deal(ledger.as_ref(), deal).unwrap().state,
            DealState::Active { .. }
        ) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(matches!(
        storage::deal(ledger.as_ref(), deal).unwrap().state,
        DealState::Active { .. }
    ));
    assert_eq!(
        archive::deal_for(ledger.as_ref(), &session.id("alice"), &made.tree.root()).unwrap(),
        deal
    );
    let port = free_port();
    let listen = format!("127.0.0.1:{port}");
    let base = format!("http://{listen}");
    storage::serve_retrievals(&listen, ledger.clone(), store, session.id("bob")).unwrap();

    // Only the deal's client may retrieve it.
    let refused =
        storage::retrieve(&mut session, ledger.as_ref(), "mallory", &base, deal).unwrap_err();
    assert!(
        refused.to_string().contains("another client's"),
        "{refused}"
    );
    let back = storage::retrieve(&mut session, ledger.as_ref(), "alice", &base, deal).unwrap();
    assert_eq!(back, bytes);
    // The provider checks a request against the deal's client on the
    // ledger: mallory's signature on alice's deal does not pass, alice's does.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let request = |session: &mut Session<'_>, who: &str, expires: u64| {
        let wallet = session.wallet(who);
        serde_json::json!({
            "expires": expires,
            "policy": hex::encode(sion_core::encode::encode(&wallet.policy)),
            "authorization": hex::encode(sion_core::encode::encode(
                &wallet.keys.authorize(storage::RETRIEVE_DOMAIN, &storage::retrieve_message(&deal, expires)).unwrap()
            )),
        })
    };
    let forged = request(&mut session, "mallory", now + 60);
    let refused = storage::check_retrieval(ledger.as_ref(), &deal, &forged, now).unwrap_err();
    assert!(
        refused.contains("not signed by the deal's client"),
        "{refused}"
    );
    let signed = request(&mut session, "alice", now + 60);
    assert!(storage::check_retrieval(ledger.as_ref(), &deal, &signed, now).is_ok());
    let late = storage::check_retrieval(ledger.as_ref(), &deal, &signed, now + 61).unwrap_err();
    assert!(late.contains("expires"), "{late}");
    let early = request(
        &mut session,
        "alice",
        now + storage::MAX_REQUEST_SECONDS + 1,
    );
    assert!(storage::check_retrieval(ledger.as_ref(), &deal, &early, now).is_err());

    // The log grows; the manifest is carried forward.
    remember(&mut session, &ledger, &log, "umbrella", "net 90");
    memory::commit(&mut session, ledger.as_ref(), "alice", &log, false).unwrap();
    let head = memory::committed(ledger.as_ref(), &log).unwrap();
    assert_eq!((head.size, head.manifest), (4, made.tree.root()));

    // The store is lost; it comes back from the provider, checked.
    let dir = memory::store_dir(&log);
    std::fs::remove_dir_all(&dir).unwrap();
    let restored =
        archive::restore(&mut session, ledger.as_ref(), "alice", &log, &back, false).unwrap();
    assert_eq!(restored["restored_atoms"], 3, "{restored}");
    assert!(
        restored["checked"]
            .as_str()
            .unwrap()
            .contains("committed at Pulse"),
        "{restored}"
    );
    assert!(
        restored["missing"]
            .as_str()
            .unwrap()
            .contains("1 atom the archive does not"),
        "{restored}"
    );
    assert_eq!(LogStore::open(&dir).unwrap().log().len(), 3);
    // Over a store there is, only when asked.
    let again =
        archive::restore(&mut session, ledger.as_ref(), "alice", &log, &back, false).unwrap_err();
    assert!(again.to_string().contains("--replace"), "{again}");

    // Another wallet cannot open it; an altered copy is not the manifest.
    let other =
        archive::restore(&mut session, ledger.as_ref(), "mallory", &log, &back, true).unwrap_err();
    assert!(
        other.to_string().contains("written by another account"),
        "{other}"
    );
    let mut altered = back.clone();
    let last = altered.len() - 1;
    altered[last] ^= 1;
    let changed =
        archive::restore(&mut session, ledger.as_ref(), "alice", &log, &altered, true).unwrap_err();
    assert!(
        changed.to_string().contains("is not the manifest"),
        "{changed}"
    );

    // A newer archive moves the manifest: the older one is no longer it.
    let (_, second) = memory::open(&mut session, "alice", None).unwrap();
    let second = second.unwrap();
    remember(&mut session, &ledger, &second, "acme", "net 30");
    let a1 = home.join("a1.bin");
    let older = archive::make(
        &mut session,
        ledger.as_ref(),
        "alice",
        &second,
        Some(a1.as_path()),
    )
    .unwrap();
    remember(&mut session, &ledger, &second, "globex", "net 45");
    let a2 = home.join("a2.bin");
    let newer = archive::make(
        &mut session,
        ledger.as_ref(),
        "alice",
        &second,
        Some(a2.as_path()),
    )
    .unwrap();
    assert_ne!(older.tree.root(), newer.tree.root());
    std::fs::remove_dir_all(memory::store_dir(&second)).unwrap();
    let stale = archive::restore(
        &mut session,
        ledger.as_ref(),
        "alice",
        &second,
        &std::fs::read(&a1).unwrap(),
        false,
    )
    .unwrap_err();
    assert!(stale.to_string().contains("is not the manifest"), "{stale}");
    let fresh = archive::restore(
        &mut session,
        ledger.as_ref(),
        "alice",
        &second,
        &std::fs::read(&a2).unwrap(),
        false,
    )
    .unwrap();
    assert_eq!(fresh["restored_atoms"], 2, "{fresh}");
    assert!(fresh.get("missing").is_none(), "{fresh}");
    // An archive of one log is not another's.
    let crossed = archive::restore(
        &mut session,
        ledger.as_ref(),
        "alice",
        &log,
        &std::fs::read(&a2).unwrap(),
        true,
    )
    .unwrap_err();
    assert!(
        crossed.to_string().contains("is not the manifest"),
        "{crossed}"
    );
    std::fs::remove_dir_all(&home).unwrap();
}
