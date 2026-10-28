//! The Siontology app's core, shared by its desktop and mobile shells.
//!
//! Every install is a **light node**: it holds the device's own key, and
//! it checks what it shows instead of trusting the server it talks to.
//! [`AppCore::verify_latest`] verifies the latest certified Pulse the way
//! `sion verify` does: the chain identity against the one pinned on first
//! use, the committee certificate with every signature, the receipts root,
//! a state proof and random shares of the Pulse's data square. Many
//! devices sampling shares make it likely that a block producer that
//! withholds data is caught (see the book's *Data availability*).
//!
//! The core also pays and delegates stake to validators (pooled stake,
//! specification v1.11), through the same library the command line uses,
//! against any node's HTTP API. Its state lives in one directory the shell
//! chooses: `key.seed` (mode 0600) and `settings.json`.
//!
//! Every transaction the device signs comes back with its receipt proven
//! against its Pulse's certified header, as `sion receipt` proves one.
//!
//! [`AppCore`] is `Send + Sync` and every method takes `&self`, so a shell
//! can call it from several threads: a verification in progress does not
//! hold up a balance or a payment. The device's own transactions are
//! submitted one at a time, so that two never spend the same coin.
//!
//! Every method returns JSON, which the shells hand to their user
//! interface unchanged.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sion_cli::ledger::{HttpLedger, Ledger, LedgerExt};
use sion_cli::session::Session;
use sion_cli::storage::{Provider, ProviderPolicy, Store};
use sion_core::hash::Hash32;
use sion_core::ids::{AssetId, Pulse, TxDigest};
use sion_exec::wallet::Wallet;
use sion_exec::TxKind;
use sion_light::EpochChain;

/// The wallet name the app's key goes by inside a session.
const ME: &str = "me";

/// How many verification results the app keeps.
const HISTORY: usize = 50;

/// Shares of each data square a phone samples (a desktop may take more).
pub const DEFAULT_SAMPLES: usize = 16;

/// Why an app operation failed.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    /// Reading or writing the app's directory failed.
    #[error("storage: {0}")]
    Storage(String),
    /// The node or the protocol refused, or verification failed.
    #[error("{0}")]
    Ledger(#[from] sion_cli::error::CliError),
    /// A malformed input.
    #[error("{0}")]
    Input(String),
}

/// What the app remembers between runs.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Settings {
    /// The node's API, for example `http://127.0.0.1:8730`.
    pub node: String,
    /// The chain identity pinned on first use (hex), or set by the user.
    pub chain: Option<String>,
    /// Shares of each data square to sample when verifying.
    pub samples: usize,
    /// Whether the device provides storage (spec v1.12): it takes open
    /// storage deals it can fetch, keeps the data, and proves every round.
    #[serde(default)]
    pub provide_storage: bool,
    /// The most space one deal may take, in MiB.
    #[serde(default = "default_storage_mib")]
    pub storage_max_mib: u64,
    /// Also take deals whose data is on this machine or its local network
    /// (`file://` locators, private addresses). Off by default: a deal
    /// written by a stranger would make the device read its own files or
    /// probe its network.
    #[serde(default)]
    pub storage_local: bool,
    /// Whether the next verification may pin the chain identity the node
    /// serves. Off after settings that could not be read were reset, so
    /// that a reset never pins a new identity silently.
    #[serde(default = "yes")]
    pub pin_on_first_use: bool,
}

fn default_storage_mib() -> u64 {
    256
}

fn yes() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            node: "http://127.0.0.1:8730".into(),
            chain: None,
            samples: DEFAULT_SAMPLES,
            provide_storage: false,
            storage_max_mib: default_storage_mib(),
            storage_local: false,
            pin_on_first_use: true,
        }
    }
}

/// One verification of a Pulse, as the app shows it.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Verified {
    /// The Pulse verified, if the check got that far.
    pub pulse: Option<u64>,
    /// Whether every check passed.
    pub ok: bool,
    /// Whether this verification pinned the chain identity (trust on
    /// first use).
    pub pinned_now: bool,
    /// The checks, one line each.
    pub lines: Vec<String>,
}

