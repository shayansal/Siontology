//! A real devnet: four validators with dual post-quantum signatures, lane
//! certification, certified-DAG consensus, Pulse execution, header
//! certificates, data availability, the prover and storage.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use serde_json::Value;
use sion_core::ids::{AssetId, Pulse};
use sion_core::Amount;
use sion_exec::claim::{formulas, ClaimBody, Payload};
use sion_exec::{ExecStatus, StateView, TxKind, Wallet};
use sion_light::{
    verify_certificate, verify_object, verify_receipt, ObjectProof, PulseCertificate, ValidatorSet,
};
use sion_node::view::{AccountView, PulseView};
use sion_node::{Devnet, DevnetConfig, Query};

/// A wallet's view of the devnet through queries (what a remote wallet does
/// over HTTP).
struct Remote<'a> {
    devnet: &'a Devnet,
    config: sion_exec::ChainConfig,
}

impl StateView for Remote<'_> {
    fn config(&self) -> &sion_exec::ChainConfig {
        &self.config
    }
    fn current_pulse(&self) -> Pulse {
        self.devnet.pulse().unwrap()
    }
    fn object(&self, id: &sion_core::ids::ObjectId) -> Option<sion_exec::Object> {
        let value = self.devnet.query(Query::Object(*id)).ok()?;
        serde_json::from_value(value["proof"]["object"].clone()).ok()
    }
    fn coins(&self, account: &sion_core::ids::AccountId) -> Vec<sion_exec::Object> {
        let value = self.devnet.query(Query::Account(*account)).unwrap();
        let view: AccountView = serde_json::from_value(value).unwrap();
        view.coins
    }
}

/// The tests run one devnet at a time: each runs four validator threads with
/// post-quantum signatures, a gateway and a prover, and a restart replays
/// every log, so two at once starve each other on a small machine.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn config() -> DevnetConfig {
    DevnetConfig {
        demo_accounts: vec!["alice".into(), "bob".into()],
        prove_chunk: Some(4),
        ..DevnetConfig::default()
    }
}

