//! Storage deals (spec v1.12) from the command line: a client posts data
//! for safekeeping, a provider accepts it, proves every round that it still
//! holds it, and is paid for the rounds it proved.
//!
//! The ledger stores only the data's Merkle root; proofs publish the leaves
//! they answer with, so private data is encrypted before it is posted. A
//! provider fetches the data itself, from the deal's locator (`https://`, or
//! `file://` on the client's own machine) or a file it is given, checks that
//! it hashes to the root before it accepts, and keeps its copy under
//! `$SION_HOME/storage/<provider>`. [`Provider::tick`] is one step of a
//! provider's loop: prove the round under way for every deal it holds,
//! collect, close what ended, accept the deals whose data arrived, and start
//! fetching new ones in the background, within its terms
//! ([`ProviderPolicy`]: prices above the cost of proving, bounded
//! collateral, disk and duration, nothing local fetched for strangers).
//! `sion storage serve` runs it every second; the app and the `storage` demo
//! use it too.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use clap::Subcommand;
use serde_json::{json, Value};
use sion_core::hash::Hash32;
use sion_core::ids::{AccountId, ObjectId, Pulse};
use sion_core::Amount;
use sion_exec::execute::Event;
use sion_exec::object::{well_known, ObjectData, ObjectRef};
use sion_exec::storage::{
    current_round, Custody, CustodyProof, DataTree, Deal, DealState, MAX_PROOF_LEAVES,
};
use sion_exec::TxKind;
use sion_node::Query;

use crate::error::CliError;
use crate::ledger::{Ledger, LedgerExt};
use crate::session::{wallet_dir, Session};

/// `sion storage …`
#[derive(Debug, Subcommand)]
pub enum StorageCommand {
    /// Split a file into leaves and print the root a deal would commit to.
    Prepare {
        /// The file.
        file: PathBuf,
    },
    /// Post a deal for a file: escrow PRICE × ROUNDS to have it kept and
    /// proven every round.
    Post {
        /// Your wallet (the client).
        from: String,
        /// The file to have kept.
        file: PathBuf,
        /// Paid per proven round (decimal USD).
        #[arg(long)]
        price: String,
        /// Rounds.
        #[arg(long)]
        rounds: u32,
        /// Pulses per round: a multiple of 8, from 8 to 100,000.
        #[arg(long, default_value_t = 16)]
        round_length: u64,
        /// Collateral the provider must post (decimal USD). Default: the
        /// escrow, for a deal open to anyone (which must ask for some); none
        /// for a named provider.
        #[arg(long)]
        collateral: Option<String>,
        /// Leaves challenged per round (1 to 16).
        #[arg(long, default_value_t = 4)]
        challenges: u8,
        /// The provider (wallet name or account); without it, anyone may
        /// accept.
        #[arg(long)]
        provider: Option<String>,
        /// How many Pulses the deal stays open for acceptance.
        #[arg(long, default_value_t = 200)]
        accept_within: u64,
        /// Where providers fetch the data: an https:// URL serving the file.
        #[arg(long, conflicts_with = "local")]
        locator: Option<String>,
        /// Point providers at the file itself (file://<its path>): only a
        /// provider on this machine can fetch it, and the deal publishes the
        /// path.
        #[arg(long)]
        local: bool,
    },
    /// List storage deals.
    Deals {
        /// Only deals still open for acceptance.
        #[arg(long)]
        open: bool,
    },
    /// One deal: its terms, its state and its provider's progress.
    Show {
        /// The deal (its id, or a unique prefix of it).
        deal: String,
    },
    /// Accept a deal as its provider: fetch its data (from --file or its
    /// locator), check it hashes to the deal's root, keep a copy and post
    /// the collateral.
    Accept {
        /// Your wallet (the provider).
        from: String,
        /// The deal (its id, or a unique prefix of it).
        deal: String,
        /// Take the data from this file instead of the locator.
        #[arg(long)]
        file: Option<PathBuf>,
    },
    /// Prove the round under way for every deal you hold.
    Prove {
        /// Your wallet (the provider).
        from: String,
    },
    /// Collect for the rounds you proved.
    Settle {
        /// Your wallet (the provider).
        from: String,
        /// The deal (its id, or a unique prefix of it).
        deal: String,
    },
    /// Close a deal: refund an open one (client), or end an accepted one
    /// at its end (client or provider) or once its provider abandoned it
    /// (client).
    Close {
        /// Your wallet.
        from: String,
        /// The deal (its id, or a unique prefix of it).
        deal: String,
    },
    /// Run a provider: accept deals whose data you can fetch, prove every
    /// round, collect and close, every second until stopped.
    Serve {
        /// Your wallet (the provider).
        from: String,
        /// Accept only deals paying at least this per round (decimal USD),
        /// and at least twice what proving a round costs in fees.
        #[arg(long, default_value = "0.01")]
        min_price: String,
        /// Accept only deals asking at most this collateral (decimal USD).
        #[arg(long, default_value = "10")]
        max_collateral: String,
        /// The most collateral at stake across all deals (decimal USD).
        #[arg(long, default_value = "100")]
        max_total_collateral: String,
        /// Accept only data of at most this many MiB.
        #[arg(long, default_value_t = 64)]
        max_mib: u64,
        /// Keep at most this many MiB in all.
        #[arg(long, default_value_t = 1024)]
        max_total_mib: u64,
        /// Accept only deals lasting at most this many Pulses.
        #[arg(long, default_value_t = 100_000)]
        max_duration: u64,
        /// Fetch file:// locators (a provider on the clients' machine).
        #[arg(long)]
        allow_local: bool,
        /// Fetch from local and private network addresses.
        #[arg(long)]
        allow_private: bool,
        /// Stop after this many Pulses.
        #[arg(long)]
        pulses: Option<u64>,
    },
}

