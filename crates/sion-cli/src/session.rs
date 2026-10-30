//! A client session: wallets, transaction building and waiting.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use sion_core::ids::{AccountId, AssetId, ObjectId, Pulse};
use sion_core::Amount;
use sion_exec::wallet::Keyless;
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
    Wallet::from_seed(&seed_of(name))
}

/// The seed of the wallet called `name`: its seed file's, or the devnet
/// wallet's.
fn seed_of(name: &str) -> zeroize::Zeroizing<[u8; 32]> {
    let path = wallet_dir().join(format!("{name}.seed"));
    if let Ok(text) = std::fs::read_to_string(&path) {
        if let Ok(bytes) = hex::decode(text.trim()) {
            if let Ok(seed) = <[u8; 32]>::try_from(bytes.as_slice()) {
                return zeroize::Zeroizing::new(seed);
            }
        }
    }
    zeroize::Zeroizing::new(sion_exec::wallet::devnet_seed(name))
}

/// The secret a wallet derives its other keys from (an archive's, say):
/// its seed. A keyless subagent has none of its own, so its signer's is
/// used. A devnet wallet's seed, like its keys, anyone can derive from its
/// name.
pub fn wallet_seed(name: &str) -> zeroize::Zeroizing<[u8; 32]> {
    match keyless_alias(name) {
        Some(alias) => seed_of(&alias.signer),
        None => seed_of(name),
    }
}

/// A keyless subagent's wallet file, `<name>.keyless` under
/// [`wallet_dir`] (spec v1.13): it holds no key, only the subagent's charter
/// and the wallet that signs for it, its nearest keyed ancestor's agent. A
/// session sends from such a name as a batch of one that wallet signs.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct KeylessAlias {
    /// The subagent's charter (its id).
    pub charter: ObjectId,
    /// The keyed wallet that signs for it.
    pub signer: String,
}

