//! One node of a network, in its own process: a validator, optionally with
//! a gateway, or a gateway alone, talking to the others over QUIC.
//!
//! A [`Node`] runs one validator with its own data directory (log,
//! snapshots, lane locks, epoch proofs), the same validator code the devnet
//! runs, on a [`QuicTransport`]. It accepts connections from the validators
//! and gateways its genesis and peer files pin, and dials the validators
//! whose identifier is larger than its own. Optionally it hosts a gateway
//! too: the two share the node's identity, and what arrives is routed by
//! kind (approvals, refusals and execution reports to the gateway,
//! everything else to the validator).
//!
//! A [`GatewayNode`] is a gateway alone. It dials every validator, receives
//! approvals on its own connections, and follows the committees by asking
//! one node it trusts for them.
//!
//! Between machines a consensus session is the epoch: there is no restart
//! generation to agree on. Without a consensus signing log (not built), a
//! validator therefore never starts a session it has signed in before
//! ([`claim_session`]): a restart is refused rather than risking a second
//! header for a round it already signed, and since a stopped validator's
//! own state is still in that session (Pulse sync is not built either), it
//! does not come back (`net:no-rejoin`).

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, RwLock};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde_json::Value;
use sion_consensus::{Beacon, LaneRequest};
use sion_core::hash::Hash32;
use sion_core::ids::{AccountId, AssetId, ShardId, TxDigest, ValidatorId};
use sion_core::Amount;
use sion_exec::{SignedTransaction, State, Wallet};
use sion_net::record::resolve;
use sion_net::EndpointConfig;

use crate::api::Backend;
use crate::committees::{decode_view, CommitteeView, Committees};
use crate::devnet::{chain_spec, class_limits, genesis_beacon, local_querier, API_TIMEOUT, FAUCET};
use crate::error::NodeError;
use crate::gateway::{Gateway, GatewayInput, Querier};
use crate::keys::{NodeVerifier, SignatureMode};
use crate::network::{
    load_network_key, load_validator, network_peers, validator_records, NetworkGenesis, PeerFile,
};
use crate::payload::PayloadItem;
use crate::prover::{self, ProverHandle};
use crate::pulses::Carry;
use crate::storage::{replay, LaneLog, Wal};
use crate::transport::quic::QuicTransport;
use crate::transport::{Delivery, NodeId, PeerMessage, Transport};
use crate::validator::{ExecutedReports, Input, Shared, Validator, ValidatorSetup};
use crate::view::{Query, SubmitReceipt, TxStatus, TxView};

/// How a node asks another node (a gateway alone asks the node it trusts).
pub type NodeQuery = Arc<dyn Fn(Query) -> Result<Value, String> + Send + Sync>;

/// How often a gateway alone refreshes the committees.
const FOLLOW_INTERVAL: Duration = Duration::from_millis(250);

/// The largest signed transaction posted as a forced intent.
const MAX_FORCED_BYTES: usize = 256 * 1024;

fn config_error(message: impl Into<String>) -> NodeError {
    NodeError::Config(message.into())
}

/// Routes what arrives by kind: to the gateway what is for gateways, the
/// rest to the validator (either may be absent).
fn route(validator: Option<Sender<Input>>, gateway: Option<Sender<GatewayInput>>) -> Delivery {
    Arc::new(move |from, message| {
        if message.for_gateway() {
            if let Some(gateway) = &gateway {
                let _ = gateway.send(GatewayInput::Peer {
                    from,
                    message: Box::new(message),
                });
            }
        } else if let Some(validator) = &validator {
            let _ = validator.send(Input::Peer { from, message });
        }
    })
}

/// Everything a node shares with its validator and gateway threads, built
/// from the genesis file the same way on every node.
fn shared(
    genesis: &NetworkGenesis,
    peers: &PeerFile,
    data_dir: Option<PathBuf>,
    prove_chunk: Option<usize>,
    fsync: bool,
) -> Result<Shared, NodeError> {
    let mut config = genesis.config();
    config.data_dir = data_dir;
    config.prove_chunk = prove_chunk;
    config.fsync = fsync;
    let spec = genesis.spec();
    let genesis_digest = spec.digest();
    let chain = chain_spec(&config, &spec, genesis_digest)?;
    let chain_id = chain.id();
    let mut validators: Vec<ValidatorId> = spec.validators.iter().map(|v| v.id).collect();
    for extra in validator_records(genesis, peers)?.keys() {
        if !validators.contains(extra) {
            validators.push(*extra);
        }
    }
    let verifier = NodeVerifier::dual(chain.keys.clone());
    Ok(Shared {
        config,
        genesis: spec,
        genesis_digest,
        chain,
        chain_id,
        // Between machines a session is the epoch.
        generation: 0,
        verifier,
        validators,
        view: RwLock::new(CommitteeView::default()),
    })
}

