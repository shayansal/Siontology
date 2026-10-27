//! Assembling and running a devnet: validators, gateway and prover in one
//! process, connected by an in-memory network.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use fold_consensus::{sample_committee, Beacon, Committee, HashChain};
use fold_core::hash::{hash_domain, Hash32};
use fold_core::ids::{AccountId, AssetId, Epoch, Pulse, ShardId, TxDigest};
use fold_core::Amount;
use fold_exec::{
    genesis, ChainConfig, GenesisAccount, GenesisSpec, SignedTransaction, State, Wallet,
};
use serde_json::Value;

use crate::config::DevnetConfig;
use crate::error::NodeError;
use crate::gateway::{Gateway, GatewayInput};
use crate::keys::{NodeVerifier, ValidatorKeys};
use crate::prover::{self, ProverHandle};
use crate::storage::{replay, Snapshot, Wal};
use crate::validator::{Bus, Input, Shared, Validator, ValidatorSetup};
use crate::view::{Query, SubmitReceipt, TxStatus, TxView};

/// Length of each validator's beacon hash chain (Pulses it can reveal for).
pub const BEACON_CHAIN_LENGTH: usize = 1 << 17;

/// The name of the devnet faucet wallet.
pub const FAUCET: &str = "faucet";

/// A running devnet.
#[derive(Debug)]
pub struct Devnet {
    /// What every validator shares.
    pub shared: Arc<Shared>,
    bus: Bus,
    threads: Vec<JoinHandle<()>>,
    prover: Option<ProverHandle>,
}

fn beacon_chain(chain_id: &str, index: usize) -> HashChain {
    let seed = hash_domain(
        "fold/devnet/beacon-seed",
        format!("{chain_id}/{index}").as_bytes(),
    );
    HashChain::new(seed, BEACON_CHAIN_LENGTH)
}

/// The genesis specification a devnet configuration produces.
pub fn devnet_genesis(config: &DevnetConfig) -> GenesisSpec {
    let faucet = Wallet::named(FAUCET);
    let mut accounts = vec![GenesisAccount {
        policy: faucet.policy.clone(),
        coins: vec![(AssetId::usd(), Amount::units(1_000_000_000))],
    }];
    for name in &config.demo_accounts {
        let wallet = Wallet::named(name);
        // Several coins so an account can send several transactions per Pulse.
        let coins = (0..4)
            .map(|_| (AssetId::usd(), Amount::units(config.demo_usd / 4)))
            .collect();
        accounts.push(GenesisAccount {
            policy: wallet.policy.clone(),
            coins,
        });
    }
    accounts.extend(config.accounts.iter().cloned());
    let mut chain = ChainConfig::devnet(Some(faucet.id));
    chain.chain_id = config.chain_id.clone();
    chain.shard_count = config.shards.max(1);
    GenesisSpec {
        config: chain,
        accounts,
        atoms: Vec::new(),
    }
}

struct Recovery {
    epoch: u64,
    states: Vec<(State, Beacon, Option<Wal>)>,
}

fn recover(
    config: &DevnetConfig,
    genesis_digest: Hash32,
    genesis_state: &State,
    genesis_beacon: &Beacon,
) -> Result<Recovery, NodeError> {
    let n = config.validators;
    let Some(dir) = &config.data_dir else {
        return Ok(Recovery {
            epoch: 0,
            states: (0..n)
                .map(|_| (genesis_state.clone(), genesis_beacon.clone(), None))
                .collect(),
        });
    };
    fs::create_dir_all(dir)?;
    let marker = dir.join("genesis");
    match fs::read_to_string(&marker) {
        Ok(stored) if stored.trim() != genesis_digest.to_hex() => {
            return Err(NodeError::Config(format!(
                "{} holds another chain (genesis {}); use a fresh directory",
                dir.display(),
                stored.trim()
            )))
        }
        Ok(_) => {}
        Err(_) => fs::write(&marker, genesis_digest.to_hex())?,
    }
    let epoch_file = dir.join("epoch");
    let epoch = match fs::read_to_string(&epoch_file) {
        Ok(text) => text.trim().parse::<u64>().map(|e| e + 1).unwrap_or(0),
        Err(_) => 0,
    };
    fs::write(&epoch_file, epoch.to_string())?;

    let mut states = Vec::with_capacity(n);
    for index in 0..n {
        let (wal, recovered) = Wal::open(&dir.join(format!("validator-{index}")), config.fsync)?;
        let (base, beacon) = match recovered.snapshot {
            Some(snapshot) => (snapshot.state, snapshot.beacon),
            None => (genesis_state.clone(), genesis_beacon.clone()),
        };
        let (state, beacon) = replay(base, beacon, &recovered.records)?;
        states.push((state, beacon, Some(wal)));
    }
    // Validators stop together, but a validator may have logged one Pulse
    // more than another; replicas are deterministic, so the laggards adopt
    // the most advanced state (state sync) and snapshot it.
    let best = (0..n).max_by_key(|i| states[*i].0.pulse).unwrap_or(0);
    let (best_state, best_beacon) = (states[best].0.clone(), states[best].1.clone());
    for (state, beacon, wal) in states.iter_mut() {
        if state.pulse < best_state.pulse {
            *state = best_state.clone();
            *beacon = best_beacon.clone();
            if let Some(wal) = wal {
                wal.snapshot(&Snapshot {
                    state: state.clone(),
                    beacon: beacon.clone(),
                    epoch,
                })?;
            }
        }
    }
    Ok(Recovery { epoch, states })
}

