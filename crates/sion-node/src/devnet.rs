//! Assembling and running a devnet: validators, gateway and prover in one
//! process, connected by an in-memory network.
//!
//! Every validator and the gateway has an endpoint on one
//! [`MemoryNetwork`]: whatever they send each other goes through the
//! [`Transport`] seam, as it would between machines. What stays in the
//! process (queries, the adversary simulation, shutdown, the API's
//! submissions) travels over local control channels.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde_json::Value;
use sion_consensus::{Beacon, HashChain, LaneRequest};
use sion_core::hash::{hash_domain, Hash32};
use sion_core::ids::{AccountId, AssetId, Pulse, ShardId, TxDigest, ValidatorId};
use sion_core::Amount;
use sion_exec::genesis::GenesisValidator;
use sion_exec::validators::{
    possession_digest, weight, Equivocation, ForcedParams, ValidatorParams,
};
use sion_exec::{
    genesis, ChainConfig, GenesisAccount, GenesisSpec, SignedTransaction, State, TxKind, Wallet,
};
use sion_light::ChainSpec;

use crate::committees::CommitteeView;
use crate::config::DevnetConfig;
use crate::error::NodeError;
use crate::gateway::{Gateway, GatewayInput, Querier};
use crate::keys::{NodeVerifier, SignatureMode, ValidatorKeys};
use crate::prover::{self, ProverHandle};
use crate::pulses::Carry;
use crate::storage::{replay, LaneLog, Snapshot, Wal};
use crate::transport::faulty::FaultNet;
use crate::transport::memory::{MemoryNetwork, MemoryTransport};
use crate::transport::{ClassLimits, Delivery, NodeId, Transport};
use crate::validator::{ExecutedReports, Input, Shared, Validator, ValidatorSetup};
use crate::view::{Query, SubmitReceipt, TxStatus, TxView};

/// How long a devnet API call waits for a validator or the gateway. Long
/// enough for a loaded machine: validators answer between protocol
/// messages, and a busy one can take seconds.
pub const API_TIMEOUT: Duration = Duration::from_secs(30);

/// Length of each validator's beacon hash chain (Pulses it can reveal for).
pub const BEACON_CHAIN_LENGTH: usize = 1 << 17;

/// The name of the devnet faucet wallet.
pub const FAUCET: &str = "faucet";

/// How a devnet's nodes reach each other.
#[derive(Clone, Debug, Default)]
pub struct DevnetNetwork {
    /// Encode and decode every message on its way (slower): checks that
    /// everything the protocol sends round-trips through its canonical
    /// encoding within its class's size limit.
    pub encode: bool,
    /// Pass every message through this fault injector (seeded loss, delay,
    /// duplication, reordering and partitions).
    pub faults: Option<Arc<FaultNet>>,
}

impl DevnetNetwork {
    fn wrap(&self, endpoint: MemoryTransport) -> Arc<dyn Transport> {
        match &self.faults {
            Some(faults) => Arc::new(faults.wrap(endpoint)),
            None => Arc::new(endpoint),
        }
    }
}

/// The local control channels: queries, the adversary simulation,
/// shutdown, and what the API hands the validators and the gateway.
#[derive(Clone, Debug)]
struct Control {
    validators: Vec<Sender<Input>>,
    gateway: Sender<GatewayInput>,
}

/// A running devnet.
#[derive(Debug)]
pub struct Devnet {
    /// What every validator shares.
    pub shared: Arc<Shared>,
    control: Control,
    network: Arc<MemoryNetwork>,
    transports: Vec<Arc<dyn Transport>>,
    threads: Vec<JoinHandle<()>>,
    prover: Option<ProverHandle>,
}

/// The devnet gateway's node identity on chain `chain_id`.
pub fn gateway_node(chain_id: &str) -> NodeId {
    NodeId::Node(hash_domain("sion/devnet/gateway", chain_id.as_bytes()))
}

/// The class limits of a devnet: its payload limits, and committees of up
/// to all of its validators (at least the 100 the active set allows).
pub fn class_limits(config: &DevnetConfig) -> ClassLimits {
    ClassLimits::for_protocol(
        config.core.max_payload_bytes,
        config.core.max_items_per_header,
        (config.validators + config.standby).max(100),
    )
}