/// A validator's recovered state.
struct Recovered {
    state: State,
    carry: Carry,
    forced: Vec<SignedTransaction>,
    wal: Wal,
    lane_log: LaneLog,
    lane_locks: Vec<(ShardId, LaneRequest)>,
}

/// Opens a validator's data directory: the log and snapshots replayed and
/// checked, the live lane locks restored.
fn recover(
    data: &Path,
    genesis_digest: Hash32,
    genesis_state: &State,
    genesis_beacon: &Beacon,
    fsync: bool,
) -> Result<Recovered, NodeError> {
    fs::create_dir_all(data)?;
    let marker = data.join("genesis");
    match fs::read_to_string(&marker) {
        Ok(stored) if stored.trim() != genesis_digest.to_hex() => {
            return Err(config_error(format!(
                "{} holds another chain (genesis {}); use a fresh directory",
                data.display(),
                stored.trim()
            )))
        }
        Ok(_) => {}
        Err(_) => fs::write(&marker, genesis_digest.to_hex())?,
    }
    let dir = data.join("validator");
    let (wal, recovered) = Wal::open(&dir, fsync)?;
    let (base, carry, forced) = match recovered.snapshot {
        Some(snapshot) => (snapshot.state, snapshot.carry, snapshot.forced),
        None => (
            genesis_state.clone(),
            Carry::fresh(genesis_beacon.clone()),
            Vec::new(),
        ),
    };
    let (state, carry, forced) = replay(base, carry, forced, &recovered.records)?;
    let (mut lane_log, entries) = LaneLog::open(&dir, fsync)?;
    let lane_locks: Vec<(ShardId, LaneRequest)> = entries
        .into_iter()
        .filter(|(_, request)| crate::owners::lock_is_live(&state, request))
        .collect();
    lane_log.compact(&lane_locks)?;
    Ok(Recovered {
        state,
        carry,
        forced,
        wal,
        lane_log,
        lane_locks,
    })
}

/// The file in a validator's data directory naming the last consensus
/// session it started signing in.
pub const SIGNED_SESSION_FILE: &str = "signed-session";

