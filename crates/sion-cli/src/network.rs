//! A network of nodes in separate processes: `sion keys validator`, `sion
//! keys gateway`, `sion genesis`, `sion node` and `sion gateway`.
//!
//! Each validator generates its keys on its own machine; the public entries
//! are collected into one genesis file, which every node then runs from.
//! Nodes talk QUIC with post-quantum TLS and accept only the keys the
//! genesis and the peer file pin. The guide (`docs/book/src/guides/network.md`)
//! walks through a localhost network.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use clap::Args;
use serde_json::Value;
use sion_core::hash::Hash32;
use sion_core::ids::AccountId;
use sion_node::network::{
    generate_gateway, generate_validator, read_json, write_json, GenesisEntry, NetworkGenesis,
    NetworkParams, NodeEntry, PeerFile, ValidatorEntry, GATEWAY_ENTRY_FILE, GENESIS_VERSION,
    VALIDATOR_ENTRY_FILE,
};
use sion_node::node::{GatewayNode, GatewayOptions, Node, NodeOptions, NodeQuery};
use sion_node::Query;

use crate::error::CliError;
use crate::ledger::{HttpLedger, Ledger};
use crate::session::wallet_for;

/// The genesis file's name in the directory `sion genesis` writes to.
pub const GENESIS_FILE: &str = "genesis.json";
/// The peer file's name next to it.
pub const PEERS_FILE: &str = "peers.json";

fn failed(error: impl std::fmt::Display) -> CliError {
    CliError::Node(error.to_string())
}

/// `sion keys validator`: a validator's keys.
#[derive(Args, Clone, Debug)]
pub struct ValidatorKeysArgs {
    /// The validator's directory (created; it must not hold keys yet).
    #[arg(long)]
    pub dir: PathBuf,
    /// The chain's name.
    #[arg(long, default_value = "sion-localnet")]
    pub chain: String,
    /// Where other nodes reach it, `host:port` (repeat for several).
    #[arg(long = "address", required = true)]
    pub addresses: Vec<String>,
    /// The wallet name or account ID that operates it (stakes for it and
    /// receives its rewards).
    #[arg(long)]
    pub operator: String,
}

/// `sion keys gateway`: a gateway's network key.
#[derive(Args, Clone, Debug)]
pub struct GatewayKeysArgs {
    /// The gateway's directory (created; it must not hold a key yet).
    #[arg(long)]
    pub dir: PathBuf,
    /// A name for people.
    #[arg(long, default_value = "gateway")]
    pub name: String,
}

/// `sion genesis`: collect validators' entries into a genesis file.
#[derive(Args, Clone, Debug)]
pub struct GenesisArgs {
    /// A validator's entry (`validator.json`, written by `sion keys
    /// validator`); repeat for each.
    #[arg(long = "validator", required = true)]
    pub validators: Vec<PathBuf>,
    /// A gateway's entry (`gateway.json`, written by `sion keys gateway`);
    /// repeat for each.
    #[arg(long = "gateway")]
    pub gateways: Vec<PathBuf>,
    /// The chain's name (default: the one the entries name).
    #[arg(long)]
    pub chain: Option<String>,
    /// Each validator's stake, in USD, minted at genesis.
    #[arg(long, default_value_t = 100_000)]
    pub stake: u64,
    /// TEST NETWORKS ONLY: named accounts funded at genesis, whose keys
    /// anyone can derive from their names.
    #[arg(long, value_delimiter = ',')]
    pub accounts: Vec<String>,
    /// USD each named account starts with.
    #[arg(long, default_value_t = 100_000)]
    pub demo_usd: u64,
    /// TEST NETWORKS ONLY: fund the devnet faucet, whose keys anyone can
    /// derive from its name.
    #[arg(long)]
    pub faucet: bool,
    /// Lane shards (default: as `sion devnet`, 2).
    #[arg(long)]
    pub shards: Option<u16>,
    /// Seats per lane committee (default: as `sion devnet`, 4).
    #[arg(long)]
    pub seats: Option<usize>,
    /// Consensus rounds per Pulse (default: as `sion devnet`, 4).
    #[arg(long)]
    pub rounds_per_pulse: Option<u64>,
    /// Pulses per epoch (default: as `sion devnet`, 0: one epoch that never
    /// ends).
    #[arg(long)]
    pub epoch_length: Option<u64>,
    /// Where to write `genesis.json` and `peers.json`.
    #[arg(long, default_value = ".")]
    pub out: PathBuf,
}