/// A query answered by a validator thread of this process.
pub(crate) fn local_querier(validator: Sender<Input>, timeout: Duration) -> Querier {
    Box::new(move |query| {
        let (reply, answer) = mpsc::channel();
        validator
            .send(Input::Query(query, reply))
            .map_err(|_| "validator stopped".to_string())?;
        answer
            .recv_timeout(timeout)
            .map_err(|_| "validator did not answer".to_string())?
    })
}

/// Validator `index`'s beacon hash chain.
pub fn beacon_chain(chain_id: &str, index: usize) -> HashChain {
    let seed = hash_domain(
        "sion/devnet/beacon-seed",
        format!("{chain_id}/{index}").as_bytes(),
    );
    HashChain::new(seed, BEACON_CHAIN_LENGTH)
}

/// The wallet that operates devnet validator `index` (stakes for it,
/// receives its rewards).
pub fn operator_name(index: usize) -> String {
    format!("operator-{index}")
}

/// What devnet validator `index`'s operator signs to stake `stake` for it
/// on the chain `config` describes: the validator's public keys, the tip of
/// its beacon hash chain and a proof that it holds both keys. On a network
/// the operator builds this on its own machine from its validator's keys; a
/// devnet derives every validator's keys from the chain identifier, so
/// anyone can.
pub fn registration(
    config: &ChainConfig,
    index: usize,
    operator: &AccountId,
    stake: Amount,
) -> TxKind {
    let keys = ValidatorKeys::devnet(&config.chain_id, index);
    let (lattice, hash) = keys.public();
    let message = possession_digest(config, operator, &keys.id);
    TxKind::RegisterValidator {
        lattice,
        hash,
        beacon_tip: beacon_chain(&config.chain_id, index).tip(),
        stake,
        possession: keys.sign_dual(&message.0),
    }
}

/// The identifier of devnet validator `index` of the chain `chain_id`.
pub fn validator_id(chain_id: &str, index: usize) -> ValidatorId {
    ValidatorKeys::devnet(chain_id, index).id
}

/// ADVERSARY SIMULATION: what devnet validator `index` leaves behind when
/// it equivocates — two different consensus headers for the same round of
/// consensus session `session` on chain `chain`, both signed with its
/// keys. Anyone who sees both can report them.
pub fn equivocation(
    chain_id: &str,
    chain: Hash32,
    index: usize,
    session: u64,
    round: u64,
) -> Equivocation {
    let keys = ValidatorKeys::devnet(chain_id, index);
    let signed = |payload: &[u8]| {
        let header = sion_consensus::Header {
            author: keys.id,
            chain,
            round: sion_core::ids::Round(round),
            epoch: sion_core::ids::Epoch(session),
            shard: ShardId(0),
            parents: Vec::new(),
            payload: vec![payload.to_vec()],
        };
        let signature = keys.sign_dual(&sion_consensus::Header::signing_digest(&header.digest()).0);
        sion_consensus::SignedHeader { header, signature }
    };
    Equivocation::Headers {
        first: signed(b"one history"),
        second: signed(b"another history"),
    }
}

/// The genesis specification a devnet configuration produces: its genesis
/// validators (the first `validators` devnet validators, each staking
/// `validator_stake` USD, operated by `operator-<index>`), accounts and
/// parameters.
pub fn devnet_genesis(config: &DevnetConfig) -> GenesisSpec {
    let validators = (0..config.validators)
        .map(|index| {
            let keys = ValidatorKeys::devnet(&config.chain_id, index);
            let (lattice, hash) = keys.public();
            GenesisValidator {
                id: keys.id,
                lattice,
                hash,
                operator: Wallet::named(&operator_name(index)).id,
                stake: Amount::units(config.validator_stake),
                beacon_tip: beacon_chain(&config.chain_id, index).tip(),
            }
        })
        .collect();
    genesis_spec(config, validators, true)
}