/// Where providers keep their copies: `$SION_HOME/storage` (default
/// `~/.sion/storage`).
pub fn storage_dir() -> PathBuf {
    wallet_dir()
        .parent()
        .map(|home| home.join("storage"))
        .unwrap_or_else(|| PathBuf::from(".sion/storage"))
}

/// A provider's copies of the data it keeps, one file per deal.
#[derive(Clone, Debug)]
pub struct Store {
    dir: PathBuf,
}

impl Store {
    /// The store in `dir`.
    pub fn new(dir: impl Into<PathBuf>) -> Store {
        Store { dir: dir.into() }
    }

    fn path(&self, deal: &ObjectId) -> PathBuf {
        self.dir.join(format!("{}.bin", deal.0.to_hex()))
    }

    /// Keeps a copy of `data` for `deal` (written whole, then renamed into
    /// place, so a crash never leaves half a copy).
    pub fn put(&self, deal: &ObjectId, data: &[u8]) -> Result<(), CliError> {
        std::fs::create_dir_all(&self.dir).map_err(|e| CliError::Usage(e.to_string()))?;
        let path = self.path(deal);
        let partial = path.with_extension("partial");
        std::fs::write(&partial, data)
            .and_then(|()| std::fs::rename(&partial, &path))
            .map_err(|e| CliError::Usage(e.to_string()))
    }

    /// The bytes the copies take.
    pub fn usage(&self) -> u64 {
        std::fs::read_dir(&self.dir)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .filter_map(|entry| entry.metadata().ok())
                    .filter(|metadata| metadata.is_file())
                    .map(|metadata| metadata.len())
                    .sum()
            })
            .unwrap_or(0)
    }

    /// The copy for `deal`, if this provider keeps one.
    pub fn get(&self, deal: &ObjectId) -> Option<Vec<u8>> {
        std::fs::read(self.path(deal)).ok()
    }

    /// Deletes the copy for `deal` (a provider that loses data).
    pub fn forget(&self, deal: &ObjectId) {
        let _ = std::fs::remove_file(self.path(deal));
    }
}

/// The storage deals on the ledger: with `open`, only those that can still
/// be accepted.
pub fn deals(ledger: &dyn Ledger, open: bool) -> Result<Vec<(ObjectId, Deal)>, CliError> {
    let value = ledger.query(Query::Deals { open })?;
    let list = value["deals"].as_array().cloned().unwrap_or_default();
    list.into_iter()
        .map(|entry| {
            let id: ObjectId = serde_json::from_value(entry["id"].clone())
                .map_err(|e| CliError::Protocol(e.to_string()))?;
            let deal: Deal = serde_json::from_value(entry["deal"].clone())
                .map_err(|e| CliError::Protocol(e.to_string()))?;
            Ok((id, deal))
        })
        .collect()
}

/// One deal.
pub fn deal(ledger: &dyn Ledger, id: ObjectId) -> Result<Deal, CliError> {
    match ledger.object(id)?.map(|o| o.data) {
        Some(ObjectData::Deal(deal)) => Ok(*deal),
        _ => Err(CliError::Usage(format!(
            "{} is not a storage deal",
            id.short()
        ))),
    }
}

/// A deal by full id or unique prefix.
pub fn find_deal(ledger: &dyn Ledger, text: &str) -> Result<ObjectId, CliError> {
    let text = text.trim().to_lowercase();
    if let Ok(hash) = Hash32::from_hex(&text) {
        return Ok(ObjectId::from_hash(hash));
    }
    let matches: Vec<ObjectId> = deals(ledger, false)?
        .into_iter()
        .map(|(id, _)| id)
        .filter(|id| id.0.to_hex().starts_with(&text))
        .collect();
    match matches.as_slice() {
        [one] => Ok(*one),
        [] => Err(CliError::Usage(format!("no deal starts with {text:?}"))),
        _ => Err(CliError::Usage(format!(
            "{} deals start with {text:?}; give more digits",
            matches.len()
        ))),
    }
}

/// The custody records `provider` holds, by id.
pub fn custody_records(
    ledger: &dyn Ledger,
    provider: AccountId,
) -> Result<Vec<(ObjectId, Custody)>, CliError> {
    Ok(ledger
        .account(provider)?
        .custody
        .into_iter()
        .filter_map(|object| match object.data {
            ObjectData::Custody(custody) => Some((object.id, custody)),
            _ => None,
        })
        .collect())
}

