//! Storage deals (spec v1.12) from the command line: a client posts data
//! for safekeeping, a provider accepts it, proves every round that it still
//! holds it, and is paid for the rounds it proved.
//!
//! The ledger only sees the data's Merkle root. A provider fetches the data
//! itself, from the deal's locator (`file://` or `http(s)://`) or a file it
//! is given, checks that it hashes to the root before it accepts, and keeps
//! its copy under `$SION_HOME/storage/<deal id>`. [`Provider::tick`] is one
//! step of a provider's loop: accept open deals it can fetch, prove the
//! round under way for every deal it holds, collect, and close deals that
//! ended. `sion storage serve` runs it every second; the app and the
//! `storage` demo use it too.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use clap::Subcommand;
use serde_json::{json, Value};
use sion_core::hash::Hash32;
use sion_core::ids::{AccountId, ObjectId, Pulse};
use sion_core::Amount;
use sion_exec::execute::Event;
use sion_exec::object::{ObjectData, ObjectRef};
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
        /// Pulses per round (at least 8).
        #[arg(long, default_value_t = 20)]
        round_length: u64,
        /// Collateral the provider must post (decimal USD).
        #[arg(long, default_value = "0")]
        collateral: String,
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
        /// Where providers fetch the data (default: file://<the file>).
        #[arg(long)]
        locator: Option<String>,
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
    /// Run a provider: accept open deals whose data you can fetch, prove
    /// every round, collect and close, every second until stopped.
    Serve {
        /// Your wallet (the provider).
        from: String,
        /// Accept only deals paying at least this per round (decimal USD).
        #[arg(long, default_value = "0.000001")]
        min_price: String,
        /// Accept only deals asking at most this collateral (decimal USD).
        #[arg(long, default_value = "1000")]
        max_collateral: String,
        /// Accept only data of at most this many MiB.
        #[arg(long, default_value_t = 64)]
        max_mib: u64,
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

    /// Keeps a copy of `data` for `deal`.
    pub fn put(&self, deal: &ObjectId, data: &[u8]) -> Result<(), CliError> {
        std::fs::create_dir_all(&self.dir).map_err(|e| CliError::Usage(e.to_string()))?;
        std::fs::write(self.path(deal), data).map_err(|e| CliError::Usage(e.to_string()))
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

/// Every storage deal on the ledger.
pub fn deals(ledger: &dyn Ledger) -> Result<Vec<(ObjectId, Deal)>, CliError> {
    let value = ledger.query(Query::Deals)?;
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
    let matches: Vec<ObjectId> = deals(ledger)?
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

/// Fetches data from a locator: `file://<path>` or `http(s)://<url>`, at
/// most `max_bytes`.
pub fn fetch(locator: &str, max_bytes: u64) -> Result<Vec<u8>, CliError> {
    if let Some(path) = locator.strip_prefix("file://") {
        let length = std::fs::metadata(path)
            .map_err(|e| CliError::Usage(format!("{locator}: {e}")))?
            .len();
        if length > max_bytes {
            return Err(CliError::Usage(format!(
                "{locator} holds {length} bytes, more than {max_bytes}"
            )));
        }
        return std::fs::read(path).map_err(|e| CliError::Usage(format!("{locator}: {e}")));
    }
    if locator.starts_with("http://") || locator.starts_with("https://") {
        let response = ureq::get(locator)
            .timeout(std::time::Duration::from_secs(60))
            .call()
            .map_err(|e| CliError::Usage(format!("{locator}: {e}")))?;
        let mut data = Vec::new();
        response
            .into_reader()
            .take(max_bytes.saturating_add(1))
            .read_to_end(&mut data)
            .map_err(|e| CliError::Usage(format!("{locator}: {e}")))?;
        if data.len() as u64 > max_bytes {
            return Err(CliError::Usage(format!(
                "{locator} holds more than {max_bytes} bytes"
            )));
        }
        return Ok(data);
    }
    Err(CliError::Usage(format!(
        "cannot fetch {locator:?}: the locator must be file:// or http(s)://"
    )))
}

/// Whether `data` is what `deal` commits to.
pub fn matches_deal(data: &[u8], deal: &Deal) -> bool {
    let tree = DataTree::of_data(data);
    tree.root() == deal.root && tree.size() == deal.size
}

/// The terms a provider accepts open deals on.
#[derive(Clone, Debug)]
pub struct ProviderPolicy {
    /// Accept open deals.
    pub accept_open: bool,
    /// Accept deals that name this provider. With both off, the provider
    /// takes nothing new but keeps proving, collecting and closing the
    /// deals it holds.
    pub accept_named: bool,
    /// The lowest price per round taken.
    pub min_price: Amount,
    /// The most collateral posted for one deal.
    pub max_collateral: Amount,
    /// The most bytes kept for one deal.
    pub max_bytes: u64,
    /// Collect once this many proven rounds are unpaid (closing collects
    /// the rest).
    pub settle_every: u32,
}

impl Default for ProviderPolicy {
    fn default() -> Self {
        ProviderPolicy {
            accept_open: true,
            accept_named: true,
            min_price: Amount(1),
            max_collateral: Amount::units(1_000),
            max_bytes: 64 << 20,
            settle_every: 4,
        }
    }
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
    /// Deals it could not fetch, so it does not try every second.
    skipped: BTreeMap<ObjectId, String>,
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
        }
    }

    /// One step: accept what it can, prove the round under way for every
    /// deal it holds, collect, close what ended. Returns what it did, a
    /// line each.
    pub fn tick(
        &mut self,
        session: &mut Session<'_>,
        ledger: &dyn Ledger,
    ) -> Result<Vec<String>, CliError> {
        let mut done = Vec::new();
        let me = session.id(&self.name);
        let pulse = ledger.pulse()?;
        let all = deals(ledger)?;
        for (id, deal) in &all {
            if deal.state != DealState::Open || Pulse(pulse.0 + 1) > deal.accept_by {
                continue;
            }
            let named = deal.provider == Some(me);
            if !(named && self.policy.accept_named
                || deal.provider.is_none() && self.policy.accept_open)
                || deal.price < self.policy.min_price
                || deal.collateral > self.policy.max_collateral
                || self.skipped.contains_key(id)
            {
                continue;
            }
            match self.take(session, ledger, *id, deal, None) {
                Ok(line) => done.push(line),
                Err(error) => {
                    done.push(format!("skipped deal {}: {error}", id.short()));
                    self.skipped.insert(*id, error.to_string());
                }
            }
        }
        done.extend(self.prove(session, ledger)?);
        let pulse = ledger.pulse()?;
        for (id, deal) in deals(ledger)? {
            let DealState::Active {
                provider,
                custody,
                paid,
                ..
            } = deal.state
            else {
                continue;
            };
            if provider != me {
                continue;
            }
            let ended = deal.end().is_some_and(|end| pulse >= end);
            let proven = match ledger.object(custody)?.map(|o| o.data) {
                Some(ObjectData::Custody(record)) => record.proven,
                _ => continue,
            };
            if ended {
                done.push(self.send_line(session, TxKind::CloseDeal { deal: id }, Vec::new()));
                self.trees.remove(&id);
            } else if proven.saturating_sub(paid) >= self.policy.settle_every.max(1) {
                done.push(self.send_line(session, TxKind::SettleDeal { deal: id }, Vec::new()));
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

    /// Accepts `deal`: fetches its data (from `file`, or its locator),
    /// checks it, keeps a copy and posts the collateral.
    pub fn take(
        &mut self,
        session: &mut Session<'_>,
        _ledger: &dyn Ledger,
        id: ObjectId,
        deal: &Deal,
        file: Option<&Path>,
    ) -> Result<String, CliError> {
        let data = match file {
            Some(path) => std::fs::read(path)
                .map_err(|e| CliError::Usage(format!("{}: {e}", path.display())))?,
            None => fetch(&deal.locator, self.policy.max_bytes)?,
        };
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
        self.store.put(&id, &data)?;
        self.trees.insert(id, tree);
        let view = session.send(&self.name, TxKind::AcceptDeal { deal: id }, Vec::new())?;
        Ok(format!(
            "Pulse {}: {}",
            view.pulse,
            crate::describe::receipt(&session.names(), &view.effects)
        ))
    }

    /// Proves the round under way for every deal this provider holds a
    /// copy of, in as few transactions as the leaf limit allows.
    pub fn prove(
        &mut self,
        session: &mut Session<'_>,
        ledger: &dyn Ledger,
    ) -> Result<Vec<String>, CliError> {
        let me = session.id(&self.name);
        let next = Pulse(ledger.pulse()?.0 + 1);
        // Each batch remembers the last Pulse of the earliest round it
        // proves: the transaction expires then, so a proof that comes too
        // late is rejected (no fee) rather than failed.
        let mut batches: Vec<(Vec<ObjectRef>, Vec<CustodyProof>, u64)> =
            vec![(Vec::new(), Vec::new(), u64::MAX)];
        let mut leaves = 0;
        for (custody_id, custody) in custody_records(ledger, me)? {
            let Some(round) = current_round(custody.start, custody.round_length, next) else {
                continue;
            };
            if round >= u64::from(custody.rounds) || round < u64::from(custody.next) {
                continue;
            }
            let active = matches!(
                deal(ledger, custody.deal).map(|d| d.state),
                Ok(DealState::Active { .. })
            );
            if !active {
                continue;
            }
            let Some(data) = self.store.get(&custody.deal) else {
                continue;
            };
            let tree = self
                .trees
                .entry(custody.deal)
                .or_insert_with(|| DataTree::of_data(&data));
            let Some(proof) = tree.answer(&data, &custody, round as u32) else {
                continue;
            };
            if leaves + proof.leaves.len() > MAX_PROOF_LEAVES {
                batches.push((Vec::new(), Vec::new(), u64::MAX));
                leaves = 0;
            }
            leaves += proof.leaves.len();
            let round_end = custody
                .start
                .0
                .saturating_add((round + 1).saturating_mul(custody.round_length))
                .saturating_sub(1);
            let batch = batches.last_mut().expect("one batch");
            batch.0.push(session.reference(&custody_id)?);
            batch.1.push(proof);
            batch.2 = batch.2.min(round_end);
        }
        let mut done = Vec::new();
        let now = ledger.pulse()?.0;
        let usual = session.wallet(&self.name).validity;
        for (refs, proofs, round_end) in batches.into_iter().filter(|(refs, ..)| !refs.is_empty()) {
            session.wallet(&self.name).validity = round_end.saturating_sub(now).min(usual);
            done.push(self.send_line(session, TxKind::ProveStorage { proofs }, refs));
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
        } => {
            let data = std::fs::read(&file)
                .map_err(|e| CliError::Usage(format!("{}: {e}", file.display())))?;
            let tree = DataTree::of_data(&data);
            let locator = match locator {
                Some(locator) => locator,
                None => format!(
                    "file://{}",
                    std::fs::canonicalize(&file)
                        .map_err(|e| CliError::Usage(e.to_string()))?
                        .display()
                ),
            };
            let provider = provider.map(|p| session.id(&p));
            let accept_by = Pulse(ledger.pulse()?.0 + accept_within.max(1));
            let view = session.send(
                &from,
                TxKind::PostDeal {
                    root: tree.root(),
                    size: tree.size(),
                    challenges,
                    price: crate::agents::parse_amount(&price)?,
                    rounds,
                    round_length,
                    collateral: crate::agents::parse_amount(&collateral)?,
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
            for (id, deal) in deals(ledger)? {
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
            max_mib,
            pulses,
        } => {
            let policy = ProviderPolicy {
                accept_open: true,
                accept_named: true,
                min_price: crate::agents::parse_amount(&min_price)?,
                max_collateral: crate::agents::parse_amount(&max_collateral)?,
                max_bytes: max_mib.saturating_mul(1 << 20),
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
