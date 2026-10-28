//! A network of nodes in separate processes: its genesis file, its peer
//! file, and the key files in each node's directory.
//!
//! - `sion keys validator` generates a validator's secrets in its directory
//!   (the dual consensus keys, the network key, the beacon hash chain's
//!   seed) and writes its public [`ValidatorEntry`] to `validator.json`:
//!   its operator, the tip of its hash chain, and a network record that
//!   binds its network key and addresses to it, signed with both of its
//!   consensus keys, which doubles as its proof that it holds them.
//! - `sion keys gateway` generates a gateway's network key and writes its
//!   [`NodeEntry`] to `gateway.json`.
//! - `sion genesis` collects entries into a [`NetworkGenesis`]
//!   (`genesis.json`). It fixes the chain: every node derives the same
//!   genesis state and chain identity from it, with the devnet's own
//!   [`genesis_spec`] and [`chain_spec`]. Next to it, a [`PeerFile`]
//!   (`peers.json`) lists the gateways nodes accept, and any newer records
//!   (`net:static-peers`).

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::Path;

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sion_consensus::{CoreConfig, HashChain};
use sion_core::hash::{hash_domain, Hash32};
use sion_core::ids::{AccountId, ValidatorId};
use sion_core::Amount;
use sion_exec::genesis::GenesisValidator;
use sion_exec::{GenesisAccount, GenesisSpec};
use sion_light::ChainSpec;
use sion_net::{NetworkKey, NetworkPublicKey, NetworkRecord, NodeId, Peer, SignedRecord};

use crate::config::DevnetConfig;
use crate::devnet::{chain_spec, genesis_spec, BEACON_CHAIN_LENGTH};
use crate::error::NodeError;
use crate::keys::{SignatureMode, ValidatorKeys};

/// The genesis file format's version.
pub const GENESIS_VERSION: u32 = 1;

/// A validator's secret consensus-key seed, in its directory.
pub const VALIDATOR_KEY_FILE: &str = "validator.key";
/// A node's secret network-key seed, in its directory.
pub const NETWORK_KEY_FILE: &str = "network.key";
/// A validator's secret beacon hash-chain seed, in its directory.
pub const BEACON_KEY_FILE: &str = "beacon.key";
/// A validator's public entry, in its directory.
pub const VALIDATOR_ENTRY_FILE: &str = "validator.json";
/// A gateway's public entry, in its directory.
pub const GATEWAY_ENTRY_FILE: &str = "gateway.json";

/// The protocol parameters every node of a network runs with.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkParams {
    /// Lane shards.
    pub shards: u16,
    /// Seats per lane committee.
    pub seats: usize,
    /// Consensus rounds per Pulse.
    pub rounds_per_pulse: u64,
    /// How often validators seal lane approval batches, in milliseconds.
    pub batch_interval_ms: u64,
    /// Pulses per epoch (0: one epoch that never ends).
    pub epoch_length: u64,
    /// Pulses between leaving the active set and withdrawing stake.
    pub unbonding: u64,
    /// The least stake a validator may register with, in USD.
    pub min_stake: u64,
    /// Pulses a shard has to execute a forced intent.
    pub forced_window: u64,
    /// Consensus parameters.
    pub core: CoreConfig,
}

impl Default for NetworkParams {
    /// The devnet's parameters.
    fn default() -> Self {
        let devnet = DevnetConfig::default();
        NetworkParams {
            shards: devnet.shards,
            seats: devnet.shard_seats,
            rounds_per_pulse: devnet.rounds_per_pulse,
            batch_interval_ms: devnet.batch_interval_ms,
            epoch_length: devnet.epoch_length,
            unbonding: devnet.unbonding,
            min_stake: devnet.min_stake,
            forced_window: devnet.forced_window,
            core: devnet.core,
        }
    }
}

/// A validator's public entry: what `sion keys validator` writes, before a
/// genesis assigns it a stake.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidatorEntry {
    /// The account that operates it (stakes for it, receives its rewards).
    pub operator: AccountId,
    /// The tip of its beacon hash chain.
    pub beacon_tip: Hash32,
    /// Its network record, signed with both of its consensus keys.
    pub record: SignedRecord,
}

