//! `sion`: run a Siontology devnet and use it.

use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};

use clap::{Args, Parser, Subcommand};
use serde_json::Value;
use sion_assurance::{AtomClass, Policy, Ratio};
use sion_cli::demos::{self, Narrator};
use sion_cli::ledger::{HttpLedger, Ledger, LedgerExt, TxState};
use sion_cli::session::{create_wallet, stream_dir, wallet_dir, wallet_for, Session};
use sion_cli::stream::{
    default_spacing, format_jump, load_stream, parse_jump, standing, standing_by_jump, PayerBook,
    PayerRecord, Payment, Standing,
};
use sion_cli::{mcp, verify, CliError};
use sion_core::hash::Hash32;
use sion_core::ids::{AssetId, AtomId, ClaimId, ObjectId, Pulse, TxDigest};
use sion_core::Amount;
use sion_exec::charter::Caveats;
use sion_exec::claim::{claim_id, formulas, ClaimBody, Payload};
use sion_exec::execute::bond_atom;
use sion_exec::market::{BatchStatus, OrderTerms, Submission};
use sion_exec::{Event, ObjectData, TxKind};
use sion_node::{Devnet, DevnetConfig, SignatureMode};

#[derive(Parser)]
#[command(
    name = "sion",
    version,
    about = "Siontology: a neo-blockchain devnet and client",
    long_about = None,
    after_help = "With no command, sion talks with your agent in a new conversation (`sion setup` makes one); `sion -c` continues the last."
)]
struct Cli {
    /// The node's API.
    #[arg(
        long,
        global = true,
        env = "SION_NODE",
        default_value = "http://127.0.0.1:8730"
    )]
    node: String,
    /// With no command: continue the last conversation, rather than start
    /// one.
    #[arg(short = 'c', long = "continue")]
    continue_last: bool,
    #[command(subcommand)]
    command: Option<Command>,
}

/// What runs: the node, and the command (bare `sion` made one).
struct Run {
    node: String,
    command: Command,
}

#[derive(Args, Clone)]
struct DevnetArgs {
    /// Genesis validators (the first active set).
    #[arg(long, default_value_t = 4)]
    validators: usize,
    /// Standby validators: they run from the start, follow the chain without
    /// a seat, and join the active set when their operator stakes for them
    /// (`operator-<index>`).
    #[arg(long, default_value_t = 0)]
    standby: usize,
    /// Pulses per epoch: committees change at epoch boundaries (0: one epoch
    /// that never ends).
    #[arg(long, default_value_t = 0)]
    epoch_length: u64,
    /// Pulses between leaving the active set and withdrawing stake.
    #[arg(long, default_value_t = 20)]
    unbonding: u64,
    /// Pulses a shard has to execute a forced intent before its committee is
    /// replaced.
    #[arg(long, default_value_t = 8)]
    forced_window: u64,
    /// Lane shards.
    #[arg(long, default_value_t = 2)]
    shards: u16,
    /// Seats per shard committee.
    #[arg(long, default_value_t = 4)]
    seats: usize,
    /// Consensus rounds per Pulse.
    #[arg(long, default_value_t = 4)]
    rounds_per_pulse: u64,
    /// Keep logs and snapshots here (restarts resume from them).
    #[arg(long)]
    data_dir: Option<std::path::PathBuf>,
    /// Where to serve the API.
    #[arg(long, default_value = "127.0.0.1:8730")]
    api: String,
    /// INSECURE: replace dual post-quantum validator signatures with a fast
    /// simulation MAC (load experiments only).
    #[arg(long)]
    insecure_fast_signatures: bool,
    /// Do not run the prover.
    #[arg(long)]
    no_prove: bool,
    /// When a validator has nothing to order, wait this long before its next
    /// round (0: never): a chain at rest then uses little CPU. A
    /// transaction, or another validator's header, starts the round at once,
    /// and the chain goes at full pace until the Pulse holding it closes.
    #[arg(long, default_value_t = 0, value_name = "MS")]
    idle_round_ms: u64,
    /// Transactions per proof chunk.
    #[arg(long, default_value_t = 16)]
    chunk: usize,
    /// Named accounts funded at genesis.
    #[arg(long, value_delimiter = ',', default_value = "alice,bob,carol")]
    accounts: Vec<String>,
    /// Stop after this many seconds (0: run until interrupted).
    #[arg(long, default_value_t = 0)]
    duration: u64,
    /// Print status every this many seconds (0: silent).
    #[arg(long, default_value_t = 5)]
    report: u64,
}

impl DevnetArgs {
    fn config(&self) -> DevnetConfig {
        DevnetConfig {
            validators: self.validators,
            standby: self.standby,
            epoch_length: self.epoch_length,
            unbonding: self.unbonding,
            forced_window: self.forced_window,
            shards: self.shards,
            shard_seats: self.seats,
            rounds_per_pulse: self.rounds_per_pulse,
            signatures: if self.insecure_fast_signatures {
                SignatureMode::InsecureFast
            } else {
                SignatureMode::DualPostQuantum
            },
            data_dir: self.data_dir.clone(),
            demo_accounts: self.accounts.clone(),
            prove_chunk: (!self.no_prove).then_some(self.chunk.max(1)),
            idle_round_ms: self.idle_round_ms,
            ..DevnetConfig::default()
        }
    }
}