/// The keyless subagent called `name`, if a `.keyless` file names one.
pub fn keyless_alias(name: &str) -> Option<KeylessAlias> {
    let path = wallet_dir().join(format!("{name}.keyless"));
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// Writes the `.keyless` file for `name`, unless a wallet of that name
/// exists already (a seed or another keyless subagent).
pub fn create_keyless_alias(name: &str, alias: &KeylessAlias) -> Result<(), CliError> {
    let dir = wallet_dir();
    std::fs::create_dir_all(&dir).map_err(|e| CliError::Usage(e.to_string()))?;
    for existing in [format!("{name}.seed"), format!("{name}.keyless")] {
        if dir.join(&existing).exists() {
            return Err(CliError::Usage(format!(
                "{} already exists",
                dir.join(existing).display()
            )));
        }
    }
    let text = serde_json::to_string_pretty(alias).map_err(|e| CliError::Usage(e.to_string()))?;
    std::fs::write(dir.join(format!("{name}.keyless")), text)
        .map_err(|e| CliError::Usage(e.to_string()))
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
    /// Names for assets in this session's sentences, over the usual ones.
    asset_labels: BTreeMap<AssetId, String>,
    /// Keyless subagents' transaction builders, by charter.
    keyless: BTreeMap<ObjectId, Keyless>,
    /// Names that stand for keyless subagents.
    aliases: BTreeMap<String, KeylessAlias>,
}

/// Who sends one transaction of a batch ([`Session::send_batch`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Actor {
    /// A wallet, by name.
    Wallet(String),
    /// The keyless subagent with this charter (spec §4.3).
    Keyless(ObjectId),
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
            asset_labels: BTreeMap::new(),
            keyless: BTreeMap::new(),
            aliases: BTreeMap::new(),
        })
    }

    /// Names `asset` as `label` in this session's sentences: an app on a
    /// test network calls the devnet's USD "test USD", so that no amount it
    /// shows reads as money.
    pub fn label_asset(&mut self, asset: AssetId, label: &str) {
        self.asset_labels.insert(asset, label.to_string());
    }

    /// An address book: the devnet's usual accounts and every wallet this
    /// session has used.
    pub fn names(&self) -> crate::describe::Names {
        let mut names = crate::describe::Names::devnet(["alice", "bob", "carol", "faucet"]);
        for (name, wallet) in &self.wallets {
            names.add_account(wallet.id, name);
        }
        for (name, alias) in &self.aliases {
            names.add_account(sion_exec::charter::keyless_account(&alias.charter), name);
        }
        for (asset, label) in &self.asset_labels {
            names.add_asset(*asset, label);
        }
        names
    }

    /// Uses `wallet` for `name` in this session, instead of a seed file or
    /// the deterministic devnet wallet of that name (for apps that keep
    /// their own key).
    pub fn add_wallet(&mut self, name: &str, wallet: Wallet) {
        self.wallets.insert(name.to_string(), wallet);
    }

    /// The wallet for `name`.
    pub fn wallet(&mut self, name: &str) -> &mut Wallet {
        self.wallets
            .entry(name.to_string())
            .or_insert_with(|| wallet_for(name))
    }

    /// The account of `name` (or a hex account identifier). A keyless
    /// subagent's name gives its derived account.
    pub fn id(&mut self, name: &str) -> AccountId {
        if let Ok(hash) = sion_core::hash::Hash32::from_hex(name) {
            return AccountId::from_hash(hash);
        }
        if let Some(alias) = self.keyless_of(name) {
            return sion_exec::charter::keyless_account(&alias.charter);
        }
        self.wallet(name).id
    }

    /// The keyless subagent `name` stands for, if it is one: registered in
    /// this session, or by a `.keyless` file.
    pub fn keyless_of(&mut self, name: &str) -> Option<KeylessAlias> {
        if self.wallets.contains_key(name) {
            return None;
        }
        if let Some(alias) = self.aliases.get(name) {
            return Some(alias.clone());
        }
        let alias = keyless_alias(name)?;
        self.aliases.insert(name.to_string(), alias.clone());
        Some(alias)
    }

    /// Lets `name` stand, in this session, for the keyless subagent with
    /// charter `charter`, signed for by the wallet `signer`.
    pub fn add_keyless(&mut self, name: &str, charter: ObjectId, signer: &str) {
        self.aliases.insert(
            name.to_string(),
            KeylessAlias {
                charter,
                signer: signer.to_string(),
            },
        );
    }

    /// The keyed wallet that signs for `name`: `name` itself, or the signer
    /// of the keyless subagent it stands for.
    pub fn signer_of(&mut self, name: &str) -> String {
        match self.keyless_of(name) {
            Some(alias) => alias.signer,
            None => name.to_string(),
        }
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

    /// Builds, signs, submits and waits for a transaction. From a keyless
    /// subagent's name, a batch of one its signer signs.
    pub fn send(
        &mut self,
        name: &str,
        kind: TxKind,
        extra: Vec<ObjectRef>,
    ) -> Result<TxView, CliError> {
        if let Some(alias) = self.keyless_of(name) {
            let mut views = self.send_batch(
                &alias.signer,
                vec![(Actor::Keyless(alias.charter), kind, extra)],
            )?;
            return views
                .pop()
                .ok_or_else(|| CliError::Protocol("an empty batch".into()));
        }
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
        // A submission refused outright leaves its coins free: release
        // them, or every later transaction would see them in flight.
        let receipt = match ledger.submit(&tx) {
            Ok(receipt) => receipt,
            Err(error) => {
                self.wallet(name).settle();
                return Err(error);
            }
        };
        // Wait for the transaction signed here, whatever digest the node
        // answered with.
        let digest = tx.digest();
        if receipt.digest != digest {
            self.wallet(name).settle();
            return Err(CliError::Protocol(format!(
                "the node acknowledged transaction {} instead of {}",
                receipt.digest.short(),
                digest.short()
            )));
        }
        let result = self.wait(digest);
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
        let alias = self.keyless_of(name);
        let tx = match &alias {
            Some(alias) => self
                .sign_batch(
                    &alias.signer,
                    vec![(Actor::Keyless(alias.charter), kind, extra)],
                )?
                .remove(0),
            None => self.wallet(name).build(&view, kind, extra)?,
        };
        if self.verbose {
            let sentence = crate::describe::sentence(&self.names(), &tx.tx);
            match &alias {
                Some(alias) => println!(
                    "{} signs for {name} (forced, as a batch of one): {sentence}",
                    alias.signer
                ),
                None => println!("{name} signs (forced): {sentence}"),
            }
        }
        let receipt = match ledger.force(&tx) {
            Ok(receipt) => receipt,
            Err(error) => {
                self.wallet(name).settle();
                return Err(error);
            }
        };
        let result = self.wait(receipt.digest);
        self.wallet(name).settle();
        result
    }

    /// Builds `actions`, signs them as one batch with `signer`'s wallet,
    /// submits them in one request and waits for each (spec §4.3): one
    /// authorization for all of them. An action is a sender (a wallet's
    /// name, or a keyless subagent whose nearest keyed ancestor's agent is
    /// `signer`), what it does and the owned inputs it names after its gas
    /// coin. Each pays the fee floor of a member of a batch of this size.
    pub fn send_batch(
        &mut self,
        signer: &str,
        actions: Vec<(Actor, TxKind, Vec<ObjectRef>)>,
    ) -> Result<Vec<TxView>, CliError> {
        let members = self.sign_batch(signer, actions)?;
        if self.verbose {
            // What is signed, member by member, as for a transaction alone.
            let names = self.names();
            let signer = self.signer_of(signer);
            if let [member] = members.as_slice() {
                println!(
                    "{signer} signs, as a batch of one: {}",
                    crate::describe::sentence(&names, &member.tx)
                );
            } else {
                println!(
                    "{signer} signs one batch of {} transactions:",
                    members.len()
                );
                for member in &members {
                    println!("  {}", crate::describe::sentence(&names, &member.tx));
                }
            }
        }
        let result = self
            .ledger
            .submit_batch(&members)
            .and_then(|_| members.iter().map(|m| self.wait(m.digest())).collect());
        self.settle_all();
        result
    }

    /// Builds `actions` and signs them as one batch with `signer`'s wallet,
    /// without submitting them (see [`Session::send_batch`]).
    pub fn sign_batch(
        &mut self,
        signer: &str,
        actions: Vec<(Actor, TxKind, Vec<ObjectRef>)>,
    ) -> Result<Vec<sion_exec::SignedTransaction>, CliError> {
        let size = actions.len() as u64;
        let config = self.config.clone();
        let ledger = self.ledger;
        let view = RemoteView::new(ledger, config.clone());
        let mut bodies = Vec::with_capacity(actions.len());
        for (actor, kind, extra) in actions {
            // A wallet name may stand for a keyless subagent.
            let actor = match actor {
                Actor::Wallet(name) => match self.keyless_of(&name) {
                    Some(alias) => Actor::Keyless(alias.charter),
                    None => Actor::Wallet(name),
                },
                keyless => keyless,
            };
            let drafted = match &actor {
                Actor::Wallet(name) => self
                    .wallet(name)
                    .draft(&view, kind, extra, Some(size))
                    .map(|tx| (tx, None)),
                Actor::Keyless(charter) => self
                    .keyless
                    .entry(*charter)
                    .or_insert_with(|| Keyless::new(*charter))
                    .draft(&view, kind, extra, size)
                    .map(|tx| (tx, Some(*charter))),
            };
            match drafted {
                Ok(body) => bodies.push(body),
                Err(error) => {
                    self.settle_all();
                    return Err(error.into());
                }
            }
        }
        let signer = self.signer_of(signer);
        let signed = self.wallet(&signer).sign_batch(&config, bodies);
        if signed.is_err() {
            self.settle_all();
        }
        Ok(signed?)
    }

    /// The account of the keyless subagent whose charter is `charter`.
    pub fn keyless_id(&self, charter: &ObjectId) -> AccountId {
        sion_exec::charter::keyless_account(charter)
    }

    /// Builds and signs a transaction without submitting it: from a wallet,
    /// or for a keyless subagent's name as a batch of one its signer signs.
    pub fn sign(
        &mut self,
        name: &str,
        kind: TxKind,
        extra: Vec<ObjectRef>,
    ) -> Result<sion_exec::SignedTransaction, CliError> {
        if let Some(alias) = self.keyless_of(name) {
            return Ok(self
                .sign_batch(
                    &alias.signer,
                    vec![(Actor::Keyless(alias.charter), kind, extra)],
                )?
                .remove(0));
        }
        let config = self.config.clone();
        let view = RemoteView::new(self.ledger, config);
        Ok(self.wallet(name).build(&view, kind, extra)?)
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
        let tx = match self.keyless_of(name) {
            Some(alias) => self
                .sign_batch(
                    &alias.signer,
                    vec![(Actor::Keyless(alias.charter), kind, extra)],
                )?
                .remove(0),
            None => self.wallet(name).build(&view, kind, extra)?,
        };
        Ok(ledger.submit(&tx)?.digest)
    }

    /// Clears every wallet's record of coins committed to pending transactions.
    pub fn settle_all(&mut self) {
        for wallet in self.wallets.values_mut() {
            wallet.settle();
        }
        for builder in self.keyless.values_mut() {
            builder.settle();
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