/// Records, before anything is signed, that this validator starts signing
/// in consensus `session`, and refuses a session it may already have signed
/// in: rejoining one safely needs the consensus signing log, which is not
/// built (`net:no-rejoin`). The record is replaced atomically, and one that
/// cannot be read refuses every session.
pub fn claim_session(data: &Path, session: u64) -> Result<(), NodeError> {
    let path = data.join(SIGNED_SESSION_FILE);
    match fs::read_to_string(&path) {
        Ok(text) => {
            let signed: u64 = text.trim().parse().map_err(|_| {
                config_error(format!(
                    "{} is unreadable, so this validator cannot tell which consensus sessions it \
                     signed in; it refuses to start",
                    path.display()
                ))
            })?;
            if session <= signed {
                return Err(config_error(format!(
                    "this validator already signed in consensus session {signed} and would start \
                     session {session}: rejoining a session it signed in needs the consensus \
                     signing log, which is not built yet, so it could sign a second header for a \
                     round and be slashed. A validator of a network that stops cannot rejoin yet \
                     (net:no-rejoin)"
                )));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let fresh = data.join(format!("{SIGNED_SESSION_FILE}.new"));
    {
        let mut file = fs::File::create(&fresh)?;
        file.write_all(session.to_string().as_bytes())?;
        file.sync_all()?;
    }
    fs::rename(&fresh, &path)?;
    // The rename itself must survive a crash too (where directories can be
    // synced).
    if let Ok(directory) = fs::File::open(data) {
        let _ = directory.sync_all();
    }
    Ok(())
}

/// How to run a validator of a network.
#[derive(Clone, Debug)]
pub struct NodeOptions {
    /// Its directory: the key files `sion keys validator` wrote, and
    /// `data/` for its log and snapshots.
    pub dir: PathBuf,
    /// The network's genesis.
    pub genesis: NetworkGenesis,
    /// The network's peer file.
    pub peers: PeerFile,
    /// Where to listen (default: the first address of its record).
    pub listen: Option<SocketAddr>,
    /// Whether it hosts a gateway too.
    pub gateway: bool,
    /// Transactions per proof chunk, if it runs the prover.
    pub prove_chunk: Option<usize>,
    /// Whether to fsync every log record.
    pub fsync: bool,
}

/// A running validator of a network.
pub struct Node {
    /// What its threads share.
    pub shared: Arc<Shared>,
    id: ValidatorId,
    validator: Sender<Input>,
    gateway: Option<Sender<GatewayInput>>,
    transport: Arc<QuicTransport>,
    threads: Vec<JoinHandle<()>>,
    prover: Option<ProverHandle>,
}

impl std::fmt::Debug for Node {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Node")
            .field("validator", &self.id.short())
            .field("gateway", &self.gateway.is_some())
            .field("transport", &self.transport)
            .finish()
    }
}

impl Node {
    /// Starts the validator whose keys are in `options.dir`.
    pub fn start(options: NodeOptions) -> Result<Node, NodeError> {
        let NodeOptions {
            dir,
            genesis,
            peers,
            listen,
            gateway,
            prove_chunk,
            fsync,
        } = options;
        genesis.check()?;
        let secrets = load_validator(&dir)?;
        let id = secrets.keys.id;
        let records = validator_records(&genesis, &peers)?;
        let own = records.get(&id).ok_or_else(|| {
            config_error(format!(
                "validator {} has no network record in the genesis or the peer file",
                id.short()
            ))
        })?;
        if own.record.network_key != *secrets.network.public() {
            return Err(config_error(
                "this node's network key is not the one its record pins",
            ));
        }
        if let Some(entry) = genesis.validators.iter().find(|v| v.entry.id() == id) {
            if entry.entry.beacon_tip != secrets.beacon.tip() {
                return Err(config_error(
                    "this node's beacon seed does not match the tip its genesis entry names",
                ));
            }
        }
        let listen = match listen {
            Some(address) => address,
            None => resolve(&own.record.addresses)
                .map_err(|e| config_error(e.to_string()))?
                .first()
                .copied()
                .ok_or_else(|| config_error("the node's record names no address to listen on"))?,
        };

        let data = dir.join("data");
        let shared = Arc::new(shared(
            &genesis,
            &peers,
            Some(data.clone()),
            prove_chunk,
            fsync,
        )?);
        let genesis_state = sion_exec::genesis(&shared.genesis)?;
        let beacon = genesis_beacon(&shared.genesis, shared.genesis_digest);
        let recovered = recover(&data, shared.genesis_digest, &genesis_state, &beacon, fsync)?;
        let session = Committees::derive(
            &recovered.state,
            shared.chain_id,
            shared.generation,
            shared.config.shards,
        )
        .map_err(config_error)?
        .session;

        let (validator_tx, validator_rx) = mpsc::channel();
        let gateway_channel = gateway.then(mpsc::channel::<GatewayInput>);
        let gateway_tx = gateway_channel.as_ref().map(|(tx, _)| tx.clone());
        let mut endpoint =
            EndpointConfig::new(shared.chain_id, NodeId::Validator(id), secrets.network);
        endpoint.listen = Some(listen);
        endpoint.peers = network_peers(&genesis, &peers)?
            .into_iter()
            .filter(|peer| peer.node != NodeId::Validator(id))
            .collect();
        endpoint.limits = class_limits(&shared.config);
        let transport = Arc::new(
            QuicTransport::start(
                endpoint,
                route(Some(validator_tx.clone()), gateway_tx.clone()),
            )
            .map_err(|e| config_error(format!("network: {e}")))?,
        );
        // Last before the validator starts signing: a start that failed
        // earlier (a port in use, say) signed nothing and claims nothing.
        claim_session(&data, session)?;

        let proven = Arc::new(Mutex::new(BTreeMap::new()));
        let prover = prove_chunk.map(|_| prover::spawn(shared.clone(), proven.clone()));
        let setup = ValidatorSetup {
            index: 0,
            auth: secrets.keys.authenticator(SignatureMode::DualPostQuantum),
            state: recovered.state,
            carry: recovered.carry,
            chain: secrets.beacon,
            wal: Some(recovered.wal),
            inbox: validator_rx,
            transport: transport.clone(),
            executed_to: ExecutedReports::Subscribers,
            prover: prover.as_ref().map(|p| p.jobs.clone()),
            proven,
            forced_txs: recovered.forced,
            boundary_dir: Some(data.join("validator").join("epochs")),
            lane_log: Some(recovered.lane_log),
            lane_locks: recovered.lane_locks,
            session_guard: Some(data.clone()),
        };
        let validator = Validator::new(shared.clone(), setup);
        let mut threads = vec![thread::Builder::new()
            .name("validator".into())
            .stack_size(crate::THREAD_STACK_BYTES)
            .spawn(move || validator.run())?];
        if let Some((_, gateway_rx)) = gateway_channel {
            let gateway = Gateway::new(
                shared.clone(),
                transport.clone(),
                local_querier(validator_tx.clone(), Duration::from_secs(15)),
                gateway_rx,
                Wallet::named(FAUCET),
            );
            threads.push(
                thread::Builder::new()
                    .name("gateway".into())
                    .stack_size(crate::THREAD_STACK_BYTES)
                    .spawn(move || gateway.run())?,
            );
        }
        Ok(Node {
            shared,
            id,
            validator: validator_tx,
            gateway: gateway_tx,
            transport,
            threads,
            prover,
        })
    }

    /// The validator.
    pub fn id(&self) -> ValidatorId {
        self.id
    }

    /// Its transport (address, links, counters).
    pub fn transport(&self) -> &QuicTransport {
        &self.transport
    }

    /// Asks the validator a question.
    pub fn query(&self, query: Query) -> Result<Value, String> {
        let (reply, answer) = mpsc::channel();
        self.validator
            .send(Input::Query(query, reply))
            .map_err(|_| "validator stopped".to_string())?;
        answer
            .recv_timeout(API_TIMEOUT)
            .map_err(|_| "validator did not answer in time".to_string())?
    }

    fn gateway(&self) -> Result<&Sender<GatewayInput>, String> {
        self.gateway.as_ref().ok_or_else(|| {
            "this node runs no gateway: start it with --gateway, or submit to a gateway".into()
        })
    }

    /// Submits a transaction through the node's gateway.
    pub fn submit(&self, tx: SignedTransaction) -> Result<SubmitReceipt, String> {
        let (reply, answer) = mpsc::channel();
        self.gateway()?
            .send(GatewayInput::Submit {
                tx: Box::new(tx),
                reply,
            })
            .map_err(|_| "gateway stopped".to_string())?;
        answer
            .recv_timeout(API_TIMEOUT)
            .map_err(|_| "gateway did not answer".to_string())?
    }

    /// Where a transaction is: executed, or what the node's gateway knows.
    pub fn status(&self, digest: TxDigest) -> Result<Option<TxStatus>, String> {
        if let Ok(value) = self.query(Query::Tx(digest)) {
            let view: TxView = serde_json::from_value(value).map_err(|e| e.to_string())?;
            return Ok(Some(TxStatus::Executed(Box::new(view))));
        }
        let Some(gateway) = &self.gateway else {
            return Ok(None);
        };
        let (reply, answer) = mpsc::channel();
        gateway
            .send(GatewayInput::Status { tx: digest, reply })
            .map_err(|_| "gateway stopped".to_string())?;
        answer
            .recv_timeout(API_TIMEOUT)
            .map_err(|_| "gateway did not answer".to_string())
    }

    /// Posts a forced intent: to this validator's proposals and to every
    /// other validator's (one honest proposer is enough).
    pub fn force(&self, tx: SignedTransaction) -> Result<SubmitReceipt, String> {
        if tx.size() > MAX_FORCED_BYTES {
            return Err("transaction too large".into());
        }
        let digest = tx.digest();
        let shard = ShardId::for_account(&tx.tx.sender, self.shared.config.shards);
        let item = PayloadItem::Forced { tx: Box::new(tx) }.to_bytes();
        self.validator
            .send(Input::Submit(item.clone()))
            .map_err(|_| "validator stopped".to_string())?;
        let others: Vec<NodeId> = self
            .shared
            .validators
            .iter()
            .filter(|v| **v != self.id)
            .map(|v| NodeId::Validator(*v))
            .collect();
        self.transport
            .send_all(&others, &PeerMessage::Payload { item });
        Ok(SubmitReceipt {
            digest,
            route: "forced".into(),
            shard: Some(shard.0),
        })
    }

    /// Queues faucet grants with the node's gateway (test networks).
    pub fn faucet(&self, grants: Vec<(AccountId, AssetId, Amount)>) -> Result<(), String> {
        let (reply, answer) = mpsc::channel();
        self.gateway()?
            .send(GatewayInput::Faucet { grants, reply })
            .map_err(|_| "gateway stopped".to_string())?;
        answer
            .recv_timeout(API_TIMEOUT)
            .map_err(|_| "gateway did not answer".to_string())?
    }

    /// Stops the node (the validator writes a final snapshot).
    pub fn shutdown(mut self) {
        self.stop();
    }

    fn stop(&mut self) {
        let _ = self.validator.send(Input::Shutdown);
        if let Some(gateway) = &self.gateway {
            let _ = gateway.send(GatewayInput::Shutdown);
        }
        if let Some(prover) = &self.prover {
            prover.stop();
        }
        for thread in std::mem::take(&mut self.threads) {
            let _ = thread.join();
        }
        if let Some(prover) = self.prover.take() {
            prover.join();
        }
        self.transport.endpoint().shutdown();
    }
}

impl Drop for Node {
    fn drop(&mut self) {
        self.stop();
    }
}

impl Backend for Node {
    fn query(&self, query: Query) -> Result<Value, String> {
        Node::query(self, query)
    }

    fn submit(&self, tx: SignedTransaction) -> Result<SubmitReceipt, String> {
        Node::submit(self, tx)
    }

    fn status(&self, digest: TxDigest) -> Result<Option<TxStatus>, String> {
        Node::status(self, digest)
    }

    fn faucet(&self, grants: Vec<(AccountId, AssetId, Amount)>) -> Result<(), String> {
        Node::faucet(self, grants)
    }

    fn force(&self, tx: SignedTransaction) -> Result<SubmitReceipt, String> {
        Node::force(self, tx)
    }

    fn censor(&self, _: Vec<AccountId>, _: bool, _: usize) -> Result<(), String> {
        Err("the adversary simulation runs on in-process devnets only".into())
    }
}

/// How to run a gateway alone.
#[derive(Clone)]
pub struct GatewayOptions {
    /// Its directory: the network key `sion keys gateway` wrote.
    pub dir: PathBuf,
    /// The network's genesis.
    pub genesis: NetworkGenesis,
    /// The network's peer file (it must list this gateway, or validators
    /// refuse its connections).
    pub peers: PeerFile,
    /// The node it asks about the state and the committees.
    pub node: NodeQuery,
}

impl std::fmt::Debug for GatewayOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GatewayOptions")
            .field("dir", &self.dir)
            .field("chain", &self.genesis.chain_id)
            .finish()
    }
}