#[derive(Subcommand)]
enum Command {
    /// Run a devnet and serve its API.
    Devnet(DevnetArgs),
    /// Collect validators' entries into a network's genesis file.
    Genesis(sion_cli::network::GenesisArgs),
    /// Run one validator of a network, in this process.
    Node(sion_cli::network::NodeArgs),
    /// Run a gateway of a network, in this process.
    Gateway(sion_cli::network::GatewayArgs),
    /// Chain status.
    Status,
    /// Set up Siontology for one person on this machine: a chain of your
    /// own (a service of yours), your wallet and your agent's, the agent
    /// with a budget and its memory, its model, a profile `sion` talks
    /// with, and Telegram if you like. Safe to run again.
    Setup(sion_cli::setup::SetupArgs),
    /// Replace this sion with the latest release, once its signature and
    /// SHA-256 are checked.
    Update(sion_cli::update::UpdateArgs),
    /// Put your agents on a server of yours over SSH, so that they work
    /// while this computer is off: sion copied there, set up there (or
    /// this machine's home copied, with --copy-home), and everything run
    /// by one service of yours (sion agent up).
    Deploy(sion_cli::deploy::DeployArgs),
    /// What this machine has for Siontology and its agents, and what is
    /// missing: the home, the node, sandboxes, the browser and desktop,
    /// models', speech services' and messengers' credentials (by name).
    Doctor,
    /// Wallet keys.
    #[command(subcommand)]
    Keys(KeysCommand),
    /// An account's balances and coins.
    Balance {
        /// Wallet name or account ID.
        account: String,
    },
    /// Devnet faucet.
    Faucet {
        /// Wallet name or account ID.
        account: String,
        /// Amount (decimal).
        amount: String,
        /// Asset: usd, a name such as gpu-hour, or a hex ID.
        #[arg(long, default_value = "usd")]
        asset: String,
    },
    /// Transfer.
    Send {
        /// Sending wallet.
        from: String,
        /// Recipient (wallet name or account ID).
        to: String,
        /// Amount (decimal).
        amount: String,
        /// Asset.
        #[arg(long, default_value = "usd")]
        asset: String,
        /// Post it as a forced intent, straight to consensus, bypassing the
        /// gateway (for when your transactions are being ignored).
        #[arg(long)]
        forced: bool,
    },
    /// Forced-inclusion obligations: intents posted directly to consensus
    /// that their shard has not yet executed, and any that are overdue.
    Forced,
    /// ADVERSARY SIMULATION (devnet only): censor accounts at the gateway
    /// and/or at the first N validators. No accounts: stop censoring.
    Censor {
        /// Wallet names or account IDs to censor.
        accounts: Vec<String>,
        /// The gateway drops their transactions silently.
        #[arg(long)]
        gateway: bool,
        /// This many validators ignore them too (more than a third of a
        /// committee can stall certification).
        #[arg(long, default_value_t = 0)]
        validators: usize,
    },
    /// Post a bond behind your bonded atom `bond:<you>:<label>`.
    Bond {
        /// Wallet.
        from: String,
        /// Label.
        label: String,
        /// USD.
        amount: String,
        /// economic, adjudicator or institutional.
        #[arg(long, default_value = "economic")]
        class: String,
    },
    /// Claims.
    #[command(subcommand)]
    Claim(ClaimCommand),
    /// Rely on a claim with insurance: reserve κ·V of its backing.
    Accept {
        /// Wallet.
        from: String,
        /// Claim ID.
        claim: String,
        /// Value at risk (USD).
        value: String,
        /// Pulses of reliance.
        #[arg(long, default_value_t = 1_000)]
        pulses: u64,
        /// Safety factor κ.
        #[arg(long, default_value_t = 2.0)]
        kappa: f64,
    },
    /// Challenge a bonded atom a claim rests on.
    Challenge {
        /// Wallet.
        from: String,
        /// Claim ID.
        claim: String,
        /// Atom ID.
        atom: String,
        /// Deposit (USD).
        deposit: String,
        /// Evidence (hashed and recorded).
        #[arg(long, default_value = "")]
        evidence: String,
    },
    /// Rule on a challenge as the claim's adjudicator.
    Rule {
        /// Adjudicator's wallet.
        from: String,
        /// Challenge ID.
        challenge: String,
        /// Whether the challenged atom is broken.
        #[arg(long)]
        upheld: bool,
    },
    /// Agent Charters.
    #[command(subcommand)]
    Charter(CharterCommand),
    /// Agent identities: register agents, launch subagents with their own
    /// identities, suspend, resume and revoke whole subtrees.
    #[command(subcommand)]
    Agent(Box<sion_cli::agents::AgentCommand>),
    /// Task Charters: escrow work whose acceptance rule is fixed when it is
    /// posted; attest, submit and settle results; cancel.
    #[command(subcommand)]
    Task(sion_cli::tasks::TaskCommand),
    /// A market for proofs of Pulses: ask for a Pulse's World Proof with a
    /// payment in escrow, deliver one, and attest one as a verifier after
    /// checking it under both proof stacks
    #[command(subcommand)]
    Proving(sion_cli::proving::ProvingCommand),
    /// Pooled stake: delegate to a validator without handing the stake
    /// over, collect rewards, leave; set a validator's commission.
    #[command(subcommand)]
    Stake(sion_cli::stake::StakeCommand),
    /// Memory logs for agents and robots: open a log, record atoms
    /// locally, commit heads the ledger keeps append-only, and recall with
    /// proofs of completeness.
    #[command(subcommand)]
    Memory(sion_cli::memory::MemoryCommand),
    /// Skills with a supply chain: pack a skill, audit a bundle, publish a
    /// release on bonds and audits, show one, install one for an agent under
    /// the decision rule; evaluate a skill, evolve it, and have a better
    /// version verified by others through a bounty.
    #[command(subcommand)]
    Skill(sion_cli::skills::SkillCommand),
    /// Storage deals: have data kept by a provider who proves every round
    /// that it still holds it, or provide storage and be paid for it.
    #[command(subcommand)]
    Storage(sion_cli::storage::StorageCommand),
    /// Markets.
    #[command(subcommand)]
    Market(MarketCommand),
    /// Payment streams: escrow once, pay per request with one hash each,
    /// settle thousands of payments in one transaction.
    #[command(subcommand)]
    Stream(StreamCommand),
    /// A transaction's status and effects.
    Tx {
        /// Digest.
        digest: String,
    },
    /// A transaction's receipt, verified as a light client would.
    Receipt {
        /// Digest.
        digest: String,
        /// The chain identity you trust. Without it the node vouches for itself.
        #[arg(long, env = "SION_CHAIN")]
        chain: Option<String>,
    },
    /// A Pulse's header, certificate and summary.
    Pulse {
        /// Pulse (default: latest).
        pulse: Option<u64>,
    },
    /// Recent events.
    Events {
        /// From Pulse.
        #[arg(long, default_value_t = 0)]
        from: u64,
        /// Most events.
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    /// Verify a Pulse as a light client.
    Verify {
        /// The chain identity you trust (from `sion devnet`'s banner or another
        /// source you trust). Without it the node vouches for itself.
        #[arg(long, env = "SION_CHAIN")]
        chain: Option<String>,
        /// Pulse (default: latest certified).
        #[arg(long)]
        pulse: Option<u64>,
        /// Data-availability samples.
        #[arg(long, default_value_t = 16)]
        samples: usize,
        /// Skip the World Proof.
        #[arg(long)]
        no_world: bool,
    },
    /// Run a scripted demo: invoice, agents, swarm, batch, tasks, robots, storage, market, receipts, x402, streams, censorship, light, staking or all.
    Demo {
        /// Which demo.
        #[arg(default_value = "all")]
        name: String,
        /// Start an in-process devnet instead of using --node.
        #[arg(long)]
        local: bool,
    },
    /// Serve the Model Context Protocol on stdio (for AI agents): the
    /// ledger's tools, or a person's mail, calendars or documents.
    Mcp {
        /// Serve this instead of the ledger.
        #[command(subcommand)]
        server: Option<McpServe>,
    },
    /// Sell or buy HTTP resources with x402 (HTTP 402 payments settled on Siontology).
    #[command(subcommand)]
    X402(X402Command),
    /// Measure primitives, execution, proving, clearing and a devnet.
    Bench {
        /// Smaller sizes.
        #[arg(long)]
        quick: bool,
        /// Skip the end-to-end devnet measurement.
        #[arg(long)]
        no_devnet: bool,
        /// Run only these groups: signatures, authorization, state, data,
        /// clearing, assurance, execution, streams, lineage, devnet
        /// (comma-separated).
        #[arg(long, value_delimiter = ',')]
        only: Vec<String>,
        /// Write the measurements as JSON to this file.
        #[arg(long)]
        json: Option<std::path::PathBuf>,
    },
}

#[derive(Subcommand)]
enum X402Command {
    /// Serve one resource for a price per request, paid to PAY_TO.
    Serve {
        /// Who gets paid (wallet name or account ID).
        pay_to: String,
        /// Listen address.
        #[arg(long, default_value = "127.0.0.1:8402")]
        listen: String,
        /// The resource's path.
        #[arg(long, default_value = "/v1/resource")]
        path: String,
        /// Price per request (USD).
        #[arg(long, default_value = "0.25")]
        price: String,
        /// What every paid request returns.
        #[arg(long, default_value = "{\"paid\":true}")]
        body: String,
        /// The chain identity receipts must verify against. Without it the
        /// node vouches for itself.
        #[arg(long, env = "SION_CHAIN")]
        chain: Option<String>,
        /// Also accept payments from streams (scheme `stream`), settled by
        /// PAY_TO's wallet, which must be a wallet name.
        #[arg(long)]
        streams: bool,
        /// With --streams: settle once a stream has this many unsettled steps.
        #[arg(long, default_value_t = 1_000)]
        settle_every: u64,
        /// With --streams: settle a stream's payments at the latest this many
        /// seconds after the first unsettled one.
        #[arg(long, default_value_t = 30)]
        settle_after: u64,
    },
    /// Fetch a URL, paying through x402 if the server asks.
    Fetch {
        /// The URL.
        url: String,
        /// The paying wallet (the agent's, when paying under a charter; not
        /// needed with --stream).
        #[arg(long, required_unless_present = "stream")]
        wallet: Option<String>,
        /// Pay under this Agent Charter (ID) instead of by transfer.
        #[arg(long)]
        charter: Option<String>,
        /// The agent's allotment (ID), with --charter.
        #[arg(long)]
        allotment: Option<String>,
        /// Pay from this payment stream (ID) instead: one chain element per
        /// request, no transaction.
        #[arg(long, conflicts_with_all = ["charter", "allotment"])]
        stream: Option<String>,
        /// Fetch this many times (with --stream, every request after the
        /// first is paid up front).
        #[arg(long, default_value_t = 1)]
        count: u32,
        /// The most to pay (USD).
        #[arg(long, default_value = "1")]
        max: String,
        /// The chain identity to pay on. Without it the node vouches for itself.
        #[arg(long, env = "SION_CHAIN")]
        chain: Option<String>,
    },
}

#[derive(Subcommand)]
enum StreamCommand {
    /// Open a stream: escrow funds for one payee and keep the chain that
    /// pays it (its secret seed goes under $SION_HOME/streams).
    Open {
        /// The paying wallet.
        payer: String,
        /// Who is paid (wallet name or account ID).
        payee: String,
        /// The value of one step (USD).
        #[arg(long, default_value = "0.001")]
        unit: String,
        /// Steps in the chain: the most the stream can ever pay is
        /// unit × steps.
        #[arg(long, default_value_t = 100_000)]
        steps: u64,
        /// Escrow (USD).
        #[arg(long, default_value = "10")]
        deposit: String,
        /// Pulses the payee has to settle (after them the payer may close
        /// the stream and take back what is left).
        #[arg(long, default_value_t = 10_000)]
        pulses: u64,
        /// Steps between checkpoints, which let the payee settle any number
        /// of steps for a bounded number of hashes (default: every 64 steps,
        /// fewer for very long chains; 0: no checkpoints).
        #[arg(long)]
        spacing: Option<u64>,
    },
    /// Pay from a stream: prints the next payment, `<stream>:<position>:<element>`,
    /// to give to the payee, and the jump for its checkpoint when it is the
    /// first payment past one.
    Pay {
        /// The stream.
        stream: String,
        /// Steps to pay.
        #[arg(long, default_value_t = 1)]
        steps: u64,
    },
    /// Print the jump for the checkpoint below a position (as the payer).
    Jump {
        /// The stream.
        stream: String,
        /// The position (default: the last one revealed).
        #[arg(long)]
        position: Option<u64>,
    },
    /// Check a payment against the stream's settled head (anyone can).
    Check {
        /// The payment.
        payment: String,
        /// Check it by this jump instead of walking from the head.
        #[arg(long)]
        jump: Option<String>,
    },
    /// Settle as the payee: everything up to the latest payment received.
    Settle {
        /// The payee's wallet.
        payee: String,
        /// The latest payment received.
        payment: String,
        /// Prove it by this jump (from the payer) instead of walking from
        /// the head.
        #[arg(long)]
        jump: Option<String>,
    },
    /// Add escrow and/or time, as the payer.
    TopUp {
        /// The payer's wallet.
        payer: String,
        /// The stream.
        stream: String,
        /// Escrow to add (USD).
        #[arg(long, default_value = "0")]
        deposit: String,
        /// Pulses to add to the stream's last.
        #[arg(long, default_value_t = 0)]
        pulses: u64,
    },
    /// Close a stream: the payee at any time, the payer after its last
    /// Pulse. What is left in escrow goes back to the payer.
    Close {
        /// Who closes.
        who: String,
        /// The stream.
        stream: String,
    },
    /// Show a stream.
    Show {
        /// The stream.
        stream: String,
    },
}

#[derive(Subcommand)]
enum KeysCommand {
    /// Create a wallet with a random seed.
    New {
        /// Name.
        name: String,
    },
    /// Show a wallet's account and public keys.
    Show {
        /// Name.
        name: String,
    },
    /// Generate a network validator's keys and its signed network record.
    Validator(sion_cli::network::ValidatorKeysArgs),
    /// Generate a network gateway's key.
    Gateway(sion_cli::network::GatewayKeysArgs),
}

#[derive(Subcommand)]
enum ClaimCommand {
    /// Issue a claim.
    Issue {
        /// Issuer's wallet.
        from: String,
        /// One-line statement.
        subject: String,
        /// Schema.
        #[arg(long, default_value = "note/v1")]
        schema: String,
        /// Rest it on your bond of this label.
        #[arg(long)]
        bond: Option<String>,
        /// Adjudicator atom ID.
        #[arg(long)]
        adjudicator: Option<String>,
        /// Validity in Pulses.
        #[arg(long, default_value_t = 10_000)]
        valid_for: u64,
    },
    /// Show a claim, its status and cost of corruption.
    Show {
        /// Claim ID.
        claim: String,
    },
    /// Apply the decision rule.
    Decide {
        /// Claim ID.
        claim: String,
        /// Value at risk (USD).
        value: String,
        /// κ.
        #[arg(long, default_value_t = 2.0)]
        kappa: f64,
        /// Atoms to distrust (hex IDs).
        #[arg(long, value_delimiter = ',')]
        distrust: Vec<String>,
    },
}

#[derive(Subcommand)]
enum CharterCommand {
    /// Charter an agent.
    Create {
        /// Principal's wallet.
        principal: String,
        /// Agent (wallet or account).
        agent: String,
        /// Allowed payees (comma-separated; empty: any).
        #[arg(long, value_delimiter = ',')]
        payees: Vec<String>,
        /// Largest payment (USD).
        #[arg(long, default_value = "100")]
        max_payment: String,
        /// The most the agent may spend at once: its bucket's capacity,
        /// full when it starts (USD).
        #[arg(long, default_value = "500")]
        burst: String,
        /// What refills its bucket each Pulse (USD).
        #[arg(long, default_value = "0.5")]
        rate: String,
        /// Budget (USD).
        #[arg(long, default_value = "1000")]
        funding: String,
        /// Bond behind the agent's receipts (USD).
        #[arg(long, default_value = "0")]
        bond: String,
        /// Charter lifetime (Pulses).
        #[arg(long, default_value_t = 100_000)]
        lifetime: u64,
    },
    /// Pay as the agent.
    Pay {
        /// Agent's wallet.
        agent: String,
        /// Charter ID.
        charter: String,
        /// Allotment ID.
        allotment: String,
        /// Payee.
        payee: String,
        /// USD.
        amount: String,
        /// Purpose.
        #[arg(long, default_value = "")]
        purpose: String,
    },
    /// Revoke.
    Revoke {
        /// Principal's wallet.
        principal: String,
        /// Charter ID.
        charter: String,
    },
}

#[derive(Subcommand)]
enum MarketCommand {
    /// Open a market.
    Create {
        /// Operator's wallet.
        operator: String,
        /// Name.
        label: String,
        /// The good (asset name or ID).
        #[arg(long, default_value = "gpu-hour")]
        good: String,
        /// Pulses per batch.
        #[arg(long, default_value_t = 6)]
        batch: u64,
        /// Pulses allowed for clearing.
        #[arg(long, default_value_t = 4)]
        deadline: u64,
    },
    /// Submit a plain order.
    Order {
        /// Wallet.
        from: String,
        /// Market ID.
        market: String,
        /// buy or sell.
        side: String,
        /// Units.
        quantity: u64,
        /// Limit price per unit (USD).
        price: String,
    },
    /// Clear the latest sealed batch with a certificate.
    Clear {
        /// Solver's wallet.
        solver: String,
        /// Market ID.
        market: String,
    },
    /// Show a market.
    Show {
        /// Market ID.
        market: String,
    },
}

fn parse_amount(text: &str) -> Result<Amount, CliError> {
    let text = text.trim();
    let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
    if whole.is_empty() && fraction.is_empty() || fraction.len() > 6 {
        return Err(CliError::Usage(format!(
            "{text:?} is not an amount with at most 6 decimals"
        )));
    }
    let whole: u128 = if whole.is_empty() {
        0
    } else {
        whole
            .parse()
            .map_err(|_| CliError::Usage(format!("{text:?} is not an amount")))?
    };
    let mut padded = fraction.to_string();
    while padded.len() < 6 {
        padded.push('0');
    }
    let fraction: u128 = padded
        .parse()
        .map_err(|_| CliError::Usage(format!("{text:?} is not an amount")))?;
    Ok(Amount(whole * 1_000_000 + fraction))
}

fn parse_asset(text: &str) -> AssetId {
    match text {
        "usd" | "USD" => AssetId::usd(),
        other => match Hash32::from_hex(other) {
            Ok(hash) => AssetId::from_hash(hash),
            Err(_) => AssetId::derive(other.as_bytes()),
        },
    }
}

fn parse_chain(chain: Option<String>) -> Result<Option<Hash32>, CliError> {
    chain
        .map(|hex| {
            Hash32::from_hex(hex.trim())
                .map_err(|_| CliError::Usage("--chain takes 64 hex digits".into()))
        })
        .transpose()
}

fn hex_hash(text: &str, what: &str) -> Result<Hash32, CliError> {
    Hash32::from_hex(text).map_err(|_| CliError::Usage(format!("{what} must be a 64-digit hex ID")))
}

fn kappa(value: f64) -> Ratio {
    Ratio::new((value * 1000.0).round().max(1.0) as u64, 1000)
}

fn print(value: &Value) {
    println!(
        "{}",
        serde_json::to_string_pretty(value).unwrap_or_default()
    );
}

fn show_tx(names: &sion_cli::describe::Names, view: &sion_node::TxView) {
    println!(
        "Pulse {}: {}",
        view.pulse,
        sion_cli::describe::receipt(names, &view.effects)
    );
    println!("  digest {}", view.effects.digest);
}

fn run_devnet(args: &DevnetArgs) -> Result<(), CliError> {
    let config = args.config();
    if config.signatures == SignatureMode::InsecureFast {
        eprintln!(
            "WARNING: validators use an INSECURE simulation MAC (--insecure-fast-signatures)"
        );
    }
    let started = Instant::now();
    let devnet = Arc::new(Devnet::start(config).map_err(|e| CliError::Node(e.to_string()))?);
    let api = sion_node::api::serve(devnet.clone(), &args.api)
        .map_err(|e| CliError::Node(e.to_string()))?;
    println!(
        "Siontology devnet: {} validators, {} shards, API on http://{}",
        args.validators, args.shards, api.address,
    );
    println!(
        "chain identity (pin it: sion verify --chain …): {}",
        devnet.shared.chain_id.to_hex()
    );
    println!("accounts funded at genesis: {}", args.accounts.join(", "));
    let mut last_report = Instant::now();
    loop {
        std::thread::sleep(Duration::from_millis(200));
        if args.report > 0 && last_report.elapsed() >= Duration::from_secs(args.report) {
            last_report = Instant::now();
            if let Ok(status) = devnet.query(sion_node::Query::Status) {
                println!(
                    "Pulse {:>5}  certified {:>5}  round {:>5}  executed {}  claims {}  objects {}  proven {}",
                    status["pulse"],
                    status["certified_pulse"],
                    status["round"],
                    status["executed"],
                    status["claims"],
                    status["objects"],
                    status["proven_pulse"]
                );
            }
        }
        if args.duration > 0 && started.elapsed() >= Duration::from_secs(args.duration) {
            break;
        }
    }
    api.stop();
    if let Ok(devnet) = Arc::try_unwrap(devnet) {
        devnet.shutdown();
    }
    Ok(())
}

fn run(cli: Cli) -> Result<(), CliError> {
    let command = match (cli.command, cli.continue_last) {
        (Some(_), true) => {
            return Err(CliError::Usage(
                "-c continues the last conversation: give it with no command".into(),
            ))
        }
        (Some(command), false) => command,
        // Bare `sion`: talk with your agent.
        (None, continuing) => {
            Command::Agent(Box::new(sion_cli::talk::bare(&cli.node, continuing)?))
        }
    };
    let mut cli = Run {
        node: cli.node,
        command,
    };
    // Agent commands that need no node do not wait for one.
    if let Command::Agent(command) = cli.command {
        match sion_cli::agents::local(command) {
            Ok(done) => return done,
            Err(command) => cli.command = Command::Agent(command),
        }
    }
    match &cli.command {
        Command::Doctor => return sion_cli::doctor::run(&cli.node),
        Command::Setup(args) => return sion_cli::setup::run(&cli.node, args),
        Command::Update(args) => return sion_cli::update::run(args),
        Command::Deploy(args) => {
            for line in sion_cli::deploy::deploy(args)? {
                println!("{line}");
            }
            return Ok(());
        }
        Command::Devnet(args) => return run_devnet(args),
        Command::Genesis(args) => return sion_cli::network::genesis(args).map(|_| ()),
        Command::Node(args) => return sion_cli::network::run_node(args),
        Command::Gateway(args) => return sion_cli::network::run_gateway(args),
        _ => {}
    }
    if let Command::Keys(command) = &cli.command {
        return match command {
            KeysCommand::Validator(args) => sion_cli::network::validator_keys(args).map(|_| ()),
            KeysCommand::Gateway(args) => sion_cli::network::gateway_keys(args).map(|_| ()),
            KeysCommand::New { name } => {
                let wallet = create_wallet(name)?;
                println!("created {name}: account {}", wallet.id);
                println!(
                    "seed stored in {}",
                    wallet_dir().join(format!("{name}.seed")).display()
                );
                Ok(())
            }
            KeysCommand::Show { name } => {
                let wallet = wallet_for(name);
                println!("account {}", wallet.id);
                let (lattice, hash) = wallet.keys.public_keys();
                println!("ML-DSA-44 key   {}", lattice.fingerprint());
                println!("SLH-DSA key     {}", hash.fingerprint());
                Ok(())
            }
        };
    }
    if let Command::Bench {
        quick,
        no_devnet,
        only,
        json,
    } = &cli.command
    {
        let results = sion_cli::bench::run(*quick, !*no_devnet, only, true)?;
        if let Some(path) = json {
            std::fs::write(
                path,
                serde_json::to_string_pretty(&results).unwrap_or_default(),
            )
            .map_err(|e| CliError::Usage(e.to_string()))?;
        }
        return Ok(());
    }
    if let Command::Demo { name, local: true } = &cli.command {
        // Each demo runs on the devnet it needs; consecutive demos that need
        // the same one share it (the staking demo needs epochs that end).
        let names: Vec<&str> = if name == "all" {
            demos::ALL.to_vec()
        } else {
            vec![name.as_str()]
        };
        let mut out = Narrator::printing();
        let mut index = 0;
        while index < names.len() {
            let staking = demos::needs_epochs(names[index]);
            let devnet = Devnet::start(demos::local_config(names[index]))
                .map_err(|e| CliError::Node(e.to_string()))?;
            let result = {
                let mut session = Session::open(&devnet)?;
                let mut result = Ok(());
                while index < names.len() && demos::needs_epochs(names[index]) == staking {
                    if index > 0 {
                        out.say("");
                    }
                    result = demos::run(names[index], &mut session, &mut out);
                    index += 1;
                    if result.is_err() {
                        break;
                    }
                }
                result
            };
            devnet.shutdown();
            result?;
        }
        return Ok(());
    }
    // The first-party MCP servers need no node.
    if let Command::Mcp {
        server: Some(server),
    } = &cli.command
    {
        return match server {
            McpServe::Mail => {
                let mut mail = sion_cli::email::MailServer::from_env().map_err(CliError::Usage)?;
                sion_cli::mcp_serve::serve(&mut mail)
            }
            McpServe::Calendar => {
                let mut calendars =
                    sion_cli::calendar::Calendars::from_env().map_err(CliError::Usage)?;
                sion_cli::mcp_serve::serve(&mut calendars)
            }
            McpServe::Documents { dir } => {
                let mut documents =
                    sion_cli::documents::Documents::at(dir).map_err(CliError::Usage)?;
                sion_cli::mcp_serve::serve(&mut documents)
            }
        };
    }
    // Packing, evaluating and evolving a skill need no node.
    if let Command::Skill(command) = &cli.command {
        if let Some(result) = sion_cli::skills::run_offline(command) {
            return result;
        }
    }
    let http = HttpLedger::new(&cli.node);
    let ledger: &dyn Ledger = &http;
    if let Command::Mcp { .. } = &cli.command {
        return mcp::serve(ledger);
    }
    let mut session = Session::open(ledger)?;
    session.streams = PayerBook::at(stream_dir());
    // Show what each command signs, as a sentence (demos narrate instead).
    session.verbose = !matches!(cli.command, Command::Demo { .. });
    match cli.command {
        Command::Devnet(_)
        | Command::Genesis(_)
        | Command::Node(_)
        | Command::Gateway(_)
        | Command::Keys(_)
        | Command::Mcp { .. }
        | Command::Doctor
        | Command::Setup(_)
        | Command::Update(_)
        | Command::Deploy(_)
        | Command::Bench { .. } => {
            unreachable!("handled above")
        }
        Command::X402(X402Command::Serve {
            pay_to,
            listen,
            path,
            price,
            body,
            chain,
            streams,
            settle_every,
            settle_after,
        }) => {
            use sion_cli::x402::{Facilitator, PaidRoute, PaidServer};
            let pinned = parse_chain(chain)?;
            let chain = match pinned {
                Some(chain) => chain,
                None => {
                    let served = verify::served_chain(ledger)?;
                    eprintln!(
                        "warning: chain {} is the node's word; pass --chain to pin it",
                        served.short()
                    );
                    served
                }
            };
            let desk = match streams {
                true if Hash32::from_hex(&pay_to).is_ok() => {
                    return Err(CliError::Usage(
                        "--streams settles as PAY_TO, so PAY_TO must be a wallet name".into(),
                    ))
                }
                true => {
                    let mut desk = sion_cli::stream::StreamDesk::new(wallet_for(&pay_to));
                    desk.settle_every = settle_every.max(1);
                    desk.settle_after = Duration::from_secs(settle_after);
                    Some(desk)
                }
                false => None,
            };
            let pay_to = session.id(&pay_to);
            let mut facilitator = Facilitator::new(ledger, chain)?;
            if let Some(desk) = desk {
                facilitator = facilitator.with_streams(desk);
            }
            let server = PaidServer::bind(&listen)?;
            let price = parse_amount(&price)?;
            let routes = [PaidRoute {
                path: path.clone(),
                price,
                description: format!("{path}, {price} USD per request"),
                mime_type: "application/json".into(),
                body: Box::new(move |_| body.clone()),
            }];
            println!(
                "selling http://{}{path} at {price} USD per request, paid to {} on {}{}",
                server.address,
                pay_to.short(),
                facilitator.network(),
                if facilitator.streams().is_some() {
                    " (exact transfers, or streams settled in batches)"
                } else {
                    ""
                }
            );
            let stop = std::sync::atomic::AtomicBool::new(false);
            server.serve(
                &facilitator,
                pay_to,
                &routes,
                Duration::from_secs(300),
                &stop,
            );
        }
        Command::X402(X402Command::Fetch {
            url,
            wallet,
            charter,
            allotment,
            stream,
            count,
            max,
            chain,
        }) => {
            use sion_cli::x402::{fetch_with, Payer};
            let payer = match (charter, allotment) {
                _ if stream.is_some() => Payer::Stream(ObjectId::from_hash(hex_hash(
                    stream.as_deref().unwrap_or_default(),
                    "stream",
                )?)),
                (Some(charter), Some(allotment)) => Payer::Charter {
                    agent: wallet.unwrap_or_default(),
                    charter: ObjectId::from_hash(hex_hash(&charter, "charter")?),
                    allotment: ObjectId::from_hash(hex_hash(&allotment, "allotment")?),
                },
                (None, None) => Payer::Wallet(wallet.unwrap_or_default()),
                _ => {
                    return Err(CliError::Usage(
                        "--charter and --allotment go together".into(),
                    ))
                }
            };
            let chain = match parse_chain(chain)? {
                Some(chain) => chain,
                None => verify::served_chain(ledger)?,
            };
            let max = parse_amount(&max)?;
            let mut known = None;
            for request in 0..count.max(1) {
                let fetched = fetch_with(&mut session, &payer, &url, &chain, max, known.as_ref())?;
                if let Some(quoted) = &fetched.quoted {
                    for accepted in &quoted.accepts {
                        println!(
                            "402 Payment Required: {} USD to {} on {} ({})",
                            Amount(accepted.amount.parse().unwrap_or(0)),
                            accepted.pay_to,
                            accepted.network,
                            accepted.scheme
                        );
                    }
                }
                if let Some(settlement) = &fetched.settlement {
                    let stream_position = settlement
                        .extensions
                        .as_ref()
                        .and_then(|e| e["sion"]["position"].as_u64());
                    match (settlement.success, stream_position) {
                        (true, Some(position)) => println!(
                            "paid from the stream: position {position}, checked by the server with one hash per step"
                        ),
                        (true, None) => println!(
                            "paid: transaction {} settled and verified by the server",
                            settlement.transaction
                        ),
                        (false, _) => println!(
                            "payment refused: {}",
                            settlement.error_reason.clone().unwrap_or_default()
                        ),
                    }
                }
                println!("{} {}", fetched.status, fetched.body);
                if fetched.status != 200 {
                    break;
                }
                if matches!(payer, Payer::Stream(_)) {
                    known = fetched.accepted.clone();
                } else if request + 1 < count {
                    // An exact quote pays once: ask again.
                    known = None;
                }
            }
        }
        Command::Stream(command) => stream_command(&mut session, command)?,
        Command::Status => print(&ledger.status()?),
        Command::Balance { account } => {
            let id = session.id(&account);
            let view = ledger.account(id)?;
            println!("account {id} (Pulse {})", view.pulse);
            for (asset, amount) in &view.balances {
                let name = if *asset == AssetId::usd() {
                    "USD".to_string()
                } else {
                    asset.short()
                };
                println!("  {amount} {name}");
            }
            println!(
                "  {} coins, {} allotments",
                view.coins.len(),
                view.allotments.len()
            );
        }
        Command::Faucet {
            account,
            amount,
            asset,
        } => {
            let amount = parse_amount(&amount)?;
            session.fund(&account, parse_asset(&asset), amount)?;
            println!("funded {account} with {amount}");
        }
        Command::Send {
            from,
            to,
            amount,
            asset,
            forced,
        } => {
            let to = session.id(&to);
            let kind = TxKind::Transfer {
                asset: parse_asset(&asset),
                outputs: vec![(to, parse_amount(&amount)?)],
            };
            let view = if forced {
                session.force(&from, kind, Vec::new())?
            } else {
                session.send(&from, kind, Vec::new())?
            };
            show_tx(&session.names(), &view);
        }
        Command::Forced => print(&ledger.query(sion_node::Query::Forced)?),
        Command::Censor {
            accounts,
            gateway,
            validators,
        } => {
            let ids: Vec<_> = accounts.iter().map(|a| session.id(a)).collect();
            ledger.censor(&ids, gateway, validators)?;
            if ids.is_empty() {
                println!("censorship stopped");
            } else {
                println!(
                    "ADVERSARY SIMULATION: censoring {} account(s){}{}",
                    ids.len(),
                    if gateway { " at the gateway" } else { "" },
                    if validators > 0 {
                        format!(" and at {validators} validator(s)")
                    } else {
                        String::new()
                    }
                );
            }
        }
        Command::Bond {
            from,
            label,
            amount,
            class,
        } => {
            let class = match class.as_str() {
                "economic" => AtomClass::Economic,
                "adjudicator" => AtomClass::Adjudicator,
                "institutional" => AtomClass::Institutional,
                other => return Err(CliError::Usage(format!("unknown bond class {other}"))),
            };
            let id = session.id(&from);
            let view = session.send(
                &from,
                TxKind::PostBond {
                    class,
                    label: label.clone(),
                    amount: parse_amount(&amount)?,
                },
                Vec::new(),
            )?;
            show_tx(&session.names(), &view);
            println!("atom {}", bond_atom(class, &id, &label).id);
        }
        Command::Claim(ClaimCommand::Issue {
            from,
            subject,
            schema,
            bond,
            adjudicator,
            valid_for,
        }) => {
            let id = session.id(&from);
            let formula = match &bond {
                Some(label) => {
                    formulas::bonded_statement(bond_atom(AtomClass::Economic, &id, label).id)
                }
                None => formulas::dual_signature(),
            };
            let body = ClaimBody {
                schema,
                subject: subject.clone(),
                payload: Payload::Public(subject.into_bytes()),
                formula,
                adjudicator: adjudicator
                    .map(|a| hex_hash(&a, "adjudicator").map(AtomId::from_hash))
                    .transpose()?,
                detection_permille: 1000,
                valid_for,
                challenge_period: 100,
            };
            let claim = claim_id(&id, &body);
            let view = session.send(&from, TxKind::IssueClaim { claim: body }, Vec::new())?;
            show_tx(&session.names(), &view);
            println!(
                "claim {claim} (joins the knowledge log at the end of Pulse {})",
                view.pulse
            );
        }
        Command::Claim(ClaimCommand::Show { claim }) => {
            print(&ledger.claim(ClaimId::from_hash(hex_hash(&claim, "claim")?))?);
        }
        Command::Claim(ClaimCommand::Decide {
            claim,
            value,
            kappa: k,
            distrust,
        }) => {
            let atoms: Result<Vec<AtomId>, CliError> = distrust
                .iter()
                .map(|a| hex_hash(a, "atom").map(AtomId::from_hash))
                .collect();
            let policy = Policy::standard().with_kappa(kappa(k)).distrusting(atoms?);
            print(&ledger.decide(
                ClaimId::from_hash(hex_hash(&claim, "claim")?),
                parse_amount(&value)?,
                policy,
            )?);
        }
        Command::Accept {
            from,
            claim,
            value,
            pulses,
            kappa: k,
        } => {
            let until = Pulse(ledger.pulse()?.0 + pulses);
            let view = session.send(
                &from,
                TxKind::Accept {
                    claim: ClaimId::from_hash(hex_hash(&claim, "claim")?),
                    value_at_risk: parse_amount(&value)?,
                    policy: Policy::standard().with_kappa(kappa(k)),
                    until,
                    nonce: rand_nonce(),
                },
                Vec::new(),
            )?;
            show_tx(&session.names(), &view);
        }
        Command::Challenge {
            from,
            claim,
            atom,
            deposit,
            evidence,
        } => {
            let view = session.send(
                &from,
                TxKind::Challenge {
                    claim: ClaimId::from_hash(hex_hash(&claim, "claim")?),
                    target: AtomId::from_hash(hex_hash(&atom, "atom")?),
                    evidence: sion_core::hash::hash_domain(
                        "sion/cli/evidence",
                        evidence.as_bytes(),
                    ),
                    deposit: parse_amount(&deposit)?,
                },
                Vec::new(),
            )?;
            show_tx(&session.names(), &view);
        }
        Command::Rule {
            from,
            challenge,
            upheld,
        } => {
            let view = session.send(
                &from,
                TxKind::Rule {
                    challenge: hex_hash(&challenge, "challenge")?,
                    upheld,
                },
                Vec::new(),
            )?;
            show_tx(&session.names(), &view);
        }
        Command::Agent(command) => sion_cli::agents::run(
            &mut session,
            ledger,
            std::sync::Arc::new(http.clone()),
            *command,
        )?,
        Command::Task(command) => sion_cli::tasks::run(&mut session, ledger, command)?,
        Command::Proving(command) => sion_cli::proving::run(&mut session, ledger, command)?,
        Command::Stake(command) => sion_cli::stake::run(&mut session, ledger, command)?,
        Command::Memory(command) => sion_cli::memory::run(&mut session, ledger, command)?,
        Command::Skill(command) => sion_cli::skills::run(&mut session, ledger, command)?,
        Command::Storage(command) => sion_cli::storage::run(
            &mut session,
            ledger,
            std::sync::Arc::new(http.clone()),
            command,
        )?,
        Command::Charter(CharterCommand::Create {
            principal,
            agent,
            payees,
            max_payment,
            burst,
            rate,
            funding,
            bond,
            lifetime,
        }) => {
            let agent = session.id(&agent);
            let payees = if payees.is_empty() {
                None
            } else {
                Some(payees.iter().map(|p| session.id(p)).collect())
            };
            let caveats = Caveats {
                payees,
                max_per_payment: parse_amount(&max_payment)?,
                burst: parse_amount(&burst)?,
                rate: parse_amount(&rate)?,
                expires_at: Pulse(ledger.pulse()?.0 + lifetime),
                evidence_above: None,
                evidence_policy: Policy::standard(),
                receipts: true,
                may_delegate: true,
            };
            let view = session.send(
                &principal,
                TxKind::CreateCharter {
                    agent,
                    caveats,
                    funding: parse_amount(&funding)?,
                    agent_bond: parse_amount(&bond)?,
                },
                Vec::new(),
            )?;
            show_tx(&session.names(), &view);
            for event in &view.effects.events {
                if let Event::CharterCreated { charter, allotment } = event {
                    println!("charter {charter}\nallotment {allotment}");
                }
            }
        }
        Command::Charter(CharterCommand::Pay {
            agent,
            charter,
            allotment,
            payee,
            amount,
            purpose,
        }) => {
            let allotment = ObjectId::from_hash(hex_hash(&allotment, "allotment")?);
            let input = session.reference(&allotment)?;
            let payee = session.id(&payee);
            let view = session.send(
                &agent,
                TxKind::AgentPay {
                    charter: ObjectId::from_hash(hex_hash(&charter, "charter")?),
                    payee,
                    amount: parse_amount(&amount)?,
                    purpose,
                    evidence: None,
                },
                vec![input],
            )?;
            show_tx(&session.names(), &view);
        }
        Command::Charter(CharterCommand::Revoke { principal, charter }) => {
            let view = session.send(
                &principal,
                TxKind::RevokeCharter {
                    charter: ObjectId::from_hash(hex_hash(&charter, "charter")?),
                },
                Vec::new(),
            )?;
            show_tx(&session.names(), &view);
        }
        Command::Market(MarketCommand::Create {
            operator,
            label,
            good,
            batch,
            deadline,
        }) => {
            let view = session.send(
                &operator,
                TxKind::CreateMarket {
                    label,
                    good: parse_asset(&good),
                    numeraire: AssetId::usd(),
                    batch_length: batch,
                    clear_deadline: deadline,
                    committee: Vec::new(),
                    threshold: 0,
                },
                Vec::new(),
            )?;
            show_tx(&session.names(), &view);
        }
        Command::Market(MarketCommand::Order {
            from,
            market,
            side,
            quantity,
            price,
        }) => {
            let side = match side.as_str() {
                "buy" => sion_clearing::Side::Buy,
                "sell" => sion_clearing::Side::Sell,
                other => {
                    return Err(CliError::Usage(format!(
                        "side must be buy or sell, not {other}"
                    )))
                }
            };
            let view = session.send(
                &from,
                TxKind::SubmitOrder {
                    market: ObjectId::from_hash(hex_hash(&market, "market")?),
                    submission: Submission::Plain(OrderTerms {
                        side,
                        quantity,
                        limit_price: parse_amount(&price)?,
                    }),
                },
                Vec::new(),
            )?;
            show_tx(&session.names(), &view);
        }
        Command::Market(MarketCommand::Clear { solver, market }) => {
            let market = ObjectId::from_hash(hex_hash(&market, "market")?);
            let object = ledger
                .object(market)?
                .ok_or_else(|| CliError::Node("no such market".into()))?;
            let ObjectData::Market(state) = object.data else {
                return Err(CliError::Usage("not a market".into()));
            };
            let (index, entry) = state
                .batches
                .iter()
                .find(|(_, b)| matches!(b.status, BatchStatus::Sealed { .. }))
                .ok_or_else(|| CliError::Usage("no sealed batch to clear".into()))?;
            let BatchStatus::Sealed { beacon, at } = entry.status else {
                unreachable!()
            };
            let batch = state.clearing_batch(entry, beacon, at);
            let result =
                sion_clearing::clear(&batch).map_err(|e| CliError::Build(e.to_string()))?;
            println!(
                "clearing batch {index}: volume {}, price {:?}",
                result.volume, result.price
            );
            let view = session.send(
                &solver,
                TxKind::ClearBatch {
                    market,
                    batch: *index,
                    result,
                },
                Vec::new(),
            )?;
            show_tx(&session.names(), &view);
        }
        Command::Market(MarketCommand::Show { market }) => {
            let object = ledger.object(ObjectId::from_hash(hex_hash(&market, "market")?))?;
            print(&serde_json::to_value(object).unwrap_or(Value::Null));
        }
        Command::Tx { digest } => {
            match ledger.tx(TxDigest::from_hash(hex_hash(&digest, "digest")?))? {
                TxState::Executed(view) => show_tx(&session.names(), &view),
                TxState::Pending(state) => println!("{state}"),
                TxState::Rejected(reason) => {
                    return Err(CliError::Transaction(format!("rejected: {reason}")))
                }
                TxState::Unknown => {
                    return Err(CliError::Transaction(format!(
                        "unknown transaction {digest}"
                    )))
                }
            }
        }
        Command::Pulse { pulse } => {
            print(&ledger.query(sion_node::Query::Pulse(pulse.map(Pulse)))?);
        }
        Command::Events { from, limit } => {
            for event in ledger.events(Pulse(from), limit)? {
                println!(
                    "Pulse {:>5}  {}",
                    event.pulse,
                    serde_json::to_string(&event.event).unwrap_or_default()
                );
            }
        }
        Command::Receipt { digest, chain } => {
            let digest = TxDigest::from_hash(hex_hash(&digest, "digest")?);
            let verified = verify::verify_receipt_of(ledger, parse_chain(chain)?, digest)?;
            println!(
                "Pulse {}: {}",
                verified.pulse,
                sion_cli::describe::receipt(&session.names(), &verified.effects)
            );
            match verified.signed {
                Some((stake, quorum)) => println!(
                    "  certified by {stake} of the committee's stake (quorum {quorum}), dual post-quantum signatures ✓"
                ),
                None => println!("  certificate: SKIPPED (insecure simulation signatures)"),
            }
            println!("  receipt proven against the certified header's receipts root ✓");
            println!(
                "  chain {} ({})",
                verified.chain.0.short(),
                if verified.chain.1 {
                    "pinned"
                } else {
                    "NOT pinned: pass --chain to pin it"
                }
            );
        }
        Command::Verify {
            chain,
            pulse,
            samples,
            no_world,
        } => {
            let pinned = parse_chain(chain)?;
            for line in verify::verify_pulse(ledger, pinned, pulse.map(Pulse), samples, !no_world)?
            {
                println!("{line}");
            }
        }
        Command::Demo { name, local: false } => {
            let mut out = Narrator::printing();
            demos::run(&name, &mut session, &mut out)?;
        }
        Command::Demo { local: true, .. } => unreachable!("handled above"),
    }
    Ok(())
}

fn stream_id(text: &str) -> Result<ObjectId, CliError> {
    Ok(ObjectId::from_hash(hex_hash(text, "stream")?))
}

/// `n` with thousands separators.
fn grouped(n: u64) -> String {
    Amount::units(n).to_string()
}

/// `n` and the noun, singular or plural.
fn count(n: u64, one: &str, many: &str) -> String {
    match n {
        1 => format!("1 {one}"),
        n => format!("{} {many}", grouped(n)),
    }
}

fn show_stream(
    names: &sion_cli::describe::Names,
    id: &ObjectId,
    stream: &sion_exec::stream::Stream,
) {
    println!(
        "stream {id}\n  {} pays {} {} USD per step, {} of {} steps settled{}\n  escrow {} USD, settle by Pulse {}, {}",
        names.account(&stream.payer),
        names.account(&stream.payee),
        stream.unit,
        grouped(stream.position),
        grouped(stream.length),
        match stream.spacing {
            0 => String::new(),
            spacing => format!(", a checkpoint every {} steps", grouped(spacing)),
        },
        stream.escrow,
        stream.until,
        if stream.open { "open" } else { "closed" }
    );
}

fn stream_command(session: &mut Session<'_>, command: StreamCommand) -> Result<(), CliError> {
    let ledger = session.ledger;
    match command {
        StreamCommand::Open {
            payer,
            payee,
            unit,
            steps,
            deposit,
            pulses,
            spacing,
        } => {
            let unit = parse_amount(&unit)?;
            let deposit = parse_amount(&deposit)?;
            let payee_id = session.id(&payee);
            let spacing = spacing.unwrap_or_else(|| default_spacing(steps));
            // Building a chain costs one hash per step, once.
            let (seed, chain) = PayerBook::new_chain(steps, spacing);
            let view = session.send(
                &payer,
                TxKind::OpenStream {
                    payee: payee_id,
                    asset: AssetId::usd(),
                    unit,
                    length: steps,
                    anchor: chain.anchor(),
                    deposit,
                    until: Pulse(ledger.pulse()?.0 + pulses),
                    checkpoints: chain.checkpoints(),
                },
                Vec::new(),
            )?;
            show_tx(&session.names(), &view);
            let stream = view
                .effects
                .events
                .iter()
                .find_map(|e| match e {
                    Event::StreamOpened { stream, .. } => Some(*stream),
                    _ => None,
                })
                .ok_or_else(|| CliError::Transaction("the stream did not open".into()))?;
            session.streams.add(
                PayerRecord {
                    stream,
                    wallet: payer,
                    payee: payee_id,
                    unit,
                    length: steps,
                    seed,
                    revealed: 0,
                    spacing,
                    announced: None,
                },
                chain,
            )?;
            println!(
                "stream {stream}\nits chain's secret seed is in {}",
                stream_dir().join(format!("{stream}.json")).display()
            );
        }
        StreamCommand::Pay { stream, steps } => {
            let paid = session.streams.pay_with_jump(&stream_id(&stream)?, steps)?;
            println!("{}", paid.payment);
            if let Some(jump) = &paid.jump {
                println!("jump {}", format_jump(jump));
            }
        }
        StreamCommand::Jump { stream, position } => {
            let id = stream_id(&stream)?;
            let record = session.streams.record(&id)?.clone();
            if record.spacing == 0 {
                return Err(CliError::Usage("this stream has no checkpoints".into()));
            }
            let position = position.unwrap_or(record.revealed);
            match session.streams.jump(&id, position / record.spacing)? {
                Some(jump) => println!("{}", format_jump(&jump)),
                None => return Err(CliError::Usage("no such checkpoint".into())),
            }
        }
        StreamCommand::Check { payment, jump } => {
            let payment: Payment = payment.parse()?;
            let stream = load_stream(ledger, &payment.stream)?;
            let result = match jump {
                Some(jump) => standing_by_jump(&stream, &payment, &parse_jump(&jump)?),
                None => standing(&stream, &payment),
            };
            match result {
                Standing::Settled { hashes } => println!(
                    "genuine and settled: the ledger has settled the stream up to position {} (checked with {})",
                    grouped(stream.position),
                    count(hashes, "hash", "hashes")
                ),
                Standing::Unsettled { steps, value } => println!(
                    "genuine, not yet settled: {} above the settled position, worth {value} USD to the payee",
                    count(steps, "step", "steps")
                ),
                Standing::Forged => {
                    return Err(CliError::Usage(
                        "not an element of this stream's chain".into(),
                    ))
                }
            }
        }
        StreamCommand::Settle {
            payee,
            payment,
            jump,
        } => {
            let payment: Payment = payment.parse()?;
            let view = session.send(
                &payee,
                TxKind::SettleStream {
                    stream: payment.stream,
                    position: payment.position,
                    value: payment.value,
                    jump: jump.as_deref().map(parse_jump).transpose()?,
                },
                Vec::new(),
            )?;
            show_tx(&session.names(), &view);
        }
        StreamCommand::TopUp {
            payer,
            stream,
            deposit,
            pulses,
        } => {
            let id = stream_id(&stream)?;
            let until = Pulse(load_stream(ledger, &id)?.until.0 + pulses);
            let view = session.send(
                &payer,
                TxKind::TopUpStream {
                    stream: id,
                    deposit: parse_amount(&deposit)?,
                    until,
                },
                Vec::new(),
            )?;
            show_tx(&session.names(), &view);
        }
        StreamCommand::Close { who, stream } => {
            let view = session.send(
                &who,
                TxKind::CloseStream {
                    stream: stream_id(&stream)?,
                },
                Vec::new(),
            )?;
            show_tx(&session.names(), &view);
        }
        StreamCommand::Show { stream } => {
            let id = stream_id(&stream)?;
            show_stream(&session.names(), &id, &load_stream(ledger, &id)?);
        }
    }
    Ok(())
}

fn rand_nonce() -> u64 {
    use rand_core::RngCore;
    rand_core::OsRng.next_u64()
}

/// `sion mcp <server>`: the first-party MCP servers.
#[derive(Debug, Subcommand)]
enum McpServe {
    /// A person's mailbox over IMAP and SMTP (MAIL_ADDRESS, MAIL_IMAP_URL,
    /// MAIL_SMTP_URL, MAIL_PASSWORD or MAIL_TOKEN_COMMAND): search, read,
    /// draft, send.
    Mail,
    /// A person's calendars over CalDAV (CALDAV_URL, CALDAV_USERNAME and
    /// CALDAV_PASSWORD, or CALDAV_TOKEN_COMMAND): list, read, add, remove
    /// events.
    Calendar,
    /// A folder of documents: list, read (text, web pages, Word,
    /// OpenDocument, PowerPoint, PDF), search, write text and Markdown.
    Documents {
        /// The folder.
        #[arg(long, env = "DOCUMENTS_DIR")]
        dir: std::path::PathBuf,
    },
}

fn main() -> ExitCode {
    // Inside a skill's sandbox, this program is the bridge to the egress
    // proxy (`sion_agent::egress`): not a command people run.
    let mut args = std::env::args();
    if args.nth(1).as_deref() == Some("sandbox-bridge") {
        let status = sion_agent::egress::bridge_cli(args);
        return ExitCode::from(u8::try_from(status).unwrap_or(125));
    }
    // Run by `sion agent up`: stop when it stops.
    sion_cli::up::watch_supervisor();
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}