/// How a provider may fetch a deal's data.
#[derive(Clone, Copy, Debug)]
pub struct FetchOptions {
    /// The most bytes taken.
    pub max_bytes: u64,
    /// Accept `file://` locators (a provider on the client's own machine).
    /// Off for a provider that takes strangers' deals: a locator would make
    /// it read its own files.
    pub allow_local: bool,
    /// Fetch from loopback, private and other local addresses. Off for a
    /// provider that takes strangers' deals: a locator would make it probe
    /// its own network.
    pub allow_private: bool,
}

/// Fetches data from a locator, `file://<absolute path>` (if allowed) or
/// `http(s)://<url>`, at most `options.max_bytes`. HTTP has a 5-second
/// connect timeout, a 30-second deadline, no redirects, and refuses local
/// and private addresses unless allowed ([`sion_http`]); a file must be a
/// regular file.
pub fn fetch(locator: &str, options: FetchOptions) -> Result<Vec<u8>, CliError> {
    let shown = printable(locator);
    if let Some(path) = locator.strip_prefix("file://") {
        if !options.allow_local {
            return Err(CliError::Usage(format!(
                "{shown}: file:// locators are refused (only a provider on the client's machine takes them)"
            )));
        }
        let path = Path::new(path);
        if !path.is_absolute() {
            return Err(CliError::Usage(format!("{shown}: not an absolute path")));
        }
        // The metadata follows links; anything but a regular file (a
        // device, a FIFO, a directory) is refused before it is opened.
        let metadata =
            std::fs::metadata(path).map_err(|e| CliError::Usage(format!("{shown}: {e}")))?;
        if !metadata.is_file() {
            return Err(CliError::Usage(format!("{shown}: not a regular file")));
        }
        if metadata.len() > options.max_bytes {
            return Err(CliError::Usage(format!(
                "{shown} holds {} bytes, more than {}",
                metadata.len(),
                options.max_bytes
            )));
        }
        let mut data = Vec::new();
        std::fs::File::open(path)
            .and_then(|file| {
                file.take(options.max_bytes.saturating_add(1))
                    .read_to_end(&mut data)
            })
            .map_err(|e| CliError::Usage(format!("{shown}: {e}")))?;
        if data.len() as u64 > options.max_bytes {
            return Err(CliError::Usage(format!(
                "{shown} holds more than {} bytes",
                options.max_bytes
            )));
        }
        return Ok(data);
    }
    if locator.starts_with("http://") || locator.starts_with("https://") {
        let client = sion_http::Client::new(sion_http::Options {
            allow_private: options.allow_private,
            ..sion_http::Options::default()
        })
        .map_err(|e| CliError::Usage(e.to_string()))?;
        return client
            .get(locator, options.max_bytes)
            .map(|fetched| fetched.body)
            .map_err(|e| CliError::Usage(format!("{shown}: {e}")));
    }
    Err(CliError::Usage(format!(
        "cannot fetch {shown}: the locator must be http(s):// (or file:// on the client's machine)"
    )))
}

/// `text` safe to print: control characters replaced, at most 160
/// characters (locators are written by strangers).
pub fn printable(text: &str) -> String {
    let mut shown: String = text
        .chars()
        .take(160)
        .map(|c| if c.is_control() { '?' } else { c })
        .collect();
    if text.chars().count() > 160 {
        shown.push('…');
    }
    shown
}

/// The beacon of `pulse` once the chain has recorded it: what the
/// challenge of a round starting at `pulse` is drawn from (spec §5.52).
/// `None` before that Pulse has executed, or for a Pulse that is not
/// recorded (not a multiple of the beacon interval, or too old).
pub fn recorded_beacon(ledger: &dyn Ledger, pulse: Pulse) -> Result<Option<Hash32>, CliError> {
    let Some(slot) = sion_exec::beacons::slot_of(pulse) else {
        return Ok(None);
    };
    Ok(
        match ledger.object(well_known::beacon(slot))?.map(|o| o.data) {
            Some(ObjectData::Beacon(record)) if record.pulse == pulse => Some(record.beacon),
            _ => None,
        },
    )
}

/// Whether `data` is what `deal` commits to.
pub fn matches_deal(data: &[u8], deal: &Deal) -> bool {
    let tree = DataTree::of_data(data);
    tree.root() == deal.root && tree.size() == deal.size
}

/// The most bytes a deal's data can hold: every leaf full.
pub fn deal_bytes(deal: &Deal) -> u64 {
    deal.size
        .saturating_mul(sion_exec::storage::MAX_LEAF_BYTES as u64)
}

/// What proving one round of `deal` costs in fees, at least: a transaction
/// carrying its challenged leaves (full ones) and their paths, plus room
/// for the signatures and inputs.
pub fn round_fee(config: &sion_exec::state::ChainConfig, deal: &Deal) -> Amount {
    let depth = 64 - deal.size.saturating_sub(1).leading_zeros() as usize;
    let per_leaf = sion_exec::storage::MAX_LEAF_BYTES + 32 * depth + 16;
    let bytes = PROOF_OVERHEAD + usize::from(deal.challenges) * per_leaf;
    config.min_fee(bytes)
}

/// Room left in a proof transaction for the signatures, inputs and the
/// rest, in bytes.
const PROOF_OVERHEAD: usize = 8 * 1024;