/// A running gateway, alone in its process.
pub struct GatewayNode {
    /// What its threads share.
    pub shared: Arc<Shared>,
    gateway: Sender<GatewayInput>,
    transport: Arc<QuicTransport>,
    node: NodeQuery,
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
}

impl std::fmt::Debug for GatewayNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GatewayNode")
            .field("transport", &self.transport)
            .finish()
    }
}

impl GatewayNode {
    /// Starts the gateway whose network key is in `options.dir`.
    pub fn start(options: GatewayOptions) -> Result<GatewayNode, NodeError> {
        let GatewayOptions {
            dir,
            genesis,
            peers,
            node,
        } = options;
        genesis.check()?;
        let key = load_network_key(&dir)?;
        let local = key.public().node_id();
        if !peers.nodes.iter().any(|n| n.network_key == *key.public()) {
            eprintln!(
                "gateway: the peer file does not list this gateway's key; validators will refuse it"
            );
        }
        let shared = Arc::new(shared(&genesis, &peers, None, None, false)?);
        let (gateway_tx, gateway_rx) = mpsc::channel();
        let mut endpoint = EndpointConfig::new(shared.chain_id, local, key);
        endpoint.peers = network_peers(&genesis, &peers)?
            .into_iter()
            .filter(|peer| peer.node.is_validator())
            .collect();
        endpoint.limits = class_limits(&shared.config);
        let transport = Arc::new(
            QuicTransport::start(endpoint, route(None, Some(gateway_tx.clone())))
                .map_err(|e| config_error(format!("network: {e}")))?,
        );

        // Follow the committees through the node it trusts.
        let stop = Arc::new(AtomicBool::new(false));
        let follower = {
            let (shared, node, stop) = (shared.clone(), node.clone(), stop.clone());
            thread::Builder::new()
                .name("committees".into())
                .spawn(move || {
                    while !stop.load(Ordering::Relaxed) {
                        if let Ok((pulse, committees, keys)) =
                            node(Query::Committees).and_then(|value| decode_view(&value))
                        {
                            if let Ok(mut view) = shared.view.write() {
                                view.adopt(pulse, committees, keys);
                            }
                        }
                        thread::sleep(FOLLOW_INTERVAL);
                    }
                })?
        };
        let querier: Querier = {
            let node = node.clone();
            Box::new(move |query| node(query))
        };
        let gateway = Gateway::new(
            shared.clone(),
            transport.clone(),
            querier,
            gateway_rx,
            Wallet::named(FAUCET),
        );
        let threads = vec![
            follower,
            thread::Builder::new()
                .name("gateway".into())
                .stack_size(crate::THREAD_STACK_BYTES)
                .spawn(move || gateway.run())?,
        ];
        Ok(GatewayNode {
            shared,
            gateway: gateway_tx,
            transport,
            node,
            stop,
            threads,
        })
    }