impl ValidatorEntry {
    /// The validator.
    pub fn id(&self) -> ValidatorId {
        self.record.record.validator
    }
}

/// A genesis validator: its entry and its stake.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenesisEntry {
    /// Stake in USD, minted at genesis.
    pub stake: u64,
    /// Its entry.
    pub entry: ValidatorEntry,
}

/// A network's genesis file: everything every node must agree on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkGenesis {
    /// The format's version ([`GENESIS_VERSION`]).
    pub version: u32,
    /// The chain's name.
    pub chain_id: String,
    /// Protocol parameters.
    pub params: NetworkParams,
    /// **Test networks only:** accounts funded at genesis whose keys derive
    /// from their names (anyone can spend from them).
    pub demo_accounts: Vec<String>,
    /// USD each demo account starts with.
    pub demo_usd: u64,
    /// **Test networks only:** the devnet faucet, whose keys derive from its
    /// name.
    pub faucet: bool,
    /// Further accounts.
    pub accounts: Vec<GenesisAccount>,
    /// The genesis validators.
    pub validators: Vec<GenesisEntry>,
}

/// Other nodes a node accepts, and newer validator records.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerFile {
    /// Validator records: one with a higher sequence than the genesis
    /// file's replaces it.
    pub validators: Vec<SignedRecord>,
    /// Other nodes (gateways), by network key.
    pub nodes: Vec<NodeEntry>,
}

/// A node other than a validator: a name and the network key it
/// authenticates with.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeEntry {
    /// A name for people.
    pub name: String,
    /// Its network key.
    pub network_key: NetworkPublicKey,
}

impl NodeEntry {
    /// The node.
    pub fn node(&self) -> NodeId {
        self.network_key.node_id()
    }
}

fn config_error(message: impl Into<String>) -> NodeError {
    NodeError::Config(message.into())
}

impl NetworkGenesis {
    /// Checks the file: its version, at least one validator, each listed
    /// once with a well-formed record for this chain that verifies (the
    /// validator's identifier derives from the record's keys, which both
    /// signed it), distinct network keys, and stakes of at least the
    /// minimum.
    pub fn check(&self) -> Result<(), NodeError> {
        if self.version != GENESIS_VERSION {
            return Err(config_error(format!(
                "genesis format {} (this node reads {GENESIS_VERSION})",
                self.version
            )));
        }
        if self.validators.is_empty() {
            return Err(config_error("the genesis lists no validators"));
        }
        let mut ids = BTreeMap::new();
        let mut keys = BTreeMap::new();
        for validator in &self.validators {
            let record = &validator.entry.record;
            record
                .verify(&self.chain_id)
                .map_err(|e| config_error(e.to_string()))?;
            let id = validator.entry.id();
            if ids.insert(id, ()).is_some() {
                return Err(config_error(format!("{} is listed twice", id.short())));
            }
            if keys.insert(record.record.network_key.clone(), ()).is_some() {
                return Err(config_error(format!(
                    "{} reuses another validator's network key",
                    id.short()
                )));
            }
            if validator.stake < self.params.min_stake {
                return Err(config_error(format!(
                    "{} stakes {} USD, below the minimum of {}",
                    id.short(),
                    validator.stake,
                    self.params.min_stake
                )));
            }
        }
        Ok(())
    }

    /// The parameters a node runs with (the devnet's configuration type;
    /// local settings such as the data directory are the node's).
    pub fn config(&self) -> DevnetConfig {
        let params = &self.params;
        DevnetConfig {
            chain_id: self.chain_id.clone(),
            validators: self.validators.len(),
            standby: 0,
            validator_stake: self.validators.first().map(|v| v.stake).unwrap_or(0),
            min_stake: params.min_stake,
            epoch_length: params.epoch_length,
            unbonding: params.unbonding,
            shards: params.shards,
            shard_seats: params.seats,
            rounds_per_pulse: params.rounds_per_pulse,
            batch_interval_ms: params.batch_interval_ms,
            signatures: SignatureMode::DualPostQuantum,
            data_dir: None,
            accounts: self.accounts.clone(),
            demo_accounts: self.demo_accounts.clone(),
            demo_usd: self.demo_usd,
            core: params.core.clone(),
            prove_chunk: None,
            forced_window: params.forced_window,
            ..DevnetConfig::default()
        }
    }