/// The most proof bytes one transaction carries: the gateway refuses
/// transactions over 256 KiB, and signatures and inputs need room.
pub const MAX_PROOF_BYTES: usize = 192 * 1024;

/// How many fetches a provider runs at once.
const MAX_FETCHES: usize = 2;

/// How long a fetch may run before the provider gives up on its deal.
const FETCH_DEADLINE: std::time::Duration = std::time::Duration::from_secs(60);

/// The terms a provider takes deals on. The defaults suit a phone or a
/// laptop taking strangers' deals: modest collateral, short deals, bounded
/// disk, and nothing local fetched.
#[derive(Clone, Debug)]
pub struct ProviderPolicy {
    /// Accept open deals.
    pub accept_open: bool,
    /// Accept deals that name this provider. With both off, the provider
    /// takes nothing new but keeps proving, collecting and closing the
    /// deals it holds.
    pub accept_named: bool,
    /// The lowest price per round taken; deals must also pay at least twice
    /// what proving a round costs in fees ([`round_fee`]).
    pub min_price: Amount,
    /// The most collateral posted for one deal.
    pub max_collateral: Amount,
    /// The most collateral at stake across every deal held.
    pub max_total_collateral: Amount,
    /// The most bytes kept for one deal.
    pub max_bytes: u64,
    /// The most bytes kept across every deal.
    pub max_total_bytes: u64,
    /// The longest deal taken, in Pulses (rounds × round length): a deal
    /// cannot be left early, and a provider that stops proving for
    /// [`sion_exec::storage::ABANDON_ROUNDS`] rounds loses its collateral.
    pub max_duration: u64,
    /// Fetch `file://` locators (see [`FetchOptions::allow_local`]).
    pub allow_local: bool,
    /// Fetch from local and private addresses (see
    /// [`FetchOptions::allow_private`]).
    pub allow_private: bool,
    /// Collect once this many proven rounds are unpaid (closing collects
    /// the rest).
    pub settle_every: u32,
}

impl Default for ProviderPolicy {
    fn default() -> Self {
        ProviderPolicy {
            accept_open: true,
            accept_named: true,
            min_price: Amount(10_000),
            max_collateral: Amount::units(10),
            max_total_collateral: Amount::units(100),
            max_bytes: 64 << 20,
            max_total_bytes: 1 << 30,
            max_duration: 100_000,
            allow_local: false,
            allow_private: false,
            settle_every: 4,
        }
    }
}

impl ProviderPolicy {
    /// Why `deal` is not taken under these terms, if it is not (the budgets
    /// aside).
    pub fn refuses(
        &self,
        config: &sion_exec::state::ChainConfig,
        deal: &Deal,
        me: AccountId,
    ) -> Option<String> {
        let named = deal.provider == Some(me);
        if !(named && self.accept_named || deal.provider.is_none() && self.accept_open) {
            return Some("not taking such deals".into());
        }
        let floor = self
            .min_price
            .max(Amount(round_fee(config, deal).0.saturating_mul(2)));
        if deal.price < floor {
            return Some(format!("pays {} a round, less than {floor}", deal.price));
        }
        if deal.collateral > self.max_collateral {
            return Some(format!("asks {} of collateral", deal.collateral));
        }
        if deal_bytes(deal) > self.max_bytes {
            return Some(format!("may hold {} bytes", deal_bytes(deal)));
        }
        let duration = u64::from(deal.rounds).saturating_mul(deal.round_length);
        if duration > self.max_duration {
            return Some(format!("lasts {duration} Pulses"));
        }
        None
    }

    /// How the provider fetches.
    pub fn fetch_options(&self) -> FetchOptions {
        FetchOptions {
            max_bytes: self.max_bytes,
            allow_local: self.allow_local,
            allow_private: self.allow_private,
        }
    }
}

/// A fetch under way, off the proving path.
#[derive(Debug)]
struct Fetch {
    deal: Deal,
    started: std::time::Instant,
    handle: std::thread::JoinHandle<Result<Vec<u8>, String>>,
}

/// A storage provider: its wallet, its copies and its terms.
#[derive(Debug)]
pub struct Provider {
    /// The provider's wallet name in the session.
    pub name: String,
    /// Its copies.
    pub store: Store,
    /// Its terms.
    pub policy: ProviderPolicy,
    /// Trees of the copies it holds, built once per deal.
    trees: BTreeMap<ObjectId, DataTree>,
    /// Deals it will not take, and why, so it does not try again.
    skipped: BTreeMap<ObjectId, String>,
    /// Fetches under way.
    fetches: BTreeMap<ObjectId, Fetch>,
    /// Deals it held that are closed: their custody records stay, but there
    /// is nothing left to do for them.
    finished: std::collections::BTreeSet<ObjectId>,
}

impl Provider {
    /// A provider for wallet `name`.
    pub fn new(name: &str, store: Store, policy: ProviderPolicy) -> Provider {
        Provider {
            name: name.to_string(),
            store,
            policy,
            trees: BTreeMap::new(),
            skipped: BTreeMap::new(),
            fetches: BTreeMap::new(),
            finished: std::collections::BTreeSet::new(),
        }
    }