/// `sion node`: run one validator of a network.
#[derive(Args, Clone, Debug)]
pub struct NodeArgs {
    /// The validator's directory (its keys; its log and snapshots go to
    /// `data/` inside).
    #[arg(long)]
    pub dir: PathBuf,
    /// The network's genesis file.
    #[arg(long)]
    pub genesis: PathBuf,
    /// The peer file (default: `peers.json` next to the genesis file, if
    /// there is one).
    #[arg(long)]
    pub peers: Option<PathBuf>,
    /// Where to listen for QUIC (default: the first address of its record).
    #[arg(long)]
    pub listen: Option<std::net::SocketAddr>,
    /// Where to serve the HTTP API.
    #[arg(long, default_value = "127.0.0.1:8730")]
    pub api: String,
    /// Host a gateway too: accept transactions on the API and get them
    /// certified.
    #[arg(long)]
    pub gateway: bool,
    /// Run the prover (a World Proof for every Pulse).
    #[arg(long)]
    pub prove: bool,
    /// Transactions per proof chunk, with --prove.
    #[arg(long, default_value_t = 16)]
    pub chunk: usize,
    /// Do not fsync log records (faster, loses the last records on a power
    /// failure).
    #[arg(long)]
    pub no_fsync: bool,
    /// Stop after this many seconds (0: run until interrupted).
    #[arg(long, default_value_t = 0)]
    pub duration: u64,
    /// Print status every this many seconds (0: silent).
    #[arg(long, default_value_t = 5)]
    pub report: u64,
}

/// `sion gateway`: run a gateway alone.
#[derive(Args, Clone, Debug)]
pub struct GatewayArgs {
    /// The gateway's directory (its network key).
    #[arg(long)]
    pub dir: PathBuf,
    /// The network's genesis file.
    #[arg(long)]
    pub genesis: PathBuf,
    /// The peer file, which must list this gateway (default: `peers.json`
    /// next to the genesis file).
    #[arg(long)]
    pub peers: Option<PathBuf>,
    /// The API of the node this gateway trusts for the state and the
    /// committees.
    #[arg(long)]
    pub follow: String,
    /// Where to serve the HTTP API.
    #[arg(long, default_value = "127.0.0.1:8740")]
    pub api: String,
    /// Stop after this many seconds (0: run until interrupted).
    #[arg(long, default_value_t = 0)]
    pub duration: u64,
    /// Print status every this many seconds (0: silent).
    #[arg(long, default_value_t = 5)]
    pub report: u64,
}

/// An account named by a wallet name or a hex account identifier.
fn account(name: &str) -> AccountId {
    match Hash32::from_hex(name) {
        Ok(hash) => AccountId::from_hash(hash),
        Err(_) => wallet_for(name).id,
    }
}

/// `sion keys validator`.
pub fn validator_keys(args: &ValidatorKeysArgs) -> Result<ValidatorEntry, CliError> {
    let entry = generate_validator(
        &args.dir,
        &args.chain,
        args.addresses.clone(),
        account(&args.operator),
    )
    .map_err(failed)?;
    println!(
        "validator {} of chain {}: keys in {} (validator.key, network.key, beacon.key: keep them secret)",
        entry.id(),
        args.chain,
        args.dir.display()
    );
    println!(
        "network key {}, reachable at {}",
        entry.record.record.network_key.digest().short(),
        args.addresses.join(", ")
    );
    println!(
        "its entry for the genesis: {}",
        args.dir.join(VALIDATOR_ENTRY_FILE).display()
    );
    Ok(entry)
}