    /// Its transport (links, counters).
    pub fn transport(&self) -> &QuicTransport {
        &self.transport
    }

    /// Whether it has learned the committees from its node yet.
    pub fn follows(&self) -> bool {
        self.shared.committees().is_some()
    }

    /// Submits a transaction.
    pub fn submit(&self, tx: SignedTransaction) -> Result<SubmitReceipt, String> {
        let (reply, answer) = mpsc::channel();
        self.gateway
            .send(GatewayInput::Submit {
                tx: Box::new(tx),
                reply,
            })
            .map_err(|_| "gateway stopped".to_string())?;
        answer
            .recv_timeout(API_TIMEOUT)
            .map_err(|_| "gateway did not answer".to_string())?
    }

    /// Where a transaction is: executed (as its node says), or what the
    /// gateway knows.
    pub fn status(&self, digest: TxDigest) -> Result<Option<TxStatus>, String> {
        if let Ok(value) = (self.node)(Query::Tx(digest)) {
            if let Ok(view) = serde_json::from_value::<TxView>(value) {
                return Ok(Some(TxStatus::Executed(Box::new(view))));
            }
        }
        let (reply, answer) = mpsc::channel();
        self.gateway
            .send(GatewayInput::Status { tx: digest, reply })
            .map_err(|_| "gateway stopped".to_string())?;
        answer
            .recv_timeout(API_TIMEOUT)
            .map_err(|_| "gateway did not answer".to_string())
    }