    /// One step: prove the round under way for every deal it holds (first,
    /// so nothing else can delay it), collect and close, accept the deals
    /// whose data finished fetching, and start fetching new ones. Fetches
    /// run in the background with deadlines: a locator that never answers
    /// costs a fetch slot, never a round. Returns what it did, a line each.
    pub fn tick(
        &mut self,
        session: &mut Session<'_>,
        ledger: &dyn Ledger,
    ) -> Result<Vec<String>, CliError> {
        let mut done = self.prove(session, ledger)?;
        done.extend(self.collect(session, ledger)?);
        done.extend(self.finish_fetches(session, ledger)?);
        done.extend(self.start_fetches(session, ledger)?);
        Ok(done)
    }

    /// Whether fetches are under way.
    pub fn fetching(&self) -> usize {
        self.fetches.len()
    }

    /// Collects for proven rounds once enough are unpaid, closes the deals
    /// that ended, and lets go of the copies of closed deals.
    fn collect(
        &mut self,
        session: &mut Session<'_>,
        ledger: &dyn Ledger,
    ) -> Result<Vec<String>, CliError> {
        let mut done = Vec::new();
        let me = session.id(&self.name);
        let pulse = ledger.pulse()?;
        for (_, record) in custody_records(ledger, me)? {
            let id = record.deal;
            if self.finished.contains(&id) {
                continue;
            }
            let Ok(terms) = deal(ledger, id) else {
                continue;
            };
            match terms.state {
                DealState::Active { provider, paid, .. } if provider == me => {
                    if terms.end().is_some_and(|end| pulse >= end) {
                        let line =
                            self.send_line(session, TxKind::CloseDeal { deal: id }, Vec::new());
                        if matches!(
                            deal(ledger, id).map(|d| d.state),
                            Ok(DealState::Closed { .. })
                        ) {
                            self.let_go(&id);
                        }
                        done.push(line);
                    } else if record.proven.saturating_sub(paid) >= self.policy.settle_every.max(1)
                    {
                        done.push(self.send_line(
                            session,
                            TxKind::SettleDeal { deal: id },
                            Vec::new(),
                        ));
                    }
                }
                DealState::Closed { .. } => self.let_go(&id),
                _ => {}
            }
        }
        Ok(done)
    }

    /// Forgets a closed deal and deletes its copy.
    fn let_go(&mut self, id: &ObjectId) {
        self.finished.insert(*id);
        self.trees.remove(id);
        self.store.forget(id);
    }

    /// The collateral at stake and the bytes kept or being fetched.
    fn commitments(&self, ledger: &dyn Ledger, me: AccountId) -> Result<(Amount, u64), CliError> {
        let mut collateral = Amount::ZERO;
        for (_, record) in custody_records(ledger, me)? {
            if self.finished.contains(&record.deal) {
                continue;
            }
            if let Ok(Deal {
                state: DealState::Active { bond, provider, .. },
                ..
            }) = deal(ledger, record.deal)
            {
                if provider == me {
                    collateral = collateral.saturating_add(bond);
                }
            }
        }
        let mut bytes = self.store.usage();
        for fetch in self.fetches.values() {
            collateral = collateral.saturating_add(fetch.deal.collateral);
            bytes = bytes.saturating_add(deal_bytes(&fetch.deal));
        }
        Ok((collateral, bytes))
    }

    /// Starts fetching the open deals it would take, within its budgets.
    fn start_fetches(
        &mut self,
        session: &mut Session<'_>,
        ledger: &dyn Ledger,
    ) -> Result<Vec<String>, CliError> {
        if !(self.policy.accept_open || self.policy.accept_named)
            || self.fetches.len() >= MAX_FETCHES
        {
            return Ok(Vec::new());
        }
        let me = session.id(&self.name);
        let pulse = ledger.pulse()?;
        let (mut collateral, mut bytes) = self.commitments(ledger, me)?;
        let mut done = Vec::new();
        for (id, terms) in deals(ledger, true)? {
            if self.fetches.len() >= MAX_FETCHES {
                break;
            }
            if self.skipped.contains_key(&id)
                || self.fetches.contains_key(&id)
                || self.finished.contains(&id)
                // Leave time to fetch and accept.
                || terms.accept_by.0 < pulse.0.saturating_add(4)
            {
                continue;
            }
            if let Some(reason) = self.policy.refuses(&session.config, &terms, me) {
                self.skipped.insert(id, reason);
                continue;
            }
            let more_collateral = collateral.saturating_add(terms.collateral);
            let more_bytes = bytes.saturating_add(deal_bytes(&terms));
            if more_collateral > self.policy.max_total_collateral
                || more_bytes > self.policy.max_total_bytes
            {
                // Over budget for now; it may fit once other deals end.
                continue;
            }
            collateral = more_collateral;
            bytes = more_bytes;
            let locator = terms.locator.clone();
            let options = FetchOptions {
                max_bytes: deal_bytes(&terms).min(self.policy.max_bytes),
                ..self.policy.fetch_options()
            };
            let handle =
                std::thread::spawn(move || fetch(&locator, options).map_err(|e| e.to_string()));
            done.push(format!(
                "fetching deal {} from {}",
                id.short(),
                printable(&terms.locator)
            ));
            self.fetches.insert(
                id,
                Fetch {
                    deal: terms,
                    started: std::time::Instant::now(),
                    handle,
                },
            );
        }
        Ok(done)
    }