/// `sion keys gateway`.
pub fn gateway_keys(args: &GatewayKeysArgs) -> Result<NodeEntry, CliError> {
    let entry = generate_gateway(&args.dir, &args.name).map_err(failed)?;
    println!(
        "gateway {} ({}): network key in {} (network.key: keep it secret)",
        args.name,
        entry.node(),
        args.dir.display()
    );
    println!(
        "its entry for the peer file: {}",
        args.dir.join(GATEWAY_ENTRY_FILE).display()
    );
    Ok(entry)
}

/// `sion genesis`: writes the genesis and peer files and returns the
/// genesis.
pub fn genesis(args: &GenesisArgs) -> Result<NetworkGenesis, CliError> {
    let mut validators = Vec::new();
    for path in &args.validators {
        let entry: ValidatorEntry = read_json(path).map_err(failed)?;
        validators.push(GenesisEntry {
            stake: args.stake,
            entry,
        });
    }
    let chain_id = match &args.chain {
        Some(chain) => chain.clone(),
        None => validators
            .first()
            .map(|v| v.entry.record.record.chain.clone())
            .ok_or_else(|| CliError::Usage("at least one --validator".into()))?,
    };
    let defaults = NetworkParams::default();
    let params = NetworkParams {
        shards: args.shards.unwrap_or(defaults.shards),
        seats: args.seats.unwrap_or(defaults.seats),
        rounds_per_pulse: args.rounds_per_pulse.unwrap_or(defaults.rounds_per_pulse),
        epoch_length: args.epoch_length.unwrap_or(defaults.epoch_length),
        ..defaults
    };
    let genesis = NetworkGenesis {
        version: GENESIS_VERSION,
        chain_id,
        params,
        demo_accounts: args.accounts.clone(),
        demo_usd: args.demo_usd,
        faucet: args.faucet,
        accounts: Vec::new(),
        validators,
    };
    genesis.check().map_err(failed)?;
    let mut nodes = Vec::new();
    for path in &args.gateways {
        let entry: NodeEntry = read_json(path).map_err(failed)?;
        nodes.push(entry);
    }
    let peers = PeerFile {
        validators: Vec::new(),
        nodes,
    };
    let chain = genesis.chain_spec().map_err(failed)?;
    std::fs::create_dir_all(&args.out).map_err(failed)?;
    let genesis_path = args.out.join(GENESIS_FILE);
    let peers_path = args.out.join(PEERS_FILE);
    write_json(&genesis_path, &genesis).map_err(failed)?;
    write_json(&peers_path, &peers).map_err(failed)?;
    println!(
        "genesis of chain {}: {} validators, written to {}",
        genesis.chain_id,
        genesis.validators.len(),
        genesis_path.display()
    );
    println!(
        "peer file ({} gateways accepted): {}",
        peers.nodes.len(),
        peers_path.display()
    );
    let mut open: Vec<String> = genesis.demo_accounts.clone();
    if genesis.faucet {
        open.push(sion_node::devnet::FAUCET.to_string());
    }
    if !open.is_empty() {
        println!(
            "TEST NETWORK: anyone can spend from {} (their keys derive from their names)",
            open.join(", ")
        );
    }
    println!(
        "chain identity (pin it: sion verify --chain …): {}",
        chain.id().to_hex()
    );
    Ok(genesis)
}

/// The peer file a node runs with: the one named, or `peers.json` next to
/// the genesis file if there is one, or none.
fn peer_file(genesis: &Path, named: Option<&PathBuf>) -> Result<PeerFile, CliError> {
    match named {
        Some(path) => read_json(path).map_err(failed),
        None => {
            let beside = genesis
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join(PEERS_FILE);
            if beside.exists() {
                read_json(&beside).map_err(failed)
            } else {
                Ok(PeerFile::default())
            }
        }
    }
}

