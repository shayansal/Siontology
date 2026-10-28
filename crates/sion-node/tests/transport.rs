//! The transport seam end to end: a devnet whose every message passes
//! through the fault injector at zero faults behaves as one on the plain
//! memory transport.
//!
//! The devnets here sign with the insecure simulation MAC, so they run in
//! seconds; what is compared is what the transports carry, not signatures.

use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use sion_core::ids::{AssetId, TxDigest};
use sion_core::Amount;
use sion_exec::{genesis, ExecStatus, SignedTransaction, TxEffects, TxKind, Wallet};
use sion_node::devnet::{devnet_genesis, DevnetNetwork};
use sion_node::transport::faulty::{FaultNet, Faults};
use sion_node::view::AccountView;
use sion_node::{Devnet, DevnetConfig, Query, SignatureMode};

/// One devnet at a time.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

const ACCOUNTS: [&str; 3] = ["alice", "bob", "carol"];

fn config() -> DevnetConfig {
    DevnetConfig {
        demo_accounts: ACCOUNTS.iter().map(|a| a.to_string()).collect(),
        signatures: SignatureMode::InsecureFast,
        prove_chunk: None,
        ..DevnetConfig::default()
    }
}

/// Transfers built against the genesis state, so every run submits exactly
/// the same transactions.
fn transfers(config: &DevnetConfig) -> Vec<SignedTransaction> {
    let state = genesis(&devnet_genesis(config)).unwrap();
    let id = |name: &str| Wallet::named(name).id;
    let mut alice = Wallet::named("alice");
    let mut bob = Wallet::named("bob");
    alice.validity = 10_000;
    bob.validity = 10_000;
    let transfer = |wallet: &mut Wallet, to: &str, units: u64| {
        wallet
            .build(
                &state,
                TxKind::Transfer {
                    asset: AssetId::usd(),
                    outputs: vec![(id(to), Amount::units(units))],
                },
                Vec::new(),
            )
            .unwrap()
    };
    vec![
        transfer(&mut alice, "bob", 10),
        transfer(&mut alice, "carol", 11),
        transfer(&mut bob, "carol", 12),
        transfer(&mut alice, "bob", 13),
    ]
}

/// What a run produced.
#[derive(Debug, PartialEq)]
struct Outcome {
    /// Each transaction's status and effects, in submission order.
    executed: Vec<(ExecStatus, TxEffects)>,
    /// Every account's balances afterwards.
    balances: Vec<Vec<(AssetId, Amount)>>,
}

fn run(network: DevnetNetwork, transport: &str) -> Outcome {
    let config = config();
    let txs = transfers(&config);
    let devnet = Devnet::start_with(config, network).unwrap();
    let digests: Vec<TxDigest> = txs
        .into_iter()
        .map(|tx| devnet.submit(tx).unwrap().digest)
        .collect();
    let executed = digests
        .iter()
        .map(|digest| {
            let record = devnet.wait_for(*digest, Duration::from_secs(60)).unwrap();
            (record.status, record.effects)
        })
        .collect();
    let balances = ACCOUNTS
        .iter()
        .map(|name| {
            let value = devnet
                .query(Query::Account(Wallet::named(name).id))
                .unwrap();
            serde_json::from_value::<AccountView>(value)
                .unwrap()
                .balances
        })
        .collect();
    // Every validator's transport carried everything it was handed.
    for index in 0..4 {
        let stats = devnet.transport(index).unwrap().stats();
        assert_eq!(stats.transport, transport);
        assert_eq!(stats.dropped(), 0, "validator {index} dropped messages");
        assert!(stats.sent().messages > 0 && stats.received().messages > 0);
    }
    devnet.shutdown();
    Outcome { executed, balances }
}

#[test]
fn a_devnet_through_the_fault_injector_at_zero_faults_matches_the_memory_transport() {
    let _serial = serial();
    // The memory transport, encoding and decoding every message: whatever
    // the protocol sends fits its class and round-trips.
    let memory = run(
        DevnetNetwork {
            encode: true,
            faults: None,
        },
        "memory",
    );
    assert!(memory
        .executed
        .iter()
        .all(|(status, _)| *status == ExecStatus::Success));
    let faults = FaultNet::new(7, Faults::none());
    let faulty = run(
        DevnetNetwork {
            encode: false,
            faults: Some(faults.clone()),
        },
        "memory+faults",
    );
    assert_eq!(faulty, memory);
    assert_eq!(faults.injected(), Default::default());
}