    /// Accepts the deals whose data arrived and matches; gives up on fetches
    /// that failed or ran past their deadline.
    fn finish_fetches(
        &mut self,
        session: &mut Session<'_>,
        ledger: &dyn Ledger,
    ) -> Result<Vec<String>, CliError> {
        let mut done = Vec::new();
        let ids: Vec<ObjectId> = self.fetches.keys().copied().collect();
        for id in ids {
            let finished = self.fetches[&id].handle.is_finished();
            if !finished {
                if self.fetches[&id].started.elapsed() > FETCH_DEADLINE {
                    // The thread is left to end on its own deadline.
                    self.fetches.remove(&id);
                    self.skipped.insert(id, "the fetch took too long".into());
                    done.push(format!(
                        "skipped deal {}: the fetch took too long",
                        id.short()
                    ));
                }
                continue;
            }
            let fetch = self.fetches.remove(&id).expect("listed");
            let outcome = match fetch.handle.join() {
                Ok(Ok(data)) => self.accept(session, ledger, id, &fetch.deal, data),
                Ok(Err(error)) => Err(CliError::Usage(error)),
                Err(_) => Err(CliError::Usage("the fetch failed".into())),
            };
            match outcome {
                Ok(line) => done.push(line),
                Err(error) => {
                    let reason = printable(&error.to_string());
                    done.push(format!("skipped deal {}: {reason}", id.short()));
                    self.skipped.insert(id, reason);
                }
            }
        }
        Ok(done)
    }

    /// Sends `kind` and describes what happened in one line, an error
    /// included: one action that does not go through must not stop the
    /// others.
    fn send_line(&self, session: &mut Session<'_>, kind: TxKind, extra: Vec<ObjectRef>) -> String {
        match session.send(&self.name, kind, extra) {
            Ok(view) => format!(
                "Pulse {}: {}",
                view.pulse,
                crate::describe::receipt(&session.names(), &view.effects)
            ),
            Err(error) => format!("not done: {error}"),
        }
    }

    /// Accepts `deal` by hand: fetches its data (from `file`, or its
    /// locator, `file://` included: the person chose this deal), checks it,
    /// posts the collateral and keeps a copy.
    pub fn take(
        &mut self,
        session: &mut Session<'_>,
        ledger: &dyn Ledger,
        id: ObjectId,
        deal: &Deal,
        file: Option<&Path>,
    ) -> Result<String, CliError> {
        let data = match file {
            Some(path) => std::fs::read(path)
                .map_err(|e| CliError::Usage(format!("{}: {e}", path.display())))?,
            None => fetch(
                &deal.locator,
                FetchOptions {
                    allow_local: true,
                    ..self.policy.fetch_options()
                },
            )?,
        };
        self.accept(session, ledger, id, deal, data)
    }

    /// Checks `data` against `deal`, accepts it, and keeps the copy once
    /// the acceptance went through (a refused one leaves nothing behind).
    fn accept(
        &mut self,
        session: &mut Session<'_>,
        _ledger: &dyn Ledger,
        id: ObjectId,
        deal: &Deal,
        data: Vec<u8>,
    ) -> Result<String, CliError> {
        let tree = DataTree::of_data(&data);
        if tree.root() != deal.root || tree.size() != deal.size {
            return Err(CliError::Usage(format!(
                "the data does not hash to the deal's root ({} of {} leaves, root {} instead of {})",
                tree.size(),
                deal.size,
                tree.root().short(),
                deal.root.short()
            )));
        }
        let view = session.send(&self.name, TxKind::AcceptDeal { deal: id }, Vec::new())?;
        if view.effects.status != sion_exec::ExecStatus::Success {
            return Err(CliError::Transaction(crate::describe::receipt(
                &session.names(),
                &view.effects,
            )));
        }
        self.store.put(&id, &data)?;
        self.trees.insert(id, tree);
        Ok(format!(
            "Pulse {}: {}",
            view.pulse,
            crate::describe::receipt(&session.names(), &view.effects)
        ))
    }