#[test]
fn a_transfer_is_certified_ordered_executed_proven_and_light_verified() {
    let _serial = serial();
    let devnet = Devnet::start(config()).unwrap();
    let remote = Remote {
        devnet: &devnet,
        config: devnet.shared.genesis.config.clone(),
    };
    let mut alice = Wallet::named("alice");
    let bob = Wallet::named("bob");
    let tx = alice
        .build(
            &remote,
            TxKind::Transfer {
                asset: AssetId::usd(),
                outputs: vec![(bob.id, Amount::units(250))],
            },
            Vec::new(),
        )
        .unwrap();
    let receipt = devnet.submit(tx.clone()).unwrap();
    assert_eq!(receipt.route, "lane");
    let record = devnet
        .wait_for(receipt.digest, Duration::from_secs(60))
        .unwrap();
    assert_eq!(record.status, ExecStatus::Success);
    let bob_view: AccountView =
        serde_json::from_value(devnet.query(Query::Account(bob.id)).unwrap()).unwrap();
    assert_eq!(
        bob_view.balances,
        vec![(AssetId::usd(), Amount::units(100_250))]
    );

    // A light client checks the Pulse certificate with the validators' keys,
    // the receipt against the header, and Bob's coin against the state root.
    devnet
        .wait_for_pulse(Pulse(record.pulse.0 + 1), Duration::from_secs(30))
        .unwrap();
    let validators = devnet.query(Query::Validators).unwrap();
    let keys: ValidatorSet = serde_json::from_value(validators["keys"].clone()).unwrap();
    let committee: sion_consensus::Committee =
        serde_json::from_value(validators["committee"].clone()).unwrap();
    let view: PulseView =
        serde_json::from_value(devnet.query(Query::Pulse(Some(record.pulse))).unwrap()).unwrap();
    let certificate: PulseCertificate = view.certificate.expect("certified by a quorum");
    verify_certificate(&certificate, &committee, &keys).unwrap();
    verify_receipt(&certificate.header, &record.receipt).unwrap();

    let coin_id = bob_view
        .coins
        .iter()
        .find(|c| c.as_coin().unwrap().amount == Amount::units(250))
        .unwrap()
        .id;
    let latest: PulseView =
        serde_json::from_value(devnet.query(Query::Pulse(None)).unwrap()).unwrap();
    let proof_value = devnet.query(Query::Object(coin_id)).unwrap();
    let proof: ObjectProof = serde_json::from_value(proof_value["proof"].clone()).unwrap();
    let at: Pulse = serde_json::from_value(proof_value["pulse"].clone()).unwrap();
    if at == latest.header.pulse {
        verify_object(&latest.header, &proof).unwrap();
    }

    // The World Proof of the Pulse exists and verified.
    let mut world: Option<Value> = None;
    for _ in 0..300 {
        if let Ok(value) = devnet.query(Query::World(record.pulse)) {
            world = Some(value);
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let world = world.expect("the prover folded the Pulse");
    assert!(world.get("error").is_none(), "{world}");
    assert_eq!(world["simulated"], Value::Bool(true));

    // Data availability: sample the Pulse's square against its certified root.
    let da: sion_da::DaHeader =
        serde_json::from_value(devnet.query(Query::Data(record.pulse)).unwrap()).unwrap();
    let report =
        sion_light::availability::sample(&certificate.header, &da, 12, 7, |row, column| {
            let value = devnet
                .query(Query::Share {
                    pulse: record.pulse,
                    row,
                    column,
                })
                .ok()?;
            serde_json::from_value(value).ok()
        })
        .unwrap();
    assert_eq!(report.verified, 12);
    devnet.shutdown();
}

#[test]
fn contended_and_knowledge_transactions_flow_and_state_survives_restarts() {
    let _serial = serial();
    let dir = std::env::temp_dir().join(format!("sion-devnet-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let config = DevnetConfig {
        data_dir: Some(dir.clone()),
        snapshot_every: 3,
        prove_chunk: None,
        ..config()
    };
    let devnet = Arc::new(Devnet::start(config.clone()).unwrap());
    let remote = Remote {
        devnet: &devnet,
        config: devnet.shared.genesis.config.clone(),
    };
    let mut alice = Wallet::named("alice");
    // Knowledge: a claim (no consensus needed, applied at Pulse end).
    let claim = alice
        .build(
            &remote,
            TxKind::IssueClaim {
                claim: ClaimBody {
                    schema: "test.note/v1".into(),
                    subject: "devnet works".into(),
                    payload: Payload::Public(b"hello".to_vec()),
                    formula: formulas::dual_signature(),
                    adjudicator: None,
                    detection_permille: 1000,
                    valid_for: 1_000,
                    challenge_period: 10,
                },
            },
            Vec::new(),
        )
        .unwrap();
    // Contended: a market (creation is owned), then nothing; a Release on an
    // unknown reservation goes through consensus and fails with the fee.
    let release = alice
        .build(
            &remote,
            TxKind::Release {
                reservation: sion_core::hash::hash_domain("test", b"none"),
            },
            Vec::new(),
        )
        .unwrap();
    let r1 = devnet.submit(claim).unwrap();
    let r2 = devnet.submit(release).unwrap();
    assert_eq!(r2.route, "consensus");
    let claim_record = devnet.wait_for(r1.digest, Duration::from_secs(60)).unwrap();
    assert_eq!(claim_record.status, ExecStatus::Success);
    let release_record = devnet.wait_for(r2.digest, Duration::from_secs(60)).unwrap();
    assert!(matches!(release_record.status, ExecStatus::Failed(_)));

    // Faucet.
    let carol = Wallet::named("carol");
    devnet
        .faucet(vec![(carol.id, AssetId::usd(), Amount::units(42))])
        .unwrap();
    let mut funded = false;
    for _ in 0..600 {
        let view: AccountView =
            serde_json::from_value(devnet.query(Query::Account(carol.id)).unwrap()).unwrap();
        if view.balances == vec![(AssetId::usd(), Amount::units(42))] {
            funded = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(funded, "the faucet paid carol");

    let status: Value = devnet.query(Query::Status).unwrap();
    let before: sion_exec::WorldCommitment =
        serde_json::from_value(status["commitment"].clone()).unwrap();
    let (epoch, session) = (status["epoch"].clone(), status["session"].as_u64().unwrap());
    let devnet = Arc::try_unwrap(devnet).unwrap();
    devnet.shutdown();

    // Restart: the state is recovered and verified by replay, and the chain
    // continues in the same state epoch with a new consensus session (round
    // numbers start again, so they must not collide with the old DAG's).
    let devnet = Devnet::start(config).unwrap();
    let status: Value = devnet.query(Query::Status).unwrap();
    let after: sion_exec::WorldCommitment =
        serde_json::from_value(status["commitment"].clone()).unwrap();
    assert!(after.pulse >= before.pulse);
    assert_eq!(status["epoch"], epoch);
    assert!(status["session"].as_u64().unwrap() > session);
    let view: AccountView =
        serde_json::from_value(devnet.query(Query::Account(carol.id)).unwrap()).unwrap();
    assert_eq!(view.balances, vec![(AssetId::usd(), Amount::units(42))]);
    devnet
        .wait_for_pulse(Pulse(after.pulse.0 + 2), Duration::from_secs(60))
        .unwrap();
    devnet.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}