    /// The genesis specification every node derives.
    pub fn spec(&self) -> GenesisSpec {
        let validators = self
            .validators
            .iter()
            .map(|validator| {
                let record = &validator.entry.record;
                GenesisValidator {
                    id: validator.entry.id(),
                    lattice: record.lattice.clone(),
                    hash: record.hash.clone(),
                    operator: validator.entry.operator,
                    stake: Amount::units(validator.stake),
                    beacon_tip: validator.entry.beacon_tip,
                }
            })
            .collect();
        genesis_spec(&self.config(), validators, self.faucet)
    }

    /// The chain identity's specification: what light clients pin.
    pub fn chain_spec(&self) -> Result<ChainSpec, NodeError> {
        let spec = self.spec();
        chain_spec(&self.config(), &spec, spec.digest())
    }

    /// Reads and checks a genesis file.
    pub fn read(path: &Path) -> Result<NetworkGenesis, NodeError> {
        let genesis: NetworkGenesis = read_json(path)?;
        genesis.check()?;
        Ok(genesis)
    }
}

/// The latest verified record of every validator: the genesis file's,
/// replaced by any record in the peer file with a higher sequence.
pub fn validator_records(
    genesis: &NetworkGenesis,
    peers: &PeerFile,
) -> Result<BTreeMap<ValidatorId, SignedRecord>, NodeError> {
    let mut records: BTreeMap<ValidatorId, SignedRecord> = BTreeMap::new();
    let all = genesis
        .validators
        .iter()
        .map(|v| &v.entry.record)
        .chain(&peers.validators);
    for record in all {
        record
            .verify(&genesis.chain_id)
            .map_err(|e| config_error(format!("peer file: {e}")))?;
        let id = record.record.validator;
        let newer = records.get(&id).map_or(true, |current| {
            record.record.sequence > current.record.sequence
        });
        if newer {
            records.insert(id, record.clone());
        }
    }
    Ok(records)
}

/// The peers of a network: every validator as its latest record says, and
/// every node of the peer file (which dials, so it has no address).
pub fn network_peers(genesis: &NetworkGenesis, peers: &PeerFile) -> Result<Vec<Peer>, NodeError> {
    let mut out = Vec::new();
    for record in validator_records(genesis, peers)?.values() {
        out.push(record.peer().map_err(|e| config_error(e.to_string()))?);
    }
    for node in &peers.nodes {
        node.network_key
            .check()
            .map_err(|e| config_error(format!("peer file: {}: {e}", node.name)))?;
        out.push(Peer {
            node: node.node(),
            key: node.network_key.clone(),
            addresses: Vec::new(),
        });
    }
    Ok(out)
}

/// Reads a JSON file.
pub fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T, NodeError> {
    let text =
        fs::read_to_string(path).map_err(|e| config_error(format!("{}: {e}", path.display())))?;
    serde_json::from_str(&text).map_err(|e| config_error(format!("{}: {e}", path.display())))
}

/// Writes a JSON file (replacing it).
pub fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), NodeError> {
    let text = serde_json::to_string_pretty(value)
        .map_err(|e| config_error(format!("{}: {e}", path.display())))?;
    fs::write(path, text + "\n")?;
    Ok(())
}

/// Writes a secret seed, readable by its owner only, refusing to replace
/// one: a validator's keys are generated once.
pub fn write_seed(path: &Path, seed: &[u8; 32]) -> Result<(), NodeError> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|e| config_error(format!("{}: {e}", path.display())))?;
    file.write_all(hex::encode(seed).as_bytes())?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    Ok(())
}

/// Reads a secret seed.
pub fn read_seed(path: &Path) -> Result<[u8; 32], NodeError> {
    let text =
        fs::read_to_string(path).map_err(|e| config_error(format!("{}: {e}", path.display())))?;
    let bytes = hex::decode(text.trim())
        .map_err(|_| config_error(format!("{} is not hex", path.display())))?;
    bytes
        .try_into()
        .map_err(|_| config_error(format!("{} does not hold 32 bytes", path.display())))
}

