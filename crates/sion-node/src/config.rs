//! Devnet configuration.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use sion_consensus::CoreConfig;
use sion_exec::GenesisAccount;

use crate::keys::SignatureMode;

/// How to run a devnet.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DevnetConfig {
    /// Chain identifier.
    pub chain_id: String,
    /// Validators in the genesis active set.
    pub validators: usize,
    /// Standby validators: they run from the start and follow the chain as
    /// observers, and can join the active set by staking.
    pub standby: usize,
    /// Each genesis validator's stake, in USD.
    pub validator_stake: u64,
    /// The least stake a validator may register with, in USD.
    pub min_stake: u64,
    /// Pulses per epoch (0: one epoch that never ends).
    pub epoch_length: u64,
    /// Pulses between leaving the active set and withdrawing stake.
    pub unbonding: u64,
    /// Number of lane shards (certification committees for owned traffic).
    pub shards: u16,
    /// Seats per shard committee, sampled from each epoch's active set
    /// (capped at its size).
    pub shard_seats: usize,
    /// Consensus rounds per Pulse.
    pub rounds_per_pulse: u64,
    /// How often validators seal lane approval batches.
    pub batch_interval_ms: u64,
    /// Validator signatures.
    pub signatures: SignatureMode,
    /// Where validators keep their logs and snapshots (`None`: memory only).
    pub data_dir: Option<PathBuf>,
    /// Take a snapshot every this many Pulses.
    pub snapshot_every: u64,
    /// Whether to fsync every log record.
    pub fsync: bool,
    /// Extra genesis accounts (the faucet and named demo accounts are added
    /// automatically).
    pub accounts: Vec<GenesisAccount>,
    /// Named demo accounts funded at genesis with `demo_usd` each.
    pub demo_accounts: Vec<String>,
    /// USD each demo account starts with.
    pub demo_usd: u64,
    /// Consensus parameters.
    pub core: CoreConfig,
    /// Transactions per proof chunk; `None` disables proving.
    pub prove_chunk: Option<usize>,
    /// Pulses of history (outputs, data squares, traces) kept in memory.
    pub history: usize,
    /// Pulses a shard has to execute (or show invalid) a forced intent.
    pub forced_window: u64,
    /// How long a validator with nothing to order waits before its next
    /// header ([`sion_consensus::Core::pace_when_idle`]; 0: it does not), so
    /// that a chain at rest uses little CPU.
    #[serde(default)]
    pub idle_round_ms: u64,
}

impl Default for DevnetConfig {
    fn default() -> Self {
        DevnetConfig {
            chain_id: "sion-devnet-1".into(),
            validators: 4,
            standby: 0,
            validator_stake: 100_000,
            min_stake: 10_000,
            epoch_length: 0,
            unbonding: 20,
            shards: 2,
            shard_seats: 4,
            rounds_per_pulse: 4,
            batch_interval_ms: 25,
            signatures: SignatureMode::DualPostQuantum,
            data_dir: None,
            snapshot_every: 50,
            fsync: false,
            accounts: Vec::new(),
            demo_accounts: Vec::new(),
            demo_usd: 100_000,
            core: CoreConfig {
                leader_timeout_ms: 400,
                resend_interval_ms: 600,
                gc_depth: 50,
                max_items_per_header: 2_000,
                max_payload_bytes: 512 * 1024,
                max_rounds_ahead: 64,
                max_pending_headers: 4_096,
                repropose_after_rounds: 8,
                leader_schedule: sion_consensus::LeaderSchedule::RoundRobin,
            },
            prove_chunk: Some(16),
            history: 256,
            forced_window: 8,
            idle_round_ms: 0,
        }
    }
}
