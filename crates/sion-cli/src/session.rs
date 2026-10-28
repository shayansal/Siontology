//! A client session: wallets, transaction building and waiting.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use sion_core::ids::{AccountId, AssetId, ObjectId, Pulse};
use sion_core::Amount;
use sion_exec::{ChainConfig, ExecStatus, Object, ObjectRef, StateView, TxKind, Wallet};
use sion_node::view::TxView;

use crate::error::CliError;
use crate::ledger::{Ledger, LedgerExt, TxState};

/// A wallet's view of the chain through a ledger.
pub struct RemoteView<'a> {
    ledger: &'a dyn Ledger,
    config: ChainConfig,
}

impl<'a> RemoteView<'a> {
    /// A view.
    pub fn new(ledger: &'a dyn Ledger, config: ChainConfig) -> Self {
        RemoteView { ledger, config }
    }
}

impl std::fmt::Debug for RemoteView<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RemoteView({})", self.ledger.describe())
    }
}

impl StateView for RemoteView<'_> {
    fn config(&self) -> &ChainConfig {
        &self.config
    }

    fn current_pulse(&self) -> Pulse {
        self.ledger.pulse().unwrap_or(Pulse(0))
    }

    fn object(&self, id: &ObjectId) -> Option<Object> {
        self.ledger.object(*id).ok().flatten()
    }

    fn coins(&self, account: &AccountId) -> Vec<Object> {
        self.ledger
            .account(*account)
            .map(|v| v.coins)
            .unwrap_or_default()
    }
}

/// Where named wallets' seeds live: `$SION_HOME/wallets` (default
/// `~/.sion/wallets`).
pub fn wallet_dir() -> PathBuf {
    let home = std::env::var_os("SION_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".sion")))
        .unwrap_or_else(|| PathBuf::from(".sion"));
    home.join("wallets")
}

/// The wallet called `name`: from its seed file if one exists, otherwise
/// the deterministic devnet wallet of that name (devnet only — anyone who
/// knows the name can derive its keys).
pub fn wallet_for(name: &str) -> Wallet {
    let path = wallet_dir().join(format!("{name}.seed"));
    if let Ok(text) = std::fs::read_to_string(&path) {
        if let Ok(bytes) = hex::decode(text.trim()) {
            if let Ok(seed) = <[u8; 32]>::try_from(bytes.as_slice()) {
                return Wallet::from_seed(&seed);
            }
        }
    }
    Wallet::named(name)
}

/// Where a payer's stream chains live: `$SION_HOME/streams` (default
/// `~/.sion/streams`). Each file holds a chain's secret seed.
pub fn stream_dir() -> PathBuf {
    wallet_dir()
        .parent()
        .map(|home| home.join("streams"))
        .unwrap_or_else(|| PathBuf::from(".sion/streams"))
}

/// Creates a wallet with a fresh random seed stored under [`wallet_dir`].
pub fn create_wallet(name: &str) -> Result<Wallet, CliError> {
    let dir = wallet_dir();
    std::fs::create_dir_all(&dir).map_err(|e| CliError::Usage(e.to_string()))?;
    let path = dir.join(format!("{name}.seed"));
    if path.exists() {
        return Err(CliError::Usage(format!(
            "{} already exists",
            path.display()
        )));
    }
    let seed = sion_crypto::keys::random_seed();
    std::fs::write(&path, hex::encode(seed)).map_err(|e| CliError::Usage(e.to_string()))?;
    Ok(Wallet::from_seed(&seed))
}

/// A session against one ledger.
pub struct Session<'a> {
    /// The ledger.
    pub ledger: &'a dyn Ledger,
    /// Chain configuration.
    pub config: ChainConfig,
    wallets: BTreeMap<String, Wallet>,
    /// How long to wait for a transaction.
    pub timeout: Duration,
    /// Print what happens.
    pub verbose: bool,
    /// The payment streams this session pays from (in memory unless the
    /// caller points it at a directory).
    pub streams: crate::stream::PayerBook,
}

impl std::fmt::Debug for Session<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Session({})", self.ledger.describe())
    }
}