/// The app's state.
#[derive(Debug)]
pub struct AppCore {
    dir: PathBuf,
    wallet: Wallet,
    /// The settings, and the client for the node they name: replaced
    /// together, under the settings' lock.
    settings: RwLock<Settings>,
    ledger: RwLock<HttpLedger>,
    history: Mutex<VecDeque<Verified>>,
    /// The committees followed so far, so that each verification follows
    /// only the epoch boundaries since the last one.
    epochs: Mutex<Option<EpochChain>>,
    /// Held while one of the device's transactions is built and submitted.
    submitting: Mutex<()>,
    /// The device's storage provider, kept between ticks for its cache of
    /// trees and of deals it could not fetch.
    provider: Mutex<Option<Provider>>,
    /// Something the person should know about the settings (they could not
    /// be read and were reset), until settings are saved again.
    notice: Mutex<Option<String>>,
}

// A lock is only poisoned if a thread panicked while holding it; every
// write below replaces whole values, so the data is still consistent.
fn read<T>(lock: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(PoisonError::into_inner)
}

fn write<T>(lock: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    lock.write().unwrap_or_else(PoisonError::into_inner)
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl AppCore {
    /// Opens the app's state in `dir`, creating the device's key on first
    /// use.
    pub fn open(dir: impl AsRef<Path>) -> Result<AppCore, AppError> {
        let dir = dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&dir).map_err(|e| AppError::Storage(e.to_string()))?;
        let path = dir.join("settings.json");
        let mut notice = None;
        let settings: Settings = match std::fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str(&text) {
                Ok(settings) => settings,
                Err(error) => {
                    // Keep the unreadable file for the person to look at,
                    // and do not pin a chain identity on first use again
                    // without being asked.
                    let kept = dir.join("settings.json.unreadable");
                    let _ = std::fs::rename(&path, &kept);
                    notice = Some(format!(
                        "The settings could not be read ({error}) and were reset; the old file is kept as {}. Pin the chain identity again in Settings: it will not be pinned on first use.",
                        kept.display()
                    ));
                    Settings {
                        pin_on_first_use: false,
                        ..Settings::default()
                    }
                }
            },
            Err(_) => Settings::default(),
        };
        let wallet = Wallet::from_seed(&load_or_create_seed(&dir.join("key.seed"))?);
        let ledger = HttpLedger::new(&settings.node);
        Ok(AppCore {
            dir,
            wallet,
            settings: RwLock::new(settings),
            ledger: RwLock::new(ledger),
            history: Mutex::new(VecDeque::new()),
            epochs: Mutex::new(None),
            submitting: Mutex::new(()),
            provider: Mutex::new(None),
            notice: Mutex::new(notice),
        })
    }

    /// Saves the settings: written whole to a temporary file, then renamed
    /// into place, so a crash never leaves half a file.
    fn save(&self, settings: &Settings) -> Result<(), AppError> {
        let text =
            serde_json::to_string_pretty(settings).map_err(|e| AppError::Storage(e.to_string()))?;
        let path = self.dir.join("settings.json");
        let partial = self.dir.join("settings.json.partial");
        std::fs::write(&partial, text)
            .and_then(|()| std::fs::rename(&partial, &path))
            .map_err(|e| AppError::Storage(e.to_string()))
    }

    /// The settings and the client for their node, read together.
    fn snapshot(&self) -> (Settings, HttpLedger) {
        let settings = read(&self.settings);
        (settings.clone(), read(&self.ledger).clone())
    }

    fn ledger(&self) -> HttpLedger {
        self.snapshot().1
    }

    fn session<'l>(&self, ledger: &'l HttpLedger) -> Result<Session<'l>, AppError> {
        let mut session = Session::open(ledger)?;
        session.add_wallet(ME, self.wallet.clone());
        // Every amount the app shows is test money: its sentences say so.
        session.label_asset(AssetId::usd(), "test USD");
        Ok(session)
    }

    /// The settings, and a notice about them if there is one.
    pub fn settings(&self) -> Value {
        let mut value = json!(*read(&self.settings));
        value["notice"] = json!(*lock(&self.notice));
        value
    }

    /// Points the app at another node. The pinned chain identity stays: a
    /// node of another chain fails verification.
    pub fn set_node(&self, node: &str) -> Result<Value, AppError> {
        let node = node.trim().trim_end_matches('/');
        if !(node.starts_with("http://") || node.starts_with("https://")) {
            return Err(AppError::Input(
                "the node must be an http:// or https:// URL".into(),
            ));
        }
        let mut settings = write(&self.settings);
        settings.node = node.to_string();
        *write(&self.ledger) = HttpLedger::new(node);
        self.save(&settings)?;
        Ok(json!(*settings))
    }

    /// Pins a chain identity (64 hex digits), or forgets it with `None`, so
    /// that the next verification pins whatever the node serves.
    pub fn pin_chain(&self, chain: Option<&str>) -> Result<Value, AppError> {
        let chain = match chain.map(str::trim).filter(|c| !c.is_empty()) {
            Some(hex) => Some(
                Hash32::from_hex(hex)
                    .map_err(|_| AppError::Input("a chain identity is 64 hex digits".into()))?
                    .to_hex(),
            ),
            None => None,
        };
        let mut settings = write(&self.settings);
        // Pinning by hand settles the question; unpinning asks for the next
        // verification to pin again.
        settings.pin_on_first_use = chain.is_none();
        settings.chain = chain;
        self.save(&settings)?;
        *lock(&self.notice) = None;
        Ok(json!(*settings))
    }

    /// The device's account.
    pub fn me(&self) -> Value {
        json!({ "account": self.wallet.id, "short": self.wallet.id.short() })
    }

    /// The node's status, as it reports it (not verified).
    pub fn status(&self) -> Result<Value, AppError> {
        Ok(self.ledger().status()?)
    }

    /// The device account's balances and coins (from the node; the latest
    /// verification says whether the node's Pulse checked out).
    pub fn balance(&self) -> Result<Value, AppError> {
        let view = self.ledger().account(self.wallet.id)?;
        let usd: u128 = view
            .balances
            .iter()
            .filter(|(asset, _)| *asset == AssetId::usd())
            .map(|(_, amount)| amount.0)
            .sum();
        Ok(json!({
            "account": self.wallet.id,
            "usd": sion_core::Amount(usd).to_string(),
            "coins": view.coins.len(),
            "delegations": view.delegations.len(),
            "pulse": view.pulse,
        }))
    }

    /// Devnet only: asks the node's faucet for `amount` USD.
    pub fn faucet(&self, amount: &str) -> Result<Value, AppError> {
        let amount = sion_cli::agents::parse_amount(amount)?;
        self.ledger()
            .faucet(self.wallet.id, AssetId::usd(), amount)?;
        Ok(json!({ "requested": amount.to_string() }))
    }

    /// Pays `amount` USD to `to`, a 64-digit account. (A name would mean a
    /// devnet wallet whose keys anyone can derive from the name.) Returns
    /// the sentence signed and the receipt.
    pub fn send(&self, to: &str, amount: &str) -> Result<Value, AppError> {
        let amount = sion_cli::agents::parse_amount(amount)?;
        let to = Hash32::from_hex(to.trim())
            .map(sion_core::ids::AccountId::from_hash)
            .map_err(|_| AppError::Input("give the recipient's account: 64 hex digits".into()))?;
        let _turn = lock(&self.submitting);
        let ledger = self.ledger();
        let mut session = self.session(&ledger)?;
        let kind = TxKind::Transfer {
            asset: AssetId::usd(),
            outputs: vec![(to, amount)],
        };
        self.submit(&mut session, kind, Vec::new())
    }

    /// Verifies the latest certified Pulse as a light client, pinning the
    /// chain identity on first use, and records the result.
    ///
    /// It holds no lock while it talks to the node. It keeps the
    /// committees it followed for the next verification. It pins only if
    /// the settings did not change meanwhile (the same node, still
    /// unpinned), and it pins the identity it verified against.
    pub fn verify_latest(&self) -> Result<Value, AppError> {
        let (settings, ledger) = self.snapshot();
        let pinned = match &settings.chain {
            Some(hex) => Some(Hash32::from_hex(hex).map_err(|_| {
                AppError::Input("the pinned chain identity is not 64 hex digits".into())
            })?),
            None => None,
        };
        // The newest certified Pulse: a device follows the head of the chain.
        let latest = ledger
            .status()
            .ok()
            .and_then(|status| {
                serde_json::from_value::<Pulse>(status["certified_pulse"].clone()).ok()
            })
            .filter(|pulse| pulse.0 > 0);
        let mut epochs = lock(&self.epochs).take();
        let result = sion_cli::verify::verify_pulse_cached(
            &ledger,
            pinned,
            latest,
            settings.samples.max(1),
            false,
            &mut epochs,
        );
        if epochs.is_some() {
            *lock(&self.epochs) = epochs;
        }
        let verified = match result {
            Ok(mut report) => {
                let mut pinned_now = false;
                if pinned.is_none() {
                    let mut current = write(&self.settings);
                    if current.chain.is_none()
                        && current.node == settings.node
                        && current.pin_on_first_use
                    {
                        current.chain = Some(report.chain.to_hex());
                        self.save(&current)?;
                        pinned_now = true;
                    }
                }
                if let Some(first) = report.lines.first_mut() {
                    if pinned_now {
                        *first = format!(
                            "chain: {} pinned now, on first use: the node vouched for it; compare it with an identity from a source you trust (Settings)",
                            report.chain.short()
                        );
                    } else if pinned.is_none() {
                        *first = format!(
                            "chain: {} as the node serves it, not pinned: pin an identity you trust in Settings",
                            report.chain.short()
                        );
                    }
                }
                Verified {
                    pulse: Some(report.pulse.0),
                    ok: true,
                    pinned_now,
                    lines: report.lines,
                }
            }
            Err(error) => Verified {
                pulse: None,
                ok: false,
                pinned_now: false,
                lines: vec![error.to_string()],
            },
        };
        let mut history = lock(&self.history);
        history.push_front(verified.clone());
        history.truncate(HISTORY);
        Ok(json!(verified))
    }

    /// The verifications so far, newest first, and how many passed.
    pub fn history(&self) -> Value {
        let history = lock(&self.history);
        let passed = history.iter().filter(|v| v.ok).count();
        json!({
            "verified": passed,
            "failed": history.len() - passed,
            "latest": history.front(),
            "recent": history.iter().take(10).collect::<Vec<_>>(),
        })
    }

    /// Every validator with its pool.
    pub fn pools(&self) -> Result<Value, AppError> {
        let ledger = self.ledger();
        let session = self.session(&ledger)?;
        let names = sion_cli::stake::operator_names(&session);
        let set = sion_cli::stake::validator_set(&ledger)?;
        let book = sion_cli::stake::delegation_book(&ledger)?.unwrap_or_default();
        let mut validators = Vec::new();
        for (id, record) in &set.validators {
            let pool = book.pools.get(id).cloned().unwrap_or_default();
            validators.push(json!({
                "validator": id,
                "short": id.short(),
                "operator": names.account(&record.operator),
                "status": format!("{:?}", record.status),
                "own": record.stake.to_string(),
                "delegated": pool.bonded().unwrap_or_default().to_string(),
                "commission": sion_cli::describe::percent(pool.commission_permille),
                "next_commission": pool.next_commission.map(sion_cli::describe::percent),
                "weight": set.active.iter().find(|(v, _)| v == id).map(|(_, w)| *w),
            }));
        }
        Ok(json!({
            "epoch": set.epoch,
            "epoch_length": set.params.epoch_length,
            "validators": validators,
        }))
    }

    /// Delegates `amount` of stake to `validator` (id or unique prefix).
    pub fn delegate(&self, validator: &str, amount: &str) -> Result<Value, AppError> {
        let amount = sion_cli::agents::parse_amount(amount)?;
        let _turn = lock(&self.submitting);
        let ledger = self.ledger();
        let mut session = self.session(&ledger)?;
        let set = sion_cli::stake::validator_set(&ledger)?;
        let validator = sion_cli::stake::find_validator(&set, validator)?;
        self.submit(
            &mut session,
            TxKind::DelegateStake { validator, amount },
            Vec::new(),
        )
    }

    /// The device account's delegations, with what each is owed.
    pub fn delegations(&self) -> Result<Value, AppError> {
        Ok(sion_cli::stake::delegations_of(
            &self.ledger(),
            self.wallet.id,
        )?)
    }

    /// Collects, undelegates or withdraws a delegation: `action` is
    /// `claim`, `undelegate` or `withdraw`.
    pub fn delegation_action(&self, action: &str, delegation: &str) -> Result<Value, AppError> {
        let kind = match action {
            "claim" => TxKind::ClaimDelegatorRewards,
            "undelegate" => TxKind::Undelegate,
            "withdraw" => TxKind::WithdrawDelegation,
            other => return Err(AppError::Input(format!("unknown action {other:?}"))),
        };
        let _turn = lock(&self.submitting);
        let ledger = self.ledger();
        let mut session = self.session(&ledger)?;
        let id = sion_cli::stake::find_delegation(&ledger, self.wallet.id, delegation)?;
        let reference = session.reference(&id)?;
        self.submit(&mut session, kind, vec![reference])
    }

    /// Whether the device's storage provider also takes deals whose data is
    /// on this machine or its local network (see [`Settings::storage_local`]).
    pub fn set_storage_local(&self, allowed: bool) -> Result<Value, AppError> {
        let mut settings = write(&self.settings);
        settings.storage_local = allowed;
        self.save(&settings)?;
        Ok(json!(*settings))
    }

    /// Turns the device's storage provider on or off, and sets the most
    /// space one deal may take.
    pub fn set_storage(&self, enabled: bool, max_mib: u64) -> Result<Value, AppError> {
        if max_mib == 0 {
            return Err(AppError::Input("give a deal at least 1 MiB".into()));
        }
        let mut settings = write(&self.settings);
        settings.provide_storage = enabled;
        settings.storage_max_mib = max_mib;
        self.save(&settings)?;
        Ok(json!(*settings))
    }

    /// One step of the device's storage provider: when it is on, accept
    /// deals whose data it can fetch; on or off, prove the round under way
    /// for every deal it keeps, collect, and close deals that ended, so
    /// that turning it off never abandons a deal. Returns what it did, a
    /// line each.
    pub fn storage_tick(&self) -> Result<Value, AppError> {
        use sion_exec::storage::DealState;
        let (settings, ledger) = self.snapshot();
        if !settings.provide_storage {
            let mut keeps = false;
            for (_, record) in sion_cli::storage::custody_records(&ledger, self.wallet.id)? {
                if matches!(
                    sion_cli::storage::deal(&ledger, record.deal)?.state,
                    DealState::Active { .. }
                ) {
                    keeps = true;
                    break;
                }
            }
            if !keeps {
                return Ok(json!({ "enabled": false, "lines": [] }));
            }
        }
        let mut slot = lock(&self.provider);
        let provider = slot.get_or_insert_with(|| {
            Provider::new(
                ME,
                Store::new(self.dir.join("storage")),
                ProviderPolicy::default(),
            )
        });
        provider.policy.accept_open = settings.provide_storage;
        provider.policy.accept_named = settings.provide_storage;
        provider.policy.max_bytes = settings.storage_max_mib.saturating_mul(1 << 20);
        provider.policy.allow_local = settings.storage_local;
        provider.policy.allow_private = settings.storage_local;
        let _turn = lock(&self.submitting);
        let mut session = self.session(&ledger)?;
        let lines = provider.tick(&mut session, &ledger)?;
        Ok(json!({ "enabled": settings.provide_storage, "lines": lines }))
    }

    /// The deals this device keeps or kept: their terms, rounds proven
    /// and paid, and what they earned.
    pub fn storage_status(&self) -> Result<Value, AppError> {
        use sion_exec::storage::{current_round, DealState};
        let (settings, ledger) = self.snapshot();
        let pulse = ledger.pulse()?;
        let mut deals = Vec::new();
        let (mut collected, mut due, mut at_stake, mut kept) = (0u128, 0u128, 0u128, 0usize);
        for (custody_id, record) in sion_cli::storage::custody_records(&ledger, self.wallet.id)? {
            let deal = sion_cli::storage::deal(&ledger, record.deal)?;
            let price = deal.price.0;
            let (phase, paid) = match &deal.state {
                DealState::Active { paid, bond, .. } => {
                    at_stake += bond.0;
                    kept += 1;
                    ("active", *paid)
                }
                DealState::Closed { .. } => ("closed", record.proven),
                DealState::Open => ("open", 0),
            };
            collected += price.saturating_mul(u128::from(paid));
            due += price.saturating_mul(u128::from(record.proven.saturating_sub(paid)));
            deals.push(json!({
                "deal": record.deal,
                "short": record.deal.short(),
                "custody": custody_id,
                "leaves": record.size,
                "price": deal.price.to_string(),
                "rounds": record.rounds,
                "proven": record.proven,
                "round_now": current_round(record.start, record.round_length, pulse),
                "phase": phase,
                "closed": match &deal.state {
                    DealState::Closed { missed, forfeited, .. } => {
                        Some(json!({ "missed": missed, "forfeited": forfeited.to_string() }))
                    }
                    _ => None,
                },
            }));
        }
        let bytes: u64 = std::fs::read_dir(self.dir.join("storage"))
            .map(|entries| {
                entries
                    .filter_map(|e| e.ok()?.metadata().ok())
                    .map(|m| m.len())
                    .sum()
            })
            .unwrap_or(0);
        Ok(json!({
            "enabled": settings.provide_storage,
            "max_mib": settings.storage_max_mib,
            "kept": kept,
            "bytes": bytes,
            "collected": sion_core::Amount(collected).to_string(),
            "due": sion_core::Amount(due).to_string(),
            "at_stake": sion_core::Amount(at_stake).to_string(),
            "deals": deals,
        }))
    }

    /// The latest Pulse the node reports.
    pub fn latest_pulse(&self) -> Result<Pulse, AppError> {
        Ok(self.ledger().pulse()?)
    }
}