/// The genesis specification of a chain run with `config` and these genesis
/// validators: the named demo accounts and extra accounts of `config`, the
/// stake, epoch and forced-inclusion parameters, and, if `faucet`, the
/// devnet faucet (whose keys anyone can derive from its name).
pub fn genesis_spec(
    config: &DevnetConfig,
    validators: Vec<GenesisValidator>,
    faucet: bool,
) -> GenesisSpec {
    let faucet = faucet.then(|| Wallet::named(FAUCET));
    let mut accounts = Vec::new();
    if let Some(faucet) = &faucet {
        accounts.push(GenesisAccount {
            policy: faucet.policy.clone(),
            coins: vec![(AssetId::usd(), Amount::units(1_000_000_000))],
        });
    }
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
    let mut chain = ChainConfig::devnet(faucet.as_ref().map(|f| f.id));
    chain.chain_id = config.chain_id.clone();
    chain.shard_count = config.shards.max(1);
    GenesisSpec {
        config: chain,
        accounts,
        atoms: Vec::new(),
        validators,
        validator_params: ValidatorParams {
            stake_asset: AssetId::usd(),
            epoch_length: if config.epoch_length == 0 {
                u64::MAX / 2
            } else {
                config.epoch_length
            },
            min_stake: Amount::units(config.min_stake),
            unbonding: config.unbonding,
            seats: u16::try_from(config.shard_seats.max(1)).unwrap_or(u16::MAX),
            max_active: 100,
            // As the token design proposes: 70 % of the fees the treasury
            // collects in an epoch goes to its validators; a reporter
            // receives 5 % of a slashed stake, so reporting oneself still
            // forfeits 95 %.
            reward_permille: 700,
            reporter_permille: 50,
        },
        forced: ForcedParams {
            window: config.forced_window.max(1),
            max_pending: 4_096,
        },
    }
}

/// What fixes who certifies a chain, which light clients pin: the genesis
/// (by its digest), its validators with their stake and keys, and the shard
/// layout of `config`.
pub fn chain_spec(
    config: &DevnetConfig,
    spec: &GenesisSpec,
    genesis_digest: Hash32,
) -> Result<ChainSpec, NodeError> {
    let mut keys = sion_light::ValidatorSet::default();
    for validator in &spec.validators {
        keys.keys.insert(
            validator.id,
            (validator.lattice.clone(), validator.hash.clone()),
        );
    }
    let chain = ChainSpec {
        genesis: genesis_digest,
        validators: spec
            .validators
            .iter()
            .map(|v| (v.id, weight(v.stake)))
            .collect(),
        keys,
        shards: config.shards.max(1),
        seats: u16::try_from(config.shard_seats.clamp(1, spec.validators.len().max(1)))
            .unwrap_or(u16::MAX),
        insecure_signatures: config.signatures == SignatureMode::InsecureFast,
    };
    chain
        .check()
        .map_err(|e| NodeError::Config(e.to_string()))?;
    Ok(chain)
}

/// The beacon a chain starts from: every genesis validator registered with
/// the tip of its hash chain.
pub fn genesis_beacon(spec: &GenesisSpec, genesis_digest: Hash32) -> Beacon {
    let mut beacon = Beacon::new(genesis_digest);
    for validator in &spec.validators {
        beacon.register(validator.id, validator.beacon_tip);
    }
    beacon
}

/// A validator's lane log and the live lane locks restored from it.
type RecoveredLanes = (Option<LaneLog>, Vec<(ShardId, LaneRequest)>);

/// A validator's recovered state, Pulse-builder carry (beacon and waiting
/// reveals), owed forced intents and log.
type RecoveredState = (State, Carry, Vec<SignedTransaction>, Option<Wal>);

struct Recovery {
    generation: u64,
    states: Vec<RecoveredState>,
    lanes: Vec<RecoveredLanes>,
}