/// How many of a node's links are up, and how many it has.
fn links(status: &Value) -> (usize, usize) {
    let links = status["network"]["endpoint"]["links"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    let up = links
        .values()
        .filter(|link| link["connected"].as_bool() == Some(true))
        .count();
    (up, links.len())
}

/// `sion node`: runs until interrupted or for `--duration` seconds.
pub fn run_node(args: &NodeArgs) -> Result<(), CliError> {
    let genesis = NetworkGenesis::read(&args.genesis).map_err(failed)?;
    let peers = peer_file(&args.genesis, args.peers.as_ref())?;
    let started = Instant::now();
    let node = Arc::new(
        Node::start(NodeOptions {
            dir: args.dir.clone(),
            genesis: genesis.clone(),
            peers,
            listen: args.listen,
            gateway: args.gateway,
            prove_chunk: args.prove.then_some(args.chunk.max(1)),
            fsync: !args.no_fsync,
        })
        .map_err(failed)?,
    );
    let api = sion_node::api::serve(node.clone(), &args.api).map_err(failed)?;
    println!(
        "Siontology node: validator {} of chain {}, QUIC on {}, API on http://{}{}",
        node.id(),
        genesis.chain_id,
        node.transport().endpoint().local_addr(),
        api.address,
        if args.gateway { ", with a gateway" } else { "" }
    );
    println!(
        "chain identity (pin it: sion verify --chain …): {}",
        node.shared.chain_id.to_hex()
    );
    let mut last_report = Instant::now();
    loop {
        std::thread::sleep(Duration::from_millis(200));
        if args.report > 0 && last_report.elapsed() >= Duration::from_secs(args.report) {
            last_report = Instant::now();
            if let Ok(status) = node.query(Query::Status) {
                let (up, all) = links(&status);
                println!(
                    "Pulse {:>5}  certified {:>5}  round {:>5}  executed {}  links {up}/{all}",
                    status["pulse"], status["certified_pulse"], status["round"], status["executed"],
                );
            }
        }
        if args.duration > 0 && started.elapsed() >= Duration::from_secs(args.duration) {
            break;
        }
    }
    api.stop();
    if let Ok(node) = Arc::try_unwrap(node) {
        node.shutdown();
    }
    Ok(())
}

/// `sion gateway`: runs until interrupted or for `--duration` seconds.
pub fn run_gateway(args: &GatewayArgs) -> Result<(), CliError> {
    let genesis = NetworkGenesis::read(&args.genesis).map_err(failed)?;
    let peers = peer_file(&args.genesis, args.peers.as_ref())?;
    let ledger = HttpLedger::new(&args.follow);
    let follow: NodeQuery = Arc::new(move |query| ledger.query(query).map_err(|e| e.to_string()));
    let started = Instant::now();
    let gateway = Arc::new(
        GatewayNode::start(GatewayOptions {
            dir: args.dir.clone(),
            genesis: genesis.clone(),
            peers,
            node: follow,
        })
        .map_err(failed)?,
    );
    let api = sion_node::api::serve(gateway.clone(), &args.api).map_err(failed)?;
    println!(
        "Siontology gateway {} of chain {}, following {}, API on http://{}",
        gateway.transport().endpoint().local(),
        genesis.chain_id,
        args.follow,
        api.address
    );
    let mut last_report = Instant::now();
    loop {
        std::thread::sleep(Duration::from_millis(200));
        if args.report > 0 && last_report.elapsed() >= Duration::from_secs(args.report) {
            last_report = Instant::now();
            let stats = gateway.transport().endpoint().stats();
            let up = stats.links.values().filter(|link| link.connected).count();
            println!(
                "links {up}/{}  following: {}",
                stats.links.len(),
                if gateway.follows() { "yes" } else { "not yet" }
            );
        }
        if args.duration > 0 && started.elapsed() >= Duration::from_secs(args.duration) {
            break;
        }
    }
    api.stop();
    if let Ok(gateway) = Arc::try_unwrap(gateway) {
        gateway.shutdown();
    }
    Ok(())
}