    /// Stops the gateway.
    pub fn shutdown(mut self) {
        self.close();
    }

    fn close(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.gateway.send(GatewayInput::Shutdown);
        for thread in std::mem::take(&mut self.threads) {
            let _ = thread.join();
        }
        self.transport.endpoint().shutdown();
    }
}

impl Drop for GatewayNode {
    fn drop(&mut self) {
        self.close();
    }
}

impl Backend for GatewayNode {
    fn query(&self, query: Query) -> Result<Value, String> {
        (self.node)(query)
    }

    fn submit(&self, tx: SignedTransaction) -> Result<SubmitReceipt, String> {
        GatewayNode::submit(self, tx)
    }

    fn status(&self, digest: TxDigest) -> Result<Option<TxStatus>, String> {
        GatewayNode::status(self, digest)
    }

    fn faucet(&self, grants: Vec<(AccountId, AssetId, Amount)>) -> Result<(), String> {
        let (reply, answer) = mpsc::channel();
        self.gateway
            .send(GatewayInput::Faucet { grants, reply })
            .map_err(|_| "gateway stopped".to_string())?;
        answer
            .recv_timeout(API_TIMEOUT)
            .map_err(|_| "gateway did not answer".to_string())?
    }

    fn force(&self, _: SignedTransaction) -> Result<SubmitReceipt, String> {
        Err("a gateway does not post forced intents: post them to a node".into())
    }

    fn censor(&self, _: Vec<AccountId>, _: bool, _: usize) -> Result<(), String> {
        Err("the adversary simulation runs on in-process devnets only".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_validator_never_starts_a_session_it_has_signed_in() {
        let dir = std::env::temp_dir().join(format!("sion-node-session-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        claim_session(&dir, 3).unwrap();
        // A restart in the same session, or an earlier one, is refused.
        assert!(claim_session(&dir, 3).is_err());
        assert!(claim_session(&dir, 2).is_err());
        // A later session (the next epoch's state) is fine.
        claim_session(&dir, 4).unwrap();
        assert_eq!(
            fs::read_to_string(dir.join(SIGNED_SESSION_FILE)).unwrap(),
            "4"
        );
        assert!(!dir.join(format!("{SIGNED_SESSION_FILE}.new")).exists());
        // A record that cannot be read refuses every session.
        fs::write(dir.join(SIGNED_SESSION_FILE), "").unwrap();
        assert!(claim_session(&dir, 9).is_err());
        let _ = fs::remove_dir_all(&dir);
    }
}