impl Devnet {
    /// Starts a devnet.
    pub fn start(config: DevnetConfig) -> Result<Devnet, NodeError> {
        let n = config.validators;
        if n == 0 {
            return Err(NodeError::Config("at least one validator".into()));
        }
        let mode = config.signatures;
        let keys: Vec<ValidatorKeys> = (0..n)
            .map(|i| ValidatorKeys::devnet(&config.chain_id, i, mode))
            .collect();
        let verifier = NodeVerifier::new(&keys, mode);
        let ids: Vec<_> = keys.iter().map(|k| k.id).collect();

        let genesis_spec = devnet_genesis(&config);
        let genesis_digest = genesis_spec.digest();
        let genesis_state = genesis(&genesis_spec)?;
        let chains: Vec<HashChain> = (0..n).map(|i| beacon_chain(&config.chain_id, i)).collect();
        let tips: Vec<Hash32> = chains.iter().map(HashChain::tip).collect();
        let mut genesis_beacon = Beacon::new(genesis_digest);
        for (id, tip) in ids.iter().zip(&tips) {
            genesis_beacon.register(*id, *tip);
        }
        let recovery = recover(&config, genesis_digest, &genesis_state, &genesis_beacon)?;
        let epoch = recovery.epoch;

        let committee = Committee::equal_weight(Epoch(epoch), ShardId(0), ids.iter().copied())?;
        let weighted: Vec<_> = ids.iter().map(|id| (*id, 1u64)).collect();
        let seats = config.shard_seats.clamp(1, n);
        let mut shard_committees = BTreeMap::new();
        for shard in 0..config.shards.max(1) {
            let sampled = sample_committee(
                &weighted,
                seats,
                &genesis_digest,
                Epoch(epoch),
                ShardId(shard),
            )?;
            shard_committees.insert(ShardId(shard), sampled);
        }

        let shared = Arc::new(Shared {
            config: config.clone(),
            genesis: genesis_spec,
            genesis_digest,
            epoch,
            committee,
            shard_committees,
            verifier,
            validators: ids.clone(),
            beacon_tips: tips,
        });

        let (senders, receivers): (Vec<Sender<Input>>, Vec<_>) =
            (0..n).map(|_| mpsc::channel()).unzip();
        let (gateway_sender, gateway_receiver) = mpsc::channel();
        let bus = Bus {
            validators: senders,
            index: ids.iter().enumerate().map(|(i, id)| (*id, i)).collect(),
            gateway: gateway_sender,
        };

        let proven = Arc::new(Mutex::new(BTreeMap::new()));
        let prover = config
            .prove_chunk
            .map(|_| prover::spawn(shared.clone(), proven.clone()));

        let mut threads = Vec::new();
        for (index, ((receiver, (state, beacon, wal)), chain)) in receivers
            .into_iter()
            .zip(recovery.states)
            .zip(chains)
            .enumerate()
        {
            let setup = ValidatorSetup {
                index,
                auth: keys[index].authenticator(mode),
                state,
                beacon,
                chain,
                wal,
                inbox: receiver,
                bus: bus.clone(),
                prover: if index == 0 {
                    prover.as_ref().map(|p| p.jobs.clone())
                } else {
                    None
                },
                proven: proven.clone(),
            };
            let validator = Validator::new(shared.clone(), setup);
            threads.push(
                thread::Builder::new()
                    .name(format!("validator-{index}"))
                    .spawn(move || validator.run())?,
            );
        }
        let gateway = Gateway::new(
            shared.clone(),
            bus.clone(),
            gateway_receiver,
            Wallet::named(FAUCET),
        );
        threads.push(
            thread::Builder::new()
                .name("gateway".into())
                .spawn(move || gateway.run())?,
        );
        Ok(Devnet {
            shared,
            bus,
            threads,
            prover,
        })
    }