/// How long the app waits for the certificate of the Pulse its own
/// transaction executed in, before reporting the receipt as unproven.
const CERTIFICATE_WAIT: Duration = Duration::from_secs(20);

impl AppCore {
    /// Signs `kind` with the device's key, submits it, waits for it to
    /// execute, then proves its receipt as a light client.
    fn submit(
        &self,
        session: &mut Session<'_>,
        kind: TxKind,
        extra: Vec<sion_exec::object::ObjectRef>,
    ) -> Result<Value, AppError> {
        // The session waits for the transaction signed here (its digest
        // computed locally), so the receipt proven below is for it.
        let view = session.send(ME, kind, extra)?;
        let digest = view.effects.digest;
        match self.prove_receipt(session.ledger, view.pulse, digest) {
            Ok((verified, line)) => {
                let pinned = verified.chain.1;
                Ok(json!({
                    "pulse": verified.pulse,
                    "status": verified.effects.status,
                    // What happened, from the proven receipt only.
                    "receipt": sion_cli::describe::receipt(&session.names(), &verified.effects),
                    "digest": digest,
                    "proof": { "ok": pinned, "proven": true, "pinned": pinned, "line": line },
                }))
            }
            Err(line) => Ok(json!({
                "pulse": view.pulse,
                "status": view.status,
                "receipt": format!(
                    "The node reports, unproven: {}",
                    sion_cli::describe::receipt(&session.names(), &view.effects)
                ),
                "digest": digest,
                "proof": { "ok": false, "proven": false, "pinned": read(&self.settings).chain.is_some(), "line": line },
            })),
        }
    }