impl<'a> Session<'a> {
    /// Opens a session.
    pub fn open(ledger: &'a dyn Ledger) -> Result<Session<'a>, CliError> {
        let config = ledger.chain_config()?;
        Ok(Session {
            ledger,
            config,
            wallets: BTreeMap::new(),
            timeout: Duration::from_secs(90),
            verbose: false,
            streams: crate::stream::PayerBook::in_memory(),
        })
    }

    /// An address book: the devnet's usual accounts and every wallet this
    /// session has used.
    pub fn names(&self) -> crate::describe::Names {
        let mut names = crate::describe::Names::devnet(["alice", "bob", "carol", "faucet"]);
        for (name, wallet) in &self.wallets {
            names.add_account(wallet.id, name);
        }
        names
    }

    /// The wallet for `name`.
    pub fn wallet(&mut self, name: &str) -> &mut Wallet {
        self.wallets
            .entry(name.to_string())
            .or_insert_with(|| wallet_for(name))
    }

    /// The account of `name` (or a hex account identifier).
    pub fn id(&mut self, name: &str) -> AccountId {
        if let Ok(hash) = sion_core::hash::Hash32::from_hex(name) {
            return AccountId::from_hash(hash);
        }
        self.wallet(name).id
    }

    /// Balance of `asset` held by `name`.
    pub fn balance(&mut self, name: &str, asset: AssetId) -> Result<Amount, CliError> {
        let id = self.id(name);
        let view = self.ledger.account(id)?;
        Ok(view
            .balances
            .iter()
            .find(|(a, _)| *a == asset)
            .map(|(_, v)| *v)
            .unwrap_or(Amount::ZERO))
    }

    /// Builds, signs, submits and waits for a transaction.
    pub fn send(
        &mut self,
        name: &str,
        kind: TxKind,
        extra: Vec<ObjectRef>,
    ) -> Result<TxView, CliError> {
        let config = self.config.clone();
        let ledger = self.ledger;
        let view = RemoteView::new(ledger, config);
        let tx = self.wallet(name).build(&view, kind, extra)?;
        if self.verbose {
            println!(
                "{name} signs: {}",
                crate::describe::sentence(&self.names(), &tx.tx)
            );
        }
        let receipt = ledger.submit(&tx)?;
        let result = self.wait(receipt.digest);
        self.wallet(name).settle();
        result
    }

    /// Posts a forced intent (bypassing the gateway) and waits for it to
    /// execute.
    pub fn force(
        &mut self,
        name: &str,
        kind: TxKind,
        extra: Vec<ObjectRef>,
    ) -> Result<TxView, CliError> {
        let config = self.config.clone();
        let ledger = self.ledger;
        let view = RemoteView::new(ledger, config);
        let tx = self.wallet(name).build(&view, kind, extra)?;
        if self.verbose {
            println!(
                "{name} signs (forced): {}",
                crate::describe::sentence(&self.names(), &tx.tx)
            );
        }
        let receipt = ledger.force(&tx)?;
        let result = self.wait(receipt.digest);
        self.wallet(name).settle();
        result
    }

    /// Submits without waiting; returns the digest.
    pub fn submit(
        &mut self,
        name: &str,
        kind: TxKind,
        extra: Vec<ObjectRef>,
    ) -> Result<sion_core::ids::TxDigest, CliError> {
        let config = self.config.clone();
        let ledger = self.ledger;
        let view = RemoteView::new(ledger, config);
        let tx = self.wallet(name).build(&view, kind, extra)?;
        Ok(ledger.submit(&tx)?.digest)
    }

    /// Clears every wallet's record of coins committed to pending transactions.
    pub fn settle_all(&mut self) {
        for wallet in self.wallets.values_mut() {
            wallet.settle();
        }
    }

    /// Waits for a transaction to execute.
    pub fn wait(&self, digest: sion_core::ids::TxDigest) -> Result<TxView, CliError> {
        let deadline = Instant::now() + self.timeout;
        loop {
            match self.ledger.tx(digest)? {
                TxState::Executed(view) => return Ok(*view),
                TxState::Rejected(reason) => {
                    return Err(CliError::Transaction(format!("not certified: {reason}")))
                }
                TxState::Pending(_) | TxState::Unknown => {}
            }
            if Instant::now() > deadline {
                return Err(CliError::Transaction(format!(
                    "{digest} did not execute in time"
                )));
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    /// Sends and requires success.
    pub fn must(
        &mut self,
        name: &str,
        kind: TxKind,
        extra: Vec<ObjectRef>,
    ) -> Result<TxView, CliError> {
        let view = self.send(name, kind, extra)?;
        match &view.status {
            ExecStatus::Success => Ok(view),
            other => Err(CliError::Transaction(format!(
                "{name}'s transaction did not succeed: {other:?}"
            ))),
        }
    }

    /// Funds `name` from the faucet and waits for the coins.
    pub fn fund(&mut self, name: &str, asset: AssetId, amount: Amount) -> Result<(), CliError> {
        let before = self.balance(name, asset)?;
        let id = self.id(name);
        self.ledger.faucet(id, asset, amount)?;
        let deadline = Instant::now() + self.timeout;
        while self.balance(name, asset)? < Amount(before.0 + amount.0) {
            if Instant::now() > deadline {
                return Err(CliError::Transaction(
                    "faucet grant did not arrive in time".into(),
                ));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        Ok(())
    }

    /// Waits until the chain has executed `pulses` more Pulses.
    pub fn wait_pulses(&self, pulses: u64) -> Result<Pulse, CliError> {
        let target = Pulse(self.ledger.pulse()?.0 + pulses);
        let deadline = Instant::now() + self.timeout;
        loop {
            let now = self.ledger.pulse()?;
            if now >= target {
                return Ok(now);
            }
            if Instant::now() > deadline {
                return Err(CliError::Transaction(format!(
                    "Pulse {target} not reached in time"
                )));
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    /// Waits until the chain reaches `pulse`.
    pub fn wait_until(&self, pulse: Pulse) -> Result<(), CliError> {
        let now = self.ledger.pulse()?;
        if now < pulse {
            self.wait_pulses(pulse.0 - now.0)?;
        }
        Ok(())
    }

    /// An object's current reference.
    pub fn reference(&self, id: &ObjectId) -> Result<ObjectRef, CliError> {
        self.ledger
            .object(*id)?
            .map(|o| o.reference())
            .ok_or_else(|| CliError::Node(format!("object {} not found", id.short())))
    }
}