/// A validator's secrets, as its directory holds them.
#[derive(Debug)]
pub struct ValidatorSecrets {
    /// Its dual consensus keys.
    pub keys: ValidatorKeys,
    /// Its network key.
    pub network: NetworkKey,
    /// Its beacon hash chain.
    pub beacon: HashChain,
}

fn beacon_chain_from(seed: &[u8; 32]) -> HashChain {
    HashChain::new(
        hash_domain("sion/node/beacon-seed", seed),
        BEACON_CHAIN_LENGTH,
    )
}

/// Generates a validator's secrets in `dir` (which must not hold any yet)
/// and writes its entry for chain `chain_id`, reachable at `addresses`,
/// operated by `operator`.
pub fn generate_validator(
    dir: &Path,
    chain_id: &str,
    addresses: Vec<String>,
    operator: AccountId,
) -> Result<ValidatorEntry, NodeError> {
    fs::create_dir_all(dir)?;
    let seeds = [
        sion_crypto::keys::random_seed(),
        sion_crypto::keys::random_seed(),
        sion_crypto::keys::random_seed(),
    ];
    let keys = ValidatorKeys::from_seed(&seeds[0]);
    let network = NetworkKey::from_seed(&seeds[1]);
    let record = SignedRecord::sign(
        NetworkRecord {
            chain: chain_id.to_string(),
            validator: keys.id,
            network_key: network.public().clone(),
            addresses,
            valid_from_epoch: 0,
            sequence: 1,
        },
        &keys.keys,
    )
    .map_err(|e| config_error(e.to_string()))?;
    record
        .verify(chain_id)
        .map_err(|e| config_error(e.to_string()))?;
    let entry = ValidatorEntry {
        operator,
        beacon_tip: beacon_chain_from(&seeds[2]).tip(),
        record,
    };
    write_seed(&dir.join(VALIDATOR_KEY_FILE), &seeds[0])?;
    write_seed(&dir.join(NETWORK_KEY_FILE), &seeds[1])?;
    write_seed(&dir.join(BEACON_KEY_FILE), &seeds[2])?;
    write_json(&dir.join(VALIDATOR_ENTRY_FILE), &entry)?;
    Ok(entry)
}

/// Loads a validator's secrets from `dir`.
pub fn load_validator(dir: &Path) -> Result<ValidatorSecrets, NodeError> {
    Ok(ValidatorSecrets {
        keys: ValidatorKeys::from_seed(&read_seed(&dir.join(VALIDATOR_KEY_FILE))?),
        network: load_network_key(dir)?,
        beacon: beacon_chain_from(&read_seed(&dir.join(BEACON_KEY_FILE))?),
    })
}

/// Generates a gateway's network key in `dir` and writes its entry.
pub fn generate_gateway(dir: &Path, name: &str) -> Result<NodeEntry, NodeError> {
    fs::create_dir_all(dir)?;
    let seed = sion_crypto::keys::random_seed();
    let key = NetworkKey::from_seed(&seed);
    let entry = NodeEntry {
        name: name.to_string(),
        network_key: key.public().clone(),
    };
    write_seed(&dir.join(NETWORK_KEY_FILE), &seed)?;
    write_json(&dir.join(GATEWAY_ENTRY_FILE), &entry)?;
    Ok(entry)
}

/// Loads a node's network key from `dir`.
pub fn load_network_key(dir: &Path) -> Result<NetworkKey, NodeError> {
    Ok(NetworkKey::from_seed(&read_seed(
        &dir.join(NETWORK_KEY_FILE),
    )?))
}