    /// Proves the round under way for every deal this provider holds a
    /// copy of, in as few transactions as the leaf and size limits allow.
    pub fn prove(
        &mut self,
        session: &mut Session<'_>,
        ledger: &dyn Ledger,
    ) -> Result<Vec<String>, CliError> {
        let me = session.id(&self.name);
        let latest = ledger.pulse()?;
        let next = latest.next();
        // Each batch remembers the last Pulse of the earliest round it
        // proves: the transaction expires then, so a proof that comes too
        // late is rejected (no fee) rather than failed.
        struct Batch {
            refs: Vec<ObjectRef>,
            proofs: Vec<CustodyProof>,
            leaves: usize,
            bytes: usize,
            round_end: u64,
        }
        let empty = || Batch {
            refs: Vec::new(),
            proofs: Vec::new(),
            leaves: 0,
            bytes: 0,
            round_end: u64::MAX,
        };
        let mut batches = vec![empty()];
        for (custody_id, custody) in custody_records(ledger, me)? {
            if self.finished.contains(&custody.deal) {
                continue;
            }
            let Some(round) = current_round(custody.start, custody.round_length, next) else {
                continue;
            };
            if round >= u64::from(custody.rounds) || round < u64::from(custody.next) {
                continue;
            }
            // The round's challenge is known once its first Pulse has
            // executed and the chain recorded its beacon.
            let first = custody.round_start(round as u32);
            if first > latest {
                continue;
            }
            let active = matches!(
                deal(ledger, custody.deal).map(|d| d.state),
                Ok(DealState::Active { .. })
            );
            if !active {
                continue;
            }
            let Some(beacon) = recorded_beacon(ledger, first)? else {
                continue;
            };
            let Some(data) = self.store.get(&custody.deal) else {
                continue;
            };
            let tree = self
                .trees
                .entry(custody.deal)
                .or_insert_with(|| DataTree::of_data(&data));
            let Some(proof) = tree.answer(&data, &custody, round as u32, &beacon) else {
                continue;
            };
            let bytes = sion_core::encode::encode(&proof).len();
            let current = batches.last().expect("one batch");
            if current.leaves + proof.leaves.len() > MAX_PROOF_LEAVES
                || current.bytes + bytes > MAX_PROOF_BYTES
            {
                batches.push(empty());
            }
            let round_end = custody
                .start
                .0
                .saturating_add((round + 1).saturating_mul(custody.round_length))
                .saturating_sub(1);
            let batch = batches.last_mut().expect("one batch");
            batch.leaves += proof.leaves.len();
            batch.bytes += bytes;
            batch.refs.push(session.reference(&custody_id)?);
            batch.proofs.push(proof);
            batch.round_end = batch.round_end.min(round_end);
        }
        let mut done = Vec::new();
        let now = ledger.pulse()?.0;
        let usual = session.wallet(&self.name).validity;
        for batch in batches.into_iter().filter(|b| !b.refs.is_empty()) {
            session.wallet(&self.name).validity = batch.round_end.saturating_sub(now).min(usual);
            done.push(self.send_line(
                session,
                TxKind::ProveStorage {
                    proofs: batch.proofs,
                },
                batch.refs,
            ));
        }
        session.wallet(&self.name).validity = usual;
        Ok(done)
    }
}

/// A deal as JSON for display: its terms, state and, once accepted, the
/// provider's progress.
pub fn deal_json(ledger: &dyn Ledger, id: ObjectId, deal: &Deal) -> Result<Value, CliError> {
    let pulse = ledger.pulse()?;
    let mut value = json!({
        "deal": id,
        "client": deal.client,
        "provider": deal.provider,
        "root": deal.root,
        "leaves": deal.size,
        "challenges": deal.challenges,
        "price": deal.price.to_string(),
        "rounds": deal.rounds,
        "round_length": deal.round_length,
        "collateral": deal.collateral.to_string(),
        "escrow": deal.escrow.to_string(),
        "accept_by": deal.accept_by,
        "locator": deal.locator,
        "state": match &deal.state {
            DealState::Open => json!({ "phase": "open" }),
            DealState::Active {
                provider,
                custody,
                start,
                bond,
                paid,
            } => json!({
                "phase": "active",
                "provider": provider,
                "custody": custody,
                "start": start,
                "bond": bond.to_string(),
                "paid_rounds": paid,
            }),
            DealState::Closed {
                at,
                proven,
                missed,
                refund,
                forfeited,
            } => json!({
                "phase": "closed",
                "at": at,
                "proven": proven,
                "missed": missed,
                "refund": refund.to_string(),
                "forfeited": forfeited.to_string(),
            }),
        },
    });
    if let DealState::Active { custody, .. } = deal.state {
        if let Some(ObjectData::Custody(record)) = ledger.object(custody)?.map(|o| o.data) {
            value["progress"] = json!({
                "round_now": current_round(record.start, record.round_length, pulse),
                "proven": record.proven,
                "next": record.next,
                "end": deal.end(),
            });
        }
    }
    Ok(value)
}