fn recover(
    config: &DevnetConfig,
    genesis_digest: Hash32,
    genesis_state: &State,
    genesis_beacon: &Beacon,
) -> Result<Recovery, NodeError> {
    let n = config.validators + config.standby;
    let Some(dir) = &config.data_dir else {
        return Ok(Recovery {
            generation: 0,
            lanes: (0..n).map(|_| (None, Vec::new())).collect(),
            states: (0..n)
                .map(|_| {
                    (
                        genesis_state.clone(),
                        Carry::fresh(genesis_beacon.clone()),
                        Vec::new(),
                        None,
                    )
                })
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
    // Every restart is a new generation, so consensus sessions never
    // repeat.
    let generation_file = dir.join("generation");
    let generation = match fs::read_to_string(&generation_file) {
        Ok(text) => text.trim().parse::<u64>().map(|e| e + 1).unwrap_or(0),
        Err(_) => 0,
    };
    fs::write(&generation_file, generation.to_string())?;

    let mut states = Vec::with_capacity(n);
    for index in 0..n {
        let (wal, recovered) = Wal::open(&dir.join(format!("validator-{index}")), config.fsync)?;
        let (base, carry, forced) = match recovered.snapshot {
            Some(snapshot) => (snapshot.state, snapshot.carry, snapshot.forced),
            None => (
                genesis_state.clone(),
                Carry::fresh(genesis_beacon.clone()),
                Vec::new(),
            ),
        };
        let (state, carry, forced) = replay(base, carry, forced, &recovered.records)?;
        states.push((state, carry, forced, Some(wal)));
    }
    // Validators stop together, but a validator may have logged one Pulse
    // more than another; replicas are deterministic, so the laggards adopt
    // the most advanced state (state sync), with the beacon reveals already
    // committed for the Pulses after it, and snapshot it.
    let best = (0..n).max_by_key(|i| states[*i].0.pulse).unwrap_or(0);
    let (best_state, best_carry, best_forced) = (
        states[best].0.clone(),
        states[best].1.clone(),
        states[best].2.clone(),
    );
    for (state, carry, forced, wal) in states.iter_mut() {
        if state.pulse < best_state.pulse {
            *state = best_state.clone();
            *carry = best_carry.clone();
            *forced = best_forced.clone();
            if let Some(wal) = wal {
                wal.snapshot(&Snapshot {
                    state: state.clone(),
                    carry: carry.clone(),
                    generation,
                    forced: forced.clone(),
                })?;
            }
        }
    }
    // Lane locks survive the restart: each validator restores the locks of
    // transactions that have neither executed nor expired, and compacts its
    // log to them.
    let mut lanes = Vec::with_capacity(n);
    for (index, (state, _, _, _)) in states.iter().enumerate() {
        let (mut log, entries) =
            LaneLog::open(&dir.join(format!("validator-{index}")), config.fsync)?;
        let live: Vec<(ShardId, LaneRequest)> = entries
            .into_iter()
            .filter(|(_, request)| crate::owners::lock_is_live(state, request))
            .collect();
        log.compact(&live)?;
        lanes.push((Some(log), live));
    }
    Ok(Recovery {
        generation,
        states,
        lanes,
    })
}

impl Devnet {
    /// Starts a devnet on the in-memory network.
    pub fn start(config: DevnetConfig) -> Result<Devnet, NodeError> {
        Devnet::start_with(config, DevnetNetwork::default())
    }

    /// Starts a devnet whose nodes reach each other as `network` says.
    pub fn start_with(config: DevnetConfig, network: DevnetNetwork) -> Result<Devnet, NodeError> {
        if config.validators == 0 {
            return Err(NodeError::Config("at least one validator".into()));
        }
        // Genesis validators first, then standby validators that run from
        // the start, observe, and can join by staking.
        let n = config.validators + config.standby;
        let mode = config.signatures;
        let keys: Vec<ValidatorKeys> = (0..n)
            .map(|i| ValidatorKeys::devnet(&config.chain_id, i))
            .collect();
        let verifier = NodeVerifier::new(&keys, mode);
        let ids: Vec<_> = keys.iter().map(|k| k.id).collect();

        let genesis_spec = devnet_genesis(&config);
        let genesis_digest = genesis_spec.digest();
        let genesis_state = genesis(&genesis_spec)?;
        let chains: Vec<HashChain> = (0..n).map(|i| beacon_chain(&config.chain_id, i)).collect();
        let genesis_beacon = genesis_beacon(&genesis_spec, genesis_digest);
        let recovery = recover(&config, genesis_digest, &genesis_state, &genesis_beacon)?;

        // The chain identity fixes the genesis validators, their keys and
        // the shard layout. Committees after that are in the state: light
        // clients follow them from epoch to epoch.
        let chain = chain_spec(&config, &genesis_spec, genesis_digest)?;
        let chain_id = chain.id();

        let shared = Arc::new(Shared {
            config: config.clone(),
            genesis: genesis_spec,
            genesis_digest,
            chain,
            chain_id,
            generation: recovery.generation,
            verifier,
            validators: ids.clone(),
            view: std::sync::RwLock::new(CommitteeView::default()),
        });

        let (senders, receivers): (Vec<Sender<Input>>, Vec<_>) =
            (0..n).map(|_| mpsc::channel()).unzip();
        let (gateway_sender, gateway_receiver) = mpsc::channel();
        let control = Control {
            validators: senders,
            gateway: gateway_sender,
        };

        // One endpoint per validator and one for the gateway; each delivers
        // into its node's inbox.
        let memory = MemoryNetwork::new(class_limits(&config), network.encode);
        let transports: Vec<Arc<dyn Transport>> = control
            .validators
            .iter()
            .zip(&ids)
            .map(|(inbox, id)| {
                let inbox = inbox.clone();
                let deliver: Delivery = Arc::new(move |from, message| {
                    let _ = inbox.send(Input::Peer { from, message });
                });
                network.wrap(memory.endpoint(NodeId::Validator(*id), deliver))
            })
            .collect();
        let gateway_id = gateway_node(&config.chain_id);
        let gateway_inbox = control.gateway.clone();
        let gateway_transport = network.wrap(memory.endpoint(
            gateway_id,
            Arc::new(move |from, message| {
                let _ = gateway_inbox.send(GatewayInput::Peer {
                    from,
                    message: Box::new(message),
                });
            }),
        ));

        let proven = Arc::new(Mutex::new(BTreeMap::new()));
        let prover = config
            .prove_chunk
            .map(|_| prover::spawn(shared.clone(), proven.clone()));

        let mut threads = Vec::new();
        for (
            index,
            (((receiver, (state, carry, forced_txs, wal)), chain), (lane_log, lane_locks)),
        ) in receivers
            .into_iter()
            .zip(recovery.states)
            .zip(chains)
            .zip(recovery.lanes)
            .enumerate()
        {
            let setup = ValidatorSetup {
                index,
                auth: keys[index].authenticator(mode),
                state,
                carry,
                chain,
                wal,
                inbox: receiver,
                transport: transports[index].clone(),
                // Validator 0 tells the gateway what executed.
                executed_to: ExecutedReports::To(if index == 0 {
                    vec![gateway_id]
                } else {
                    Vec::new()
                }),
                prover: if index == 0 {
                    prover.as_ref().map(|p| p.jobs.clone())
                } else {
                    None
                },
                proven: proven.clone(),
                forced_txs,
                boundary_dir: config
                    .data_dir
                    .as_ref()
                    .map(|dir| dir.join(format!("validator-{index}")).join("epochs")),
                lane_log,
                lane_locks,
                // A devnet's sessions change with every restart.
                session_guard: None,
            };
            let validator = Validator::new(shared.clone(), setup);
            threads.push(
                thread::Builder::new()
                    .name(format!("validator-{index}"))
                    .stack_size(crate::THREAD_STACK_BYTES)
                    .spawn(move || validator.run())?,
            );
        }
        // The gateway asks validator 0 about the state.
        let gateway = Gateway::new(
            shared.clone(),
            gateway_transport,
            local_querier(control.validators[0].clone(), Duration::from_secs(15)),
            gateway_receiver,
            Wallet::named(FAUCET),
        );
        threads.push(
            thread::Builder::new()
                .name("gateway".into())
                .stack_size(crate::THREAD_STACK_BYTES)
                .spawn(move || gateway.run())?,
        );
        Ok(Devnet {
            shared,
            control,
            network: memory,
            transports,
            threads,
            prover,
        })
    }

    /// The in-memory network the devnet's nodes share.
    pub fn network(&self) -> &Arc<MemoryNetwork> {
        &self.network
    }

    /// Validator `index`'s transport (its traffic counters, for example).
    pub fn transport(&self, index: usize) -> Option<&Arc<dyn Transport>> {
        self.transports.get(index)
    }

    /// Asks validator `index` a question.
    pub fn query_validator(&self, index: usize, query: Query) -> Result<Value, String> {
        let (reply, answer) = mpsc::channel();
        self.control
            .validators
            .get(index)
            .ok_or("no such validator")?
            .send(Input::Query(query, reply))
            .map_err(|_| "validator stopped".to_string())?;
        answer
            .recv_timeout(API_TIMEOUT)
            .map_err(|_| "validator did not answer in time".to_string())?
    }

    /// Asks validator 0.
    pub fn query(&self, query: Query) -> Result<Value, String> {
        self.query_validator(0, query)
    }

    /// Submits a signed transaction.
    pub fn submit(&self, tx: SignedTransaction) -> Result<SubmitReceipt, String> {
        let (reply, answer) = mpsc::channel();
        self.control
            .gateway
            .send(GatewayInput::Submit {
                tx: Box::new(tx),
                reply,
            })
            .map_err(|_| "gateway stopped".to_string())?;
        answer
            .recv_timeout(API_TIMEOUT)
            .map_err(|_| "gateway did not answer".to_string())?
    }

    /// Posts a forced intent: the transaction goes straight to every
    /// validator's proposals, bypassing the gateway (any honest proposer is
    /// enough). The Pulse that commits it posts it as an obligation of its
    /// sender's shard, which then has `forced_window` Pulses before the
    /// state checks whether it could still execute.
    pub fn force(&self, tx: SignedTransaction) -> Result<SubmitReceipt, String> {
        let digest = tx.digest();
        let shard = sion_core::ids::ShardId::for_account(&tx.tx.sender, self.shared.config.shards);
        let item = crate::payload::PayloadItem::Forced { tx: Box::new(tx) }.to_bytes();
        for sender in &self.control.validators {
            let _ = sender.send(Input::Submit(item.clone()));
        }
        Ok(SubmitReceipt {
            digest,
            route: "forced".into(),
            shard: Some(shard.0),
        })
    }

    /// **Adversary simulation (devnet only).** Censors these accounts: the
    /// gateway drops their transactions silently if `gateway`, and the
    /// first `validators` validators ignore their lane requests and drop
    /// their transactions from proposals. An empty list stops censorship.
    pub fn censor(&self, accounts: Vec<AccountId>, gateway: bool, validators: usize) {
        let censored: std::collections::BTreeSet<AccountId> = accounts.into_iter().collect();
        let _ = self.control.gateway.send(GatewayInput::Censor {
            accounts: if gateway {
                censored.clone()
            } else {
                Default::default()
            },
        });
        for (index, sender) in self.control.validators.iter().enumerate() {
            let _ = sender.send(Input::Adversary {
                censored: censored.clone(),
                censor: index < validators,
            });
        }
    }

    /// Queues faucet grants.
    pub fn faucet(&self, grants: Vec<(AccountId, AssetId, Amount)>) -> Result<(), String> {
        let (reply, answer) = mpsc::channel();
        self.control
            .gateway
            .send(GatewayInput::Faucet { grants, reply })
            .map_err(|_| "gateway stopped".to_string())?;
        answer
            .recv_timeout(API_TIMEOUT)
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
        self.control
            .gateway
            .send(GatewayInput::Status { tx: digest, reply })
            .map_err(|_| "gateway stopped".to_string())?;
        answer
            .recv_timeout(API_TIMEOUT)
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
    pub fn shutdown(mut self) {
        self.stop();
    }

    fn stop(&mut self) {
        for sender in &self.control.validators {
            let _ = sender.send(Input::Shutdown);
        }
        let _ = self.control.gateway.send(GatewayInput::Shutdown);
        if let Some(prover) = &self.prover {
            prover.stop();
        }
        for thread in std::mem::take(&mut self.threads) {
            let _ = thread.join();
        }
        if let Some(prover) = self.prover.take() {
            prover.join();
        }
    }
}

/// A devnet that goes out of scope stops its threads (a test that fails
/// half-way does not leave validators running).
impl Drop for Devnet {
    fn drop(&mut self) {
        self.stop();
    }
}