    /// Asks validator `index` a question.
    pub fn query_validator(&self, index: usize, query: Query) -> Result<Value, String> {
        let (reply, answer) = mpsc::channel();
        self.bus
            .validators
            .get(index)
            .ok_or("no such validator")?
            .send(Input::Query(query, reply))
            .map_err(|_| "validator stopped".to_string())?;
        answer
            .recv_timeout(Duration::from_secs(10))
            .map_err(|_| "validator did not answer in time".to_string())?
    }

    /// Asks validator 0.
    pub fn query(&self, query: Query) -> Result<Value, String> {
        self.query_validator(0, query)
    }

    /// Submits a signed transaction.
    pub fn submit(&self, tx: SignedTransaction) -> Result<SubmitReceipt, String> {
        let (reply, answer) = mpsc::channel();
        self.bus
            .gateway
            .send(GatewayInput::Submit {
                tx: Box::new(tx),
                reply,
            })
            .map_err(|_| "gateway stopped".to_string())?;
        answer
            .recv_timeout(Duration::from_secs(10))
            .map_err(|_| "gateway did not answer".to_string())?
    }

    /// Queues faucet grants.
    pub fn faucet(&self, grants: Vec<(AccountId, AssetId, Amount)>) -> Result<(), String> {
        let (reply, answer) = mpsc::channel();
        self.bus
            .gateway
            .send(GatewayInput::Faucet { grants, reply })
            .map_err(|_| "gateway stopped".to_string())?;
        answer
            .recv_timeout(Duration::from_secs(10))
            .map_err(|_| "gateway did not answer".to_string())?
    }

    /// Where a transaction is: executed (with effects and a receipt proof),
    /// or the gateway's view of it.
    pub fn status(&self, digest: TxDigest) -> Result<Option<TxStatus>, String> {
        if let Ok(value) = self.query(Query::Tx(digest)) {
            let view: TxView = serde_json::from_value(value).map_err(|e| e.to_string())?;
            return Ok(Some(TxStatus::Executed(Box::new(view))));
        }
        let (reply, answer) = mpsc::channel();
        self.bus
            .gateway
            .send(GatewayInput::Status { tx: digest, reply })
            .map_err(|_| "gateway stopped".to_string())?;
        answer
            .recv_timeout(Duration::from_secs(10))
            .map_err(|_| "gateway did not answer".to_string())
    }

    /// Waits until a transaction executes or certification fails.
    pub fn wait_for(&self, digest: TxDigest, timeout: Duration) -> Result<TxView, String> {
        let deadline = Instant::now() + timeout;
        loop {
            match self.status(digest)? {
                Some(TxStatus::Executed(view)) => return Ok(*view),
                Some(TxStatus::Rejected(reason)) => return Err(format!("not certified: {reason}")),
                _ => {}
            }
            if Instant::now() > deadline {
                return Err(format!(
                    "transaction {} not executed within {:?}",
                    digest, timeout
                ));
            }
            thread::sleep(Duration::from_millis(20));
        }
    }

    /// The last executed Pulse.
    pub fn pulse(&self) -> Result<Pulse, String> {
        let status = self.query(Query::Status)?;
        serde_json::from_value(status["pulse"].clone()).map_err(|e| e.to_string())
    }

    /// Waits until validator 0 has executed `pulse`.
    pub fn wait_for_pulse(&self, pulse: Pulse, timeout: Duration) -> Result<(), String> {
        let deadline = Instant::now() + timeout;
        while self.pulse()? < pulse {
            if Instant::now() > deadline {
                return Err(format!("Pulse {pulse} not reached within {timeout:?}"));
            }
            thread::sleep(Duration::from_millis(20));
        }
        Ok(())
    }

    /// The faucet wallet (devnet only).
    pub fn faucet_wallet(&self) -> Wallet {
        Wallet::named(FAUCET)
    }

    /// The data directory, if persistent.
    pub fn data_dir(&self) -> Option<&Path> {
        self.shared.config.data_dir.as_deref()
    }

    /// Stops every thread (validators write a final snapshot).
    pub fn shutdown(self) {
        for sender in &self.bus.validators {
            let _ = sender.send(Input::Shutdown);
        }
        let _ = self.bus.gateway.send(GatewayInput::Shutdown);
        if let Some(prover) = &self.prover {
            prover.stop();
        }
        for thread in self.threads {
            let _ = thread.join();
        }
        if let Some(prover) = self.prover {
            prover.join();
        }
    }
}
