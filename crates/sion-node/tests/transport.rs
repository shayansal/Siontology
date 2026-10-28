//! The transport seam end to end: a devnet whose every message passes
//! through the fault injector at zero faults behaves as one on the plain
//! memory transport, and the protocol's retries carry it through loss.
//!
//! The devnets here sign with the insecure simulation MAC, so they run in
//! seconds; what is compared is what the transports carry, not signatures.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde_json::Value;
use sion_core::ids::{AssetId, Pulse, TxDigest};
use sion_core::Amount;
use sion_exec::{genesis, ExecStatus, SignedTransaction, TxEffects, TxKind, Wallet};
use sion_node::devnet::{devnet_genesis, validator_id, DevnetNetwork};
use sion_node::transport::faulty::{ClassFaults, FaultNet, FaultScript, Faults};
use sion_node::transport::{MessageClass, NodeId};
use sion_node::view::{AccountView, PulseView};
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
        // Idle, these devnets close hundreds of Pulses a second: keep enough
        // of them to inspect.
        history: 20_000,
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

/// Submits `txs` and waits until every one executes successfully.
fn execute_all(devnet: &Devnet, txs: Vec<SignedTransaction>, timeout: Duration) {
    let digests: Vec<TxDigest> = txs
        .into_iter()
        .map(|tx| devnet.submit(tx).unwrap().digest)
        .collect();
    for digest in digests {
        let record = devnet.wait_for(digest, timeout).unwrap();
        assert_eq!(record.status, ExecStatus::Success);
    }
}

fn status(devnet: &Devnet, index: usize) -> Value {
    devnet.query_validator(index, Query::Status).unwrap()
}

fn pulse_of(status: &Value, field: &str) -> u64 {
    status[field].as_u64().unwrap()
}

/// Waits until every validator holds the certificate of every Pulse from
/// `first` to `last`.
fn wait_for_certificates(devnet: &Devnet, first: u64, last: u64, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    for index in 0..4 {
        for pulse in first..=last {
            loop {
                // A validator that is catching up has not executed it yet.
                let certified = devnet
                    .query_validator(index, Query::Pulse(Some(Pulse(pulse))))
                    .ok()
                    .and_then(|value| serde_json::from_value::<PulseView>(value).ok())
                    .is_some_and(|view| view.certificate.is_some());
                if certified {
                    break;
                }
                if Instant::now() >= deadline {
                    let status = status(devnet, index);
                    panic!(
                        "validator {index} never certified Pulse {pulse}: pulse {}, certified {}, round {}, network {}",
                        status["pulse"], status["certified_pulse"], status["round"], status["network"]["kinds"]
                    );
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
}

#[test]
fn lane_requests_lost_at_first_are_asked_again() {
    let _serial = serial();
    // Every lane request and payload is lost for the first two seconds.
    let ingress = Faults {
        classes: [(MessageClass::Ingress, ClassFaults::loss_percent(100))].into(),
        partition: Vec::new(),
    };
    let faults = FaultNet::new(3, ingress);
    let config = config();
    let txs = transfers(&config);
    let devnet = Devnet::start_with(
        config,
        DevnetNetwork {
            encode: false,
            faults: Some(faults.clone()),
        },
    )
    .unwrap();
    let digests: Vec<TxDigest> = txs
        .into_iter()
        .map(|tx| devnet.submit(tx).unwrap().digest)
        .collect();
    let script = faults.run_script(FaultScript {
        steps: vec![(Duration::from_secs(2), Faults::none())],
    });
    script.join().unwrap();
    assert!(
        faults.injected().lost >= 4 * 4,
        "every first request was lost"
    );
    for digest in digests {
        let record = devnet.wait_for(digest, Duration::from_secs(60)).unwrap();
        assert_eq!(record.status, ExecStatus::Success);
    }
    devnet.shutdown();
}

#[test]
fn a_validator_cut_off_for_a_moment_catches_up_and_pulls_the_signatures_it_missed() {
    let _serial = serial();
    let faults = FaultNet::new(5, Faults::none());
    let config = config();
    let node = |index| NodeId::Validator(validator_id(&config.chain_id, index));
    let (isolated, others) = (node(3), [node(0), node(1), node(2)]);
    let devnet = Devnet::start_with(
        config,
        DevnetNetwork {
            encode: false,
            faults: Some(faults.clone()),
        },
    )
    .unwrap();
    devnet
        .wait_for_pulse(Pulse(2), Duration::from_secs(60))
        .unwrap();
    // Validator 3 is cut off while the other three keep executing Pulses:
    // their signatures on those Pulses never reach it.
    let before = pulse_of(&status(&devnet, 0), "pulse");
    faults.set(Faults {
        classes: Default::default(),
        partition: vec![[isolated].into(), others.into()],
    });
    devnet
        .wait_for_pulse(Pulse(before + 2), Duration::from_secs(60))
        .unwrap();
    faults.set(Faults::none());
    let last = pulse_of(&status(&devnet, 0), "pulse");
    // It catches up through the DAG and asks for the signatures it lacks.
    wait_for_certificates(&devnet, before, last, Duration::from_secs(60));
    assert!(faults.injected().partitioned > 0);
    let stats = devnet.transport(3).unwrap().stats();
    let pulled = stats.kinds.get("pulse_signature_request").map(|t| t.sent);
    assert!(
        pulled.unwrap_or_default().messages > 0,
        "the isolated validator asked for the signatures it missed"
    );
    devnet.shutdown();
}

#[test]
fn transfers_execute_and_pulses_certify_through_ten_percent_loss() {
    let _serial = serial();
    // Loss, delay with jitter, duplicates and reordering in every class,
    // between every pair of nodes.
    let faults = FaultNet::new(
        11,
        Faults::uniform(ClassFaults {
            loss_ppm: 100_000,
            duplicate_ppm: 20_000,
            reorder_ppm: 20_000,
            delay_ms: 5,
            jitter_ms: 20,
        }),
    );
    let config = config();
    let txs = transfers(&config);
    let devnet = Arc::new(
        Devnet::start_with(
            config,
            DevnetNetwork {
                encode: false,
                faults: Some(faults.clone()),
            },
        )
        .unwrap(),
    );
    let first = pulse_of(&status(&devnet, 0), "pulse").max(1);
    execute_all(&devnet, txs, Duration::from_secs(90));
    let last = pulse_of(&status(&devnet, 0), "pulse");
    devnet
        .wait_for_pulse(Pulse(last + 2), Duration::from_secs(60))
        .unwrap();
    wait_for_certificates(&devnet, first, last, Duration::from_secs(60));
    let injected = faults.injected();
    assert!(injected.lost > 0 && injected.duplicated > 0 && injected.reordered > 0);
    Arc::try_unwrap(devnet).unwrap().shutdown();
}