    /// Proves a receipt of the device's own transaction against its
    /// Pulse's certified header (the committee followed from the pinned
    /// chain identity), once that Pulse is certified. Returns the proven
    /// receipt and a line saying how, or why it is not proven.
    fn prove_receipt(
        &self,
        ledger: &dyn Ledger,
        pulse: Pulse,
        digest: TxDigest,
    ) -> Result<(sion_cli::verify::VerifiedReceipt, String), String> {
        let pinned = read(&self.settings)
            .chain
            .as_deref()
            .and_then(|hex| Hash32::from_hex(hex).ok());
        let deadline = Instant::now() + CERTIFICATE_WAIT;
        loop {
            let certified = ledger.status().ok().and_then(|status| {
                serde_json::from_value::<Pulse>(status["certified_pulse"].clone()).ok()
            });
            if certified.is_some_and(|c| c >= pulse) || Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        match sion_cli::verify::verify_receipt_of(ledger, pinned, digest) {
            Ok(verified) => {
                let certificate = match verified.signed {
                    Some((stake, quorum)) => format!(
                        "certified by {stake} of the committee's stake (quorum {quorum}), dual post-quantum signatures"
                    ),
                    None => "certificate skipped: the devnet runs insecure simulation signatures".to_string(),
                };
                let line = if verified.chain.1 {
                    format!(
                        "Receipt proven against Pulse {}'s header, {certificate}, chain {}",
                        verified.pulse,
                        verified.chain.0.short(),
                    )
                } else {
                    format!(
                        "Receipt proven only against the chain the node serves ({}), which is not pinned: pin an identity you trust in Settings. Pulse {}'s header, {certificate}",
                        verified.chain.0.short(),
                        verified.pulse,
                    )
                };
                Ok((verified, line))
            }
            Err(error) => Err(format!("Receipt not proven: {error}")),
        }
    }
}

/// The device's key: read from `path`, or created there with mode 0600.
fn load_or_create_seed(path: &Path) -> Result<[u8; 32], AppError> {
    if let Ok(text) = std::fs::read_to_string(path) {
        let bytes = hex::decode(text.trim()).map_err(|e| AppError::Storage(e.to_string()))?;
        return <[u8; 32]>::try_from(bytes.as_slice())
            .map_err(|_| AppError::Storage("the key file is not 32 bytes".into()));
    }
    let seed = sion_crypto::keys::random_seed();
    write_secret(path, &hex::encode(seed))?;
    Ok(seed)
}

#[cfg(unix)]
fn write_secret(path: &Path, text: &str) -> Result<(), AppError> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| AppError::Storage(e.to_string()))?;
    file.write_all(text.as_bytes())
        .map_err(|e| AppError::Storage(e.to_string()))
}