#[cfg(test)]
mod tests {
    use sion_exec::Wallet;

    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("sion-network-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    /// A genesis of `n` freshly generated validators.
    fn network(name: &str, n: usize) -> (std::path::PathBuf, NetworkGenesis) {
        let dir = scratch(name);
        let validators = (0..n)
            .map(|i| GenesisEntry {
                stake: 100_000,
                entry: generate_validator(
                    &dir.join(format!("node-{i}")),
                    "sion-test-net",
                    vec![format!("127.0.0.1:{}", 9100 + i)],
                    Wallet::named(&format!("operator-{i}")).id,
                )
                .unwrap(),
            })
            .collect();
        let genesis = NetworkGenesis {
            version: GENESIS_VERSION,
            chain_id: "sion-test-net".into(),
            params: NetworkParams::default(),
            demo_accounts: vec!["alice".into()],
            demo_usd: 1_000,
            faucet: false,
            accounts: Vec::new(),
            validators,
        };
        (dir, genesis)
    }

    #[test]
    fn a_genesis_fixes_one_chain_and_its_files_reload() {
        let (dir, genesis) = network("fixes", 2);
        genesis.check().unwrap();
        let path = dir.join("genesis.json");
        write_json(&path, &genesis).unwrap();
        let reloaded = NetworkGenesis::read(&path).unwrap();
        assert_eq!(reloaded, genesis);
        // Every node derives the same chain identity.
        assert_eq!(
            reloaded.chain_spec().unwrap().id(),
            genesis.chain_spec().unwrap().id()
        );
        let spec = genesis.spec();
        assert_eq!(spec.validators.len(), 2);
        assert_eq!(spec.config.faucet, None);
        assert_eq!(spec.accounts.len(), 1, "alice, and no faucet");
        // The secrets reload into the keys the entry names.
        let secrets = load_validator(&dir.join("node-0")).unwrap();
        let entry = &genesis.validators[0].entry;
        assert_eq!(secrets.keys.id, entry.id());
        assert_eq!(secrets.network.public(), &entry.record.record.network_key);
        assert_eq!(secrets.beacon.tip(), entry.beacon_tip);
        // Keys are generated once.
        assert!(generate_validator(
            &dir.join("node-0"),
            "sion-test-net",
            Vec::new(),
            entry.operator
        )
        .is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_genesis_refuses_forged_duplicated_or_foreign_validators() {
        let (dir, genesis) = network("refuses", 2);
        let mut duplicated = genesis.clone();
        duplicated.validators.push(genesis.validators[0].clone());
        assert!(duplicated.check().is_err());

        let mut moved = genesis.clone();
        moved.validators[1].entry.record.record.addresses = vec!["10.0.0.9:1".into()];
        assert!(
            moved.check().is_err(),
            "the record's signature covers its addresses"
        );

        let mut foreign = genesis.clone();
        foreign.chain_id = "another-net".into();
        assert!(foreign.check().is_err());

        let mut poor = genesis.clone();
        poor.validators[0].stake = 1;
        assert!(poor.check().is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn peers_are_the_latest_records_and_the_listed_gateways() {
        let (dir, genesis) = network("peers", 2);
        let gateway = generate_gateway(&dir.join("gateway"), "gateway").unwrap();
        // Validator 1 moves, with a newer record signed by its keys.
        let secrets = load_validator(&dir.join("node-1")).unwrap();
        let mut moved = genesis.validators[1].entry.record.record.clone();
        moved.addresses = vec!["127.0.0.1:9999".into()];
        moved.sequence = 2;
        let moved = SignedRecord::sign(moved, &secrets.keys.keys).unwrap();
        let file = PeerFile {
            validators: vec![moved],
            nodes: vec![gateway.clone()],
        };
        let peers = network_peers(&genesis, &file).unwrap();
        assert_eq!(peers.len(), 3);
        let one = peers
            .iter()
            .find(|p| p.node == NodeId::Validator(genesis.validators[1].entry.id()))
            .unwrap();
        assert_eq!(one.addresses, vec!["127.0.0.1:9999".parse().unwrap()]);
        let node = peers.iter().find(|p| p.node == gateway.node()).unwrap();
        assert!(node.addresses.is_empty());
        assert_eq!(
            load_network_key(&dir.join("gateway")).unwrap().public(),
            &gateway.network_key
        );
        // A forged record is refused.
        let mut forged = file.clone();
        forged.validators[0].record.addresses = vec!["10.6.6.6:1".into()];
        assert!(network_peers(&genesis, &forged).is_err());
        let _ = fs::remove_dir_all(&dir);
    }
}