/// Runs a storage command.
pub fn run(
    session: &mut Session<'_>,
    ledger: &dyn Ledger,
    command: StorageCommand,
) -> Result<(), CliError> {
    match command {
        StorageCommand::Prepare { file } => {
            let data = std::fs::read(&file)
                .map_err(|e| CliError::Usage(format!("{}: {e}", file.display())))?;
            let tree = DataTree::of_data(&data);
            println!(
                "{}: {} bytes in {} leaves, root {}",
                file.display(),
                data.len(),
                tree.size(),
                tree.root()
            );
            Ok(())
        }
        StorageCommand::Post {
            from,
            file,
            price,
            rounds,
            round_length,
            collateral,
            challenges,
            provider,
            accept_within,
            locator,
            local,
        } => {
            let data = std::fs::read(&file)
                .map_err(|e| CliError::Usage(format!("{}: {e}", file.display())))?;
            let tree = DataTree::of_data(&data);
            let locator = match (locator, local) {
                (Some(locator), _) => locator,
                (None, true) => format!(
                    "file://{}",
                    std::fs::canonicalize(&file)
                        .map_err(|e| CliError::Usage(e.to_string()))?
                        .display()
                ),
                (None, false) => {
                    return Err(CliError::Usage(
                        "give --locator <https URL serving the file>, or --local for a provider on this machine (the deal then publishes the file's path)".into(),
                    ))
                }
            };
            let price_amount = crate::agents::parse_amount(&price)?;
            let collateral = match (collateral, &provider) {
                (Some(text), _) => crate::agents::parse_amount(&text)?,
                (None, Some(_)) => Amount::ZERO,
                (None, None) => Amount(price_amount.0.saturating_mul(u128::from(rounds))),
            };
            let provider = provider.map(|p| session.id(&p));
            let accept_by = Pulse(ledger.pulse()?.0 + accept_within.max(1));
            let view = session.send(
                &from,
                TxKind::PostDeal {
                    root: tree.root(),
                    size: tree.size(),
                    challenges,
                    price: price_amount,
                    rounds,
                    round_length,
                    collateral,
                    provider,
                    accept_by,
                    locator,
                },
                Vec::new(),
            )?;
            crate::agents::print_receipt(session, &view);
            for event in &view.effects.events {
                if let Event::DealPosted { deal, .. } = event {
                    println!("deal {deal}");
                }
            }
            Ok(())
        }
        StorageCommand::Deals { open } => {
            let names = session.names();
            for (id, deal) in deals(ledger, open)? {
                if open && deal.state != DealState::Open {
                    continue;
                }
                let state = match &deal.state {
                    DealState::Open => "open".to_string(),
                    DealState::Active { provider, paid, .. } => {
                        format!("kept by {}, {paid} round(s) paid", names.account(provider))
                    }
                    DealState::Closed { proven, missed, .. } => {
                        format!("closed: {proven} proven, {missed} missed")
                    }
                };
                println!(
                    "{}  {} leaves, {} × {} rounds of {} Pulses, collateral {}, for {}: {state}",
                    id.short(),
                    deal.size,
                    names.usd(deal.price),
                    deal.rounds,
                    deal.round_length,
                    names.usd(deal.collateral),
                    deal.provider
                        .map(|p| names.account(&p))
                        .unwrap_or_else(|| "anyone".into())
                );
            }
            Ok(())
        }
        StorageCommand::Show { deal: text } => {
            let id = find_deal(ledger, &text)?;
            let value = deal_json(ledger, id, &deal(ledger, id)?)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&value).unwrap_or_default()
            );
            Ok(())
        }
        StorageCommand::Accept {
            from,
            deal: text,
            file,
        } => {
            let id = find_deal(ledger, &text)?;
            let terms = deal(ledger, id)?;
            let mut provider = Provider::new(
                &from,
                Store::new(storage_dir().join(&from)),
                ProviderPolicy::default(),
            );
            println!(
                "{}",
                provider.take(session, ledger, id, &terms, file.as_deref())?
            );
            Ok(())
        }
        StorageCommand::Prove { from } => {
            let mut provider = Provider::new(
                &from,
                Store::new(storage_dir().join(&from)),
                ProviderPolicy::default(),
            );
            let lines = provider.prove(session, ledger)?;
            if lines.is_empty() {
                println!("nothing to prove: no round under way that is not proven yet");
            }
            for line in lines {
                println!("{line}");
            }
            Ok(())
        }
        StorageCommand::Settle { from, deal: text } => {
            let id = find_deal(ledger, &text)?;
            let view = session.send(&from, TxKind::SettleDeal { deal: id }, Vec::new())?;
            crate::agents::print_receipt(session, &view);
            Ok(())
        }
        StorageCommand::Close { from, deal: text } => {
            let id = find_deal(ledger, &text)?;
            let view = session.send(&from, TxKind::CloseDeal { deal: id }, Vec::new())?;
            crate::agents::print_receipt(session, &view);
            Ok(())
        }
        StorageCommand::Serve {
            from,
            min_price,
            max_collateral,
            max_total_collateral,
            max_mib,
            max_total_mib,
            max_duration,
            allow_local,
            allow_private,
            pulses,
        } => {
            let policy = ProviderPolicy {
                accept_open: true,
                accept_named: true,
                min_price: crate::agents::parse_amount(&min_price)?,
                max_collateral: crate::agents::parse_amount(&max_collateral)?,
                max_total_collateral: crate::agents::parse_amount(&max_total_collateral)?,
                max_bytes: max_mib.saturating_mul(1 << 20),
                max_total_bytes: max_total_mib.saturating_mul(1 << 20),
                max_duration,
                allow_local,
                allow_private,
                settle_every: 4,
            };
            let mut provider = Provider::new(&from, Store::new(storage_dir().join(&from)), policy);
            let start = ledger.pulse()?;
            println!(
                "{} provides storage from Pulse {start}; copies under {}",
                session.names().account(&session.id(&from)),
                storage_dir().join(&from).display()
            );
            loop {
                match provider.tick(session, ledger) {
                    Ok(lines) => lines.iter().for_each(|line| println!("{line}")),
                    Err(error) => eprintln!("storage: {error}"),
                }
                let now = ledger.pulse()?;
                if pulses.is_some_and(|n| now.0 >= start.0.saturating_add(n)) {
                    return Ok(());
                }
                std::thread::sleep(std::time::Duration::from_millis(1000));
            }
        }
    }
}