#[cfg(not(unix))]
fn write_secret(path: &Path, text: &str) -> Result<(), AppError> {
    std::fs::write(path, text).map_err(|e| AppError::Storage(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unreadable_settings_are_kept_aside_and_nothing_is_pinned_silently() {
        let dir =
            std::env::temp_dir().join(format!("sion-app-core-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Half a file, as a crash in the middle of an old-style write left.
        std::fs::write(dir.join("settings.json"), "{\"node\": \"http://10.0.0.").unwrap();
        let app = AppCore::open(&dir).unwrap();
        let settings = app.settings();
        assert!(settings["notice"]
            .as_str()
            .unwrap()
            .contains("could not be read"));
        assert_eq!(settings["pin_on_first_use"], false);
        assert!(dir.join("settings.json.unreadable").exists());
        // Pinning by hand clears the notice; unpinning allows first use again.
        app.pin_chain(Some(&"cd".repeat(32))).unwrap();
        assert!(app.settings()["notice"].is_null());
        assert_eq!(app.settings()["pin_on_first_use"], false);
        app.pin_chain(None).unwrap();
        assert_eq!(app.settings()["pin_on_first_use"], true);
        // Saved whole: what is on disk reads back.
        let reopened = AppCore::open(&dir).unwrap();
        assert!(reopened.settings()["notice"].is_null());
        assert!(!dir.join("settings.json.partial").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn payments_go_to_accounts_never_to_names() {
        let dir = std::env::temp_dir().join(format!("sion-app-core-send-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let app = AppCore::open(&dir).unwrap();
        let error = app.send("bob", "1").unwrap_err();
        assert!(error.to_string().contains("64 hex digits"), "{error}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_key_is_created_once_and_kept() {
        let dir = std::env::temp_dir().join(format!("sion-app-core-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let first = AppCore::open(&dir).unwrap().me();
        let again = AppCore::open(&dir).unwrap().me();
        assert_eq!(first, again);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join("key.seed"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let app = AppCore::open(&dir).unwrap();
        assert!(app.set_node("ftp://example").is_err());
        app.set_node("http://127.0.0.1:9999/").unwrap();
        assert!(app.pin_chain(Some("not hex")).is_err());
        app.pin_chain(Some(&"ab".repeat(32))).unwrap();
        let reopened = AppCore::open(&dir).unwrap();
        assert_eq!(reopened.settings()["node"], "http://127.0.0.1:9999");
        assert_eq!(reopened.settings()["chain"], "ab".repeat(32));
        reopened.pin_chain(None).unwrap();
        assert!(AppCore::open(&dir).unwrap().settings()["chain"].is_null());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_shell_may_share_the_core_between_threads() {
        fn shared<T: Send + Sync + 'static>() {}
        shared::<AppCore>();
    }

    #[test]
    fn a_failed_verification_is_recorded_and_pins_nothing() {
        let dir = std::env::temp_dir().join(format!("sion-app-core-down-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let app = AppCore::open(&dir).unwrap();
        // Nothing listens on port 9 (discard) on a test machine.
        app.set_node("http://127.0.0.1:9").unwrap();
        let verified = app.verify_latest().unwrap();
        assert_eq!(verified["ok"], false, "{verified}");
        assert_eq!(verified["pinned_now"], false);
        assert!(app.settings()["chain"].is_null());
        assert_eq!(app.history()["failed"], 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
