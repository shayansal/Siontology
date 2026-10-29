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

#[test]
fn a_burst_larger_than_a_consensus_payload_still_executes() {
    // Dual post-quantum transactions are about 20 KB each, and so is every
    // signed batch header. Forty certified at once are far more than one
    // 128 KiB consensus payload: the gateway must split them into groups
    // that fit, or consensus drops them and they never execute.
    let _serial = serial();
    let mut config = config();
    config.core.max_payload_bytes = 128 * 1024;
    let devnet = Devnet::start(config).unwrap();
    let remote = Remote {
        devnet: &devnet,
        config: devnet.shared.genesis.config.clone(),
    };
    let mut alice = Wallet::named("alice");
    let bob = Wallet::named("bob");
    let mut senders: Vec<Wallet> = (0..40)
        .map(|i| Wallet::named(&format!("burst-{i}")))
        .collect();
    let funding = alice
        .build(
            &remote,
            TxKind::Transfer {
                asset: AssetId::usd(),
                outputs: senders.iter().map(|w| (w.id, Amount::units(5))).collect(),
            },
            Vec::new(),
        )
        .unwrap();
    let funded = devnet.submit(funding).unwrap();
    let record = devnet
        .wait_for(funded.digest, Duration::from_secs(60))
        .unwrap();
    assert_eq!(record.status, ExecStatus::Success);

    let burst: Vec<_> = senders
        .iter_mut()
        .map(|wallet| {
            wallet
                .build(
                    &remote,
                    TxKind::Transfer {
                        asset: AssetId::usd(),
                        outputs: vec![(bob.id, Amount::units(1))],
                    },
                    Vec::new(),
                )
                .unwrap()
        })
        .collect();
    let size: usize = burst.iter().map(|tx| tx.size()).sum();
    assert!(size > 4 * 128 * 1024, "the burst is {size} bytes");
    let digests: Vec<_> = burst
        .into_iter()
        .map(|tx| devnet.submit(tx).unwrap().digest)
        .collect();
    for digest in digests {
        let record = devnet.wait_for(digest, Duration::from_secs(60)).unwrap();
        assert_eq!(record.status, ExecStatus::Success);
    }
    let bob_view: AccountView =
        serde_json::from_value(devnet.query(Query::Account(bob.id)).unwrap()).unwrap();
    assert_eq!(
        bob_view.balances,
        vec![(AssetId::usd(), Amount::units(100_040))]
    );
    devnet.shutdown();
}

#[test]
fn keyless_subagents_pay_in_one_batch_that_is_certified_executed_and_proven() {
    // Spec v1.13: an agent spawns keyless subagents and signs one batch for
    // a payment from each. The gateway hands each shard's committee the
    // members in one message, validators check the batch's signatures once,
    // and the prover replays the members in both proof stacks.
    let _serial = serial();
    let devnet = Devnet::start(config()).unwrap();
    let remote = Remote {
        devnet: &devnet,
        config: devnet.shared.genesis.config.clone(),
    };
    let mut alice = Wallet::named("alice");
    let bob = Wallet::named("bob");
    let wait = |digest| {
        let record = devnet.wait_for(digest, Duration::from_secs(90)).unwrap();
        assert_eq!(record.status, ExecStatus::Success, "{record:?}");
        record
    };
    let caveats = sion_exec::charter::Caveats {
        payees: Some([bob.id].into_iter().collect()),
        max_per_payment: Amount::units(5),
        max_per_window: Amount::units(1_000),
        window: 1_000,
        expires_at: Pulse(100_000),
        evidence_above: None,
        evidence_policy: sion_assurance::Policy::standard(),
        receipts: false,
        may_delegate: true,
    };
    let register = alice
        .build(
            &remote,
            TxKind::RegisterAgent {
                agent: alice.id,
                card: sion_core::hash::Hash32::ZERO,
                caveats: caveats.clone(),
                funding: Amount::units(100),
                agent_bond: Amount::ZERO,
            },
            Vec::new(),
        )
        .unwrap();
    let record = wait(devnet.submit(register).unwrap().digest);
    let (root, root_allotment) = record
        .effects
        .events
        .iter()
        .find_map(|event| match event {
            sion_exec::Event::AgentRegistered {
                agent, allotment, ..
            } => Some((*agent, *allotment)),
            _ => None,
        })
        .unwrap();
    alice.settle();
    let mut child = caveats.clone();
    child.max_per_window = Amount::units(10);
    let spawn = alice
        .build(
            &remote,
            TxKind::SpawnAgents {
                charter: root,
                children: (0..16)
                    .map(|_| sion_exec::charter::Spawn {
                        agent: None,
                        card: sion_core::hash::Hash32::ZERO,
                        caveats: child.clone(),
                        budget: Amount::units(2),
                    })
                    .collect(),
            },
            vec![remote.object(&root_allotment).unwrap().reference()],
        )
        .unwrap();
    let record = wait(devnet.submit(spawn).unwrap().digest);
    let spawned: Vec<_> = record
        .effects
        .events
        .iter()
        .filter_map(|event| match event {
            sion_exec::Event::AgentSpawned {
                agent,
                account,
                allotment,
                ..
            } => Some((*agent, *account, *allotment)),
            _ => None,
        })
        .collect();
    assert_eq!(spawned.len(), 16);
    alice.settle();
    let gas = alice
        .build(
            &remote,
            TxKind::Transfer {
                asset: AssetId::usd(),
                outputs: spawned
                    .iter()
                    .map(|(_, account, _)| (*account, Amount::units(1)))
                    .collect(),
            },
            Vec::new(),
        )
        .unwrap();
    wait(devnet.submit(gas).unwrap().digest);

    // One batch: a payment from each subagent, one authorization.
    let bodies: Vec<_> = spawned
        .iter()
        .map(|(charter, _, allotment)| {
            let mut builder = sion_exec::wallet::Keyless::new(*charter);
            let body = builder
                .draft(
                    &remote,
                    TxKind::AgentPay {
                        charter: *charter,
                        payee: bob.id,
                        amount: Amount::units(1),
                        purpose: "a batched tool call".into(),
                        evidence: None,
                    },
                    vec![remote.object(allotment).unwrap().reference()],
                    16,
                )
                .unwrap();
            (body, Some(*charter))
        })
        .collect();
    let members = alice
        .sign_batch(&devnet.shared.genesis.config, bodies)
        .unwrap();
    let receipts = devnet.submit_batch(members.clone()).unwrap();
    assert_eq!(receipts.len(), 16);
    let mut pulses = std::collections::BTreeSet::new();
    for member in &members {
        let record = wait(member.digest());
        assert!(
            record.effects.fee < Amount(1_000),
            "{:?}",
            record.effects.fee
        );
        pulses.insert(record.pulse);
    }
    let bob_view: AccountView =
        serde_json::from_value(devnet.query(Query::Account(bob.id)).unwrap()).unwrap();
    assert_eq!(
        bob_view.balances,
        vec![(AssetId::usd(), Amount::units(100_016))]
    );
    // The prover folded every Pulse the members executed in.
    for pulse in pulses {
        let mut world: Option<Value> = None;
        for _ in 0..600 {
            if let Ok(value) = devnet.query(Query::World(pulse)) {
                world = Some(value);
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let world = world.expect("the prover folded the Pulse");
        assert!(world.get("error").is_none(), "{world}");
    }
    devnet.shutdown();
}
